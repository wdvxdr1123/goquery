//! Complete unified interfaces (methods, embedded interfaces, unions, type terms).
use super::ty::Term;
use super::{
    BasicKind, CheckOutcome, MAX_BYTES, MAX_DEPTH, Operand, Ty, TypeArena, TypeId, UnknownReason,
};
use crate::syntax::{Lexer, TokenKind};
use std::collections::{BTreeMap, HashSet};
use std::ops::Range;
use std::path::Path;

fn matching_outer_parens(text: &str) -> Option<&str> {
    let mut tokens = Lexer::new(text);
    let first = tokens.next()?;
    if first.kind != TokenKind::LParen {
        return None;
    }
    let mut depth = 1usize;
    let mut close_start = None;
    for token in tokens {
        if close_start.is_some() {
            return None;
        }
        match token.kind {
            TokenKind::LParen => depth += 1,
            TokenKind::RParen => {
                depth -= 1;
                if depth == 0 {
                    close_start = Some(token.span.start);
                }
            }
            _ => {}
        }
    }
    close_start.map(|end| &text[first.span.end..end])
}

fn split_top_level_union(text: &str) -> Option<Vec<(usize, &str)>> {
    let mut nesting = 0usize;
    let mut start = 0usize;
    let mut parts = Vec::new();
    for token in Lexer::new(text) {
        match token.kind {
            TokenKind::LParen | TokenKind::LBrack | TokenKind::LBrace => nesting += 1,
            TokenKind::RParen | TokenKind::RBrack | TokenKind::RBrace => {
                nesting = nesting.checked_sub(1)?;
            }
            TokenKind::Or if nesting == 0 => {
                parts.push((start, text.get(start..token.span.start)?));
                start = token.span.end;
            }
            _ => {}
        }
    }
    if nesting != 0 {
        return None;
    }
    if !parts.is_empty() {
        parts.push((start, text.get(start..)?));
    }
    Some(parts)
}

#[derive(Clone)]
pub(super) struct Method<'a> {
    pub(super) name: String,
    pub(super) origin: &'a Path,
    pub(super) signature: TypeId,
    /// Declaring interface instance for display after method-set flattening.
    /// This provenance does not participate in type identity.
    pub(super) declared_on: Option<TypeId>,
    pub(super) selection: [u32; 2],
    pub(super) builtin: bool,
}

impl<'a> TypeArena<'a> {
    pub(super) fn read_interface_at_checked(
        &mut self,
        origin: &Path,
        source: &str,
        file_offset: Option<usize>,
        environment: &[(&str, TypeId)],
        depth: usize,
    ) -> CheckOutcome<TypeId> {
        if depth >= MAX_DEPTH || source.len() > MAX_BYTES {
            return CheckOutcome::Unknown(UnknownReason::BudgetExceeded);
        }
        let Some(origin) = self
            .workspace
            .files
            .get_key_value(origin)
            .map(|(p, _)| p.as_path())
        else {
            return CheckOutcome::Unknown(UnknownReason::MissingMetadata);
        };
        let parsed = crate::syntax::parse_type(source);
        if !parsed.diagnostics.is_empty() {
            return CheckOutcome::Unknown(UnknownReason::SyntaxError);
        }
        let Some(container) = Lexer::new(source).nth(1).map(|t| t.span.start) else {
            return CheckOutcome::Unknown(UnknownReason::SyntaxError);
        };
        let mut methods = Vec::new();
        let mut embeddeds = Vec::new();
        let mut has_type_elements = false;
        let mut comparable = false;
        let mut explicit = HashSet::new();
        for member in parsed
            .interface_members
            .iter()
            .filter(|m| m.container == container)
        {
            if let Some(name_span) = &member.name {
                let Some(name) = source.get(name_span.clone()) else {
                    return CheckOutcome::Unknown(UnknownReason::SyntaxError);
                };
                if name == "_" || !explicit.insert(name) {
                    return CheckOutcome::Invalid(());
                }
                let Some(header) = source.get(member.annotation.clone()) else {
                    return CheckOutcome::Unknown(UnknownReason::SyntaxError);
                };
                let signature = crate::syntax::parse_method_signature(header);
                let Some(signature) = self.signature(
                    origin,
                    header,
                    file_offset.map(|base| base + member.annotation.start),
                    &signature,
                    environment,
                    depth + 1,
                ) else {
                    return CheckOutcome::Unknown(UnknownReason::Unsupported);
                };
                let selection = file_offset
                    .and_then(|base| {
                        Some([
                            u32::try_from(base + name_span.start).ok()?,
                            u32::try_from(base + name_span.end).ok()?,
                        ])
                    })
                    .unwrap_or([0, 0]);
                methods.push(Method {
                    name: name.to_owned(),
                    origin,
                    signature,
                    declared_on: None,
                    selection,
                    builtin: false,
                });
            } else {
                let Some(text) = source.get(member.annotation.clone()) else {
                    return CheckOutcome::Unknown(UnknownReason::SyntaxError);
                };
                match self.read_interface_element_checked(
                    origin,
                    text,
                    file_offset.map(|base| base + member.annotation.start),
                    environment,
                    depth + 1,
                    &mut methods,
                    &mut embeddeds,
                    &mut has_type_elements,
                    &mut comparable,
                ) {
                    CheckOutcome::Ok(()) => {}
                    CheckOutcome::Invalid(()) => return CheckOutcome::Invalid(()),
                    CheckOutcome::Unknown(reason) => return CheckOutcome::Unknown(reason),
                }
            }
            if methods.len() > self.active_node_limit()
                || embeddeds.len() > self.active_node_limit()
            {
                return CheckOutcome::Unknown(UnknownReason::BudgetExceeded);
            }
        }
        self.store_unified_interface_checked(
            origin,
            methods,
            embeddeds,
            has_type_elements,
            comparable,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn read_interface_element_checked(
        &mut self,
        origin: &Path,
        raw_text: &str,
        file_offset: Option<usize>,
        environment: &[(&str, TypeId)],
        depth: usize,
        methods: &mut Vec<Method<'a>>,
        embeddeds: &mut Vec<TypeId>,
        has_type_elements: &mut bool,
        comparable: &mut bool,
    ) -> CheckOutcome<()> {
        if depth >= MAX_DEPTH {
            return CheckOutcome::Unknown(UnknownReason::BudgetExceeded);
        }
        let tokens: Vec<_> = Lexer::new(raw_text).collect();
        let (Some(first), Some(last)) = (
            tokens.first(),
            tokens.iter().rfind(|t| t.kind != TokenKind::Semicolon),
        ) else {
            return CheckOutcome::Unknown(UnknownReason::SyntaxError);
        };
        let Some(text) = raw_text.get(first.span.start..last.span.end) else {
            return CheckOutcome::Unknown(UnknownReason::SyntaxError);
        };
        let element_offset = file_offset.map(|base| base + first.span.start);
        if let Some(parts) = split_top_level_union(text) {
            if !parts.is_empty() {
                if parts.len() > super::sets::MAX_TERMS {
                    return CheckOutcome::Unknown(UnknownReason::BudgetExceeded);
                }
                *has_type_elements = true;
                let mut terms = Vec::with_capacity(parts.len());
                for (part_rel, part) in parts {
                    match self.read_union_part_checked(
                        origin,
                        part,
                        element_offset.map(|b| b + part_rel),
                        environment,
                        depth + 1,
                        &mut terms,
                    ) {
                        CheckOutcome::Ok(()) => {}
                        CheckOutcome::Invalid(()) => return CheckOutcome::Invalid(()),
                        CheckOutcome::Unknown(reason) => return CheckOutcome::Unknown(reason),
                    }
                }
                let Some(union_ty) = self.store_union(terms) else {
                    return CheckOutcome::Unknown(UnknownReason::BudgetExceeded);
                };
                embeddeds.push(union_ty);
                return CheckOutcome::Ok(());
            }
        } else {
            return CheckOutcome::Unknown(UnknownReason::SyntaxError);
        }
        if let Some(inner) = matching_outer_parens(text) {
            let inner_offset = element_offset.map(|b| b + 1);
            return self.read_interface_element_checked(
                origin,
                inner,
                inner_offset,
                environment,
                depth + 1,
                methods,
                embeddeds,
                has_type_elements,
                comparable,
            );
        }
        if matches!(text, "any" | "comparable")
            && self.universe_constraint(origin, text, environment) == Some(true)
        {
            if text == "comparable" {
                *comparable = true;
                *has_type_elements = true;
            }
            return CheckOutcome::Ok(());
        }
        if first.kind == TokenKind::Tilde {
            let term = match self.read_single_term_checked(
                origin,
                text,
                element_offset,
                environment,
                depth + 1,
            ) {
                CheckOutcome::Ok(term) => term,
                CheckOutcome::Invalid(()) => return CheckOutcome::Invalid(()),
                CheckOutcome::Unknown(reason) => return CheckOutcome::Unknown(reason),
            };
            let Some(union_ty) = self.store_union(vec![term]) else {
                return CheckOutcome::Unknown(UnknownReason::BudgetExceeded);
            };
            *has_type_elements = true;
            embeddeds.push(union_ty);
            return CheckOutcome::Ok(());
        }
        if environment.iter().any(|(name, _)| *name == text) {
            return CheckOutcome::Invalid(());
        }
        let ty = match self.parse_in_checked(origin, text, element_offset, environment) {
            CheckOutcome::Ok(ty) => ty,
            CheckOutcome::Invalid(()) => return CheckOutcome::Invalid(()),
            CheckOutcome::Unknown(reason) => return CheckOutcome::Unknown(reason),
        };
        match self.interface_constructor(ty) {
            Some(true) => {
                let Some(under) = self.underlying(ty) else {
                    return CheckOutcome::Unknown(UnknownReason::MissingMetadata);
                };
                let Some(iface) = self.interface_data(under).cloned() else {
                    return CheckOutcome::Unknown(UnknownReason::Unsupported);
                };
                methods.extend(self.methods_slice(under, iface.methods).iter().cloned());
                *comparable |= iface.comparable;
                *has_type_elements |= iface.has_type_elements;
                if iface.has_type_elements || iface.comparable || !iface.embeddeds.is_empty() {
                    embeddeds.push(ty);
                }
                CheckOutcome::Ok(())
            }
            Some(false) => {
                let Some(unaliased) = self.unalias(ty) else {
                    return CheckOutcome::Unknown(UnknownReason::MissingMetadata);
                };
                if matches!(self.node(unaliased), Some(Ty::TypeParam(_))) {
                    return CheckOutcome::Invalid(());
                }
                *has_type_elements = true;
                embeddeds.push(ty);
                CheckOutcome::Ok(())
            }
            None => CheckOutcome::Unknown(UnknownReason::MissingMetadata),
        }
    }

    fn read_union_part_checked(
        &mut self,
        origin: &Path,
        raw_part: &str,
        file_offset: Option<usize>,
        environment: &[(&str, TypeId)],
        depth: usize,
        out: &mut Vec<Term>,
    ) -> CheckOutcome<()> {
        if depth >= MAX_DEPTH {
            return CheckOutcome::Unknown(UnknownReason::BudgetExceeded);
        }
        let tokens: Vec<_> = Lexer::new(raw_part).collect();
        let (Some(first), Some(last)) = (
            tokens.first(),
            tokens.iter().rfind(|t| t.kind != TokenKind::Semicolon),
        ) else {
            return CheckOutcome::Unknown(UnknownReason::SyntaxError);
        };
        let Some(text) = raw_part.get(first.span.start..last.span.end) else {
            return CheckOutcome::Unknown(UnknownReason::SyntaxError);
        };
        let part_offset = file_offset.map(|b| b + first.span.start);
        if let Some(parts) = split_top_level_union(text) {
            if !parts.is_empty() {
                for (sub_rel, sub) in parts {
                    match self.read_union_part_checked(
                        origin,
                        sub,
                        part_offset.map(|b| b + sub_rel),
                        environment,
                        depth + 1,
                        out,
                    ) {
                        CheckOutcome::Ok(()) => {}
                        other => return other,
                    }
                    if out.len() > super::sets::MAX_TERMS {
                        return CheckOutcome::Unknown(UnknownReason::BudgetExceeded);
                    }
                }
                return CheckOutcome::Ok(());
            }
        } else {
            return CheckOutcome::Unknown(UnknownReason::SyntaxError);
        }
        if let Some(inner) = matching_outer_parens(text) {
            return self.read_union_part_checked(
                origin,
                inner,
                part_offset.map(|b| b + 1),
                environment,
                depth + 1,
                out,
            );
        }
        let term = match self.read_single_term_checked(
            origin,
            text,
            part_offset,
            environment,
            depth + 1,
        ) {
            CheckOutcome::Ok(term) => term,
            CheckOutcome::Invalid(()) => return CheckOutcome::Invalid(()),
            CheckOutcome::Unknown(reason) => return CheckOutcome::Unknown(reason),
        };
        if out.len() >= super::sets::MAX_TERMS {
            return CheckOutcome::Unknown(UnknownReason::BudgetExceeded);
        }
        out.push(term);
        CheckOutcome::Ok(())
    }

    fn read_single_term_checked(
        &mut self,
        origin: &Path,
        text: &str,
        file_offset: Option<usize>,
        environment: &[(&str, TypeId)],
        depth: usize,
    ) -> CheckOutcome<Term> {
        let Some(first) = Lexer::new(text).next() else {
            return CheckOutcome::Unknown(UnknownReason::SyntaxError);
        };
        if matches!(text, "any" | "comparable")
            && self.universe_constraint(origin, text, environment) == Some(true)
        {
            if text == "comparable" {
                return CheckOutcome::Invalid(());
            }
            return match self.builtin_interface(origin, false) {
                Some(ty) => CheckOutcome::Ok(Term {
                    ty,
                    approximate: false,
                }),
                None => CheckOutcome::Unknown(UnknownReason::BudgetExceeded),
            };
        }
        let approximate = first.kind == TokenKind::Tilde;
        let term_offset = if approximate {
            let rest = text.get(1..).unwrap_or("");
            let trimmed = rest.trim();
            let shift = rest.len() - rest.trim_start().len();
            let _ = trimmed;
            file_offset.map(|b| b + 1 + shift)
        } else {
            file_offset
        };
        let term_str = if approximate {
            let Some(rest) = text.get(1..) else {
                return CheckOutcome::Unknown(UnknownReason::SyntaxError);
            };
            rest.trim()
        } else {
            text
        };
        if environment.iter().any(|(name, _)| *name == term_str) {
            return CheckOutcome::Invalid(());
        }
        if term_str.starts_with("interface")
            && Lexer::new(term_str)
                .next()
                .is_some_and(|t| t.kind == TokenKind::Interface)
        {
            if approximate {
                return CheckOutcome::Invalid(());
            }
            let iface = match self.read_interface_at_checked(
                origin,
                term_str,
                term_offset,
                environment,
                depth + 1,
            ) {
                CheckOutcome::Ok(id) => id,
                CheckOutcome::Invalid(()) => return CheckOutcome::Invalid(()),
                CheckOutcome::Unknown(reason) => return CheckOutcome::Unknown(reason),
            };
            return CheckOutcome::Ok(Term {
                ty: iface,
                approximate: false,
            });
        }
        let ty = match self.parse_in_checked(origin, term_str, term_offset, environment) {
            CheckOutcome::Ok(ty) => ty,
            CheckOutcome::Invalid(()) => return CheckOutcome::Invalid(()),
            CheckOutcome::Unknown(reason) => return CheckOutcome::Unknown(reason),
        };
        let Some(is_iface) = self.interface_constructor(ty) else {
            return CheckOutcome::Unknown(UnknownReason::MissingMetadata);
        };
        if is_iface && approximate {
            return CheckOutcome::Invalid(());
        }
        let Some(unaliased) = self.unalias(ty) else {
            return CheckOutcome::Unknown(UnknownReason::MissingMetadata);
        };
        if matches!(self.node(unaliased), Some(Ty::TypeParam(_)))
            || (approximate && matches!(self.node(unaliased), Some(Ty::Named { .. })))
        {
            return CheckOutcome::Invalid(());
        }
        if self.underlying(unaliased).is_none() {
            return CheckOutcome::Unknown(UnknownReason::MissingMetadata);
        }
        CheckOutcome::Ok(Term {
            ty: unaliased,
            approximate,
        })
    }

    pub(super) fn builtin_interface(&mut self, origin: &Path, error: bool) -> Option<TypeId> {
        let origin = self.workspace.files.get_key_value(origin)?.0.as_path();
        let methods = if error {
            let result = self.basic(BasicKind::String)?;
            let signature = self.store_signature(Vec::new(), vec![result], false)?;
            vec![Method {
                name: "Error".to_owned(),
                origin,
                signature,
                declared_on: None,
                selection: [0, 0],
                builtin: true,
            }]
        } else {
            Vec::new()
        };
        let interface =
            self.store_interface_nodes(origin, methods, Vec::new(), false, false, !error)?;
        if error {
            self.push_builtin_error(interface)
        } else {
            Some(interface)
        }
    }

    pub(super) fn store_interface(
        &mut self,
        origin: &'a Path,
        methods: Vec<Method<'a>>,
    ) -> Option<TypeId> {
        self.store_interface_checked(origin, methods).ok()
    }

    pub(super) fn store_interface_checked(
        &mut self,
        origin: &'a Path,
        methods: Vec<Method<'a>>,
    ) -> CheckOutcome<TypeId> {
        self.store_unified_interface_checked(origin, methods, Vec::new(), false, false)
    }

    pub(super) fn store_unified_interface_checked(
        &mut self,
        origin: &'a Path,
        methods: Vec<Method<'a>>,
        embeddeds: Vec<TypeId>,
        has_type_elements: bool,
        comparable: bool,
    ) -> CheckOutcome<TypeId> {
        let mut sorted: BTreeMap<(String, String), Method<'a>> = BTreeMap::new();
        for method in methods {
            let package = if crate::syntax::is_exported_name(&method.name) {
                String::new()
            } else if let Some(path) = self.workspace.import_path(method.origin) {
                path
            } else if self.mode.is_symbolic() {
                let Some(parent) = method.origin.parent().and_then(Path::to_str) else {
                    return CheckOutcome::Unknown(UnknownReason::MissingMetadata);
                };
                let Some(stub) = self.workspace.files.get(method.origin) else {
                    return CheckOutcome::Unknown(UnknownReason::MissingMetadata);
                };
                format!("{}:{}", parent, stub.package)
            } else {
                return CheckOutcome::Unknown(UnknownReason::MissingMetadata);
            };
            let key = (method.name.clone(), package);
            if let Some(previous) = sorted.get(&key) {
                let Some(same) = self.identical(previous.signature, method.signature) else {
                    return CheckOutcome::Unknown(UnknownReason::BudgetExceeded);
                };
                if !same {
                    return CheckOutcome::Invalid(());
                }
            } else {
                sorted.insert(key, method);
            }
        }
        match self.store_interface_nodes(
            origin,
            sorted.into_values().collect(),
            embeddeds,
            has_type_elements,
            comparable,
            false,
        ) {
            Some(id) => CheckOutcome::Ok(id),
            None => CheckOutcome::Unknown(UnknownReason::BudgetExceeded),
        }
    }

    pub(super) fn same_interface_methods(
        &self,
        left: TypeId,
        a: Range<usize>,
        right: TypeId,
        b: Range<usize>,
    ) -> Option<bool> {
        if a.len() != b.len() {
            return Some(false);
        }
        for (a, b) in self
            .methods_slice(left, a)
            .iter()
            .zip(self.methods_slice(right, b))
        {
            if a.name != b.name
                || !crate::syntax::is_exported_name(&a.name)
                    && !self.same_field_package(a.origin, b.origin)?
            {
                return Some(false);
            }
        }
        Some(true)
    }

    pub(crate) fn is_interface(&mut self, input: TypeId) -> Option<bool> {
        let input = self.underlying(input)?;
        Some(matches!(self.node(input)?, Ty::Interface(_)))
    }

    /// Inference must not instantiate constrained named formals before their
    /// arguments have been inferred. Constructor metadata is sufficient here;
    /// assignment checks the concrete instantiated type afterward.
    pub(super) fn interface_constructor(&mut self, input: TypeId) -> Option<bool> {
        if let Some((declaration, _, _)) = self.named_info(input) {
            let (mut file, mut symbol) = (declaration.file, declaration.symbol);
            for _ in 0..MAX_DEPTH {
                let Some(info) = &symbol.type_info else {
                    return Some(false);
                };
                if info.is_interface() {
                    return Some(true);
                }
                let Some(target) = &info.target else {
                    return Some(false);
                };
                if Lexer::new(&target.name).next()?.kind != TokenKind::Ident {
                    return Some(false);
                }
                let Some((next_file, next_symbol)) =
                    self.resolver().resolve_type_declaration(file, &target.name)
                else {
                    return Some(matches!(target.name.as_str(), "any" | "error"));
                };
                (file, symbol) = (next_file, next_symbol);
            }
            return None;
        }
        self.is_interface(input)
    }

    pub(super) fn assign_interface(
        &mut self,
        source: Operand,
        target: TypeId,
        int_bits: u32,
    ) -> Option<bool> {
        if self.basic_kind(source.ty) == Some(BasicKind::UntypedNil) {
            return Some(true);
        }
        let source = self.default_operand(source, int_bits)?;
        let target_root = self.underlying(target)?;
        let data = self.interface_data(target_root)?;
        if data.has_type_elements || data.comparable {
            return None;
        }
        let required = data.methods.clone();
        if required.is_empty() {
            return Some(true);
        }
        let methods = self.methods_slice(target_root, required).to_vec();
        for method in methods {
            let Some(signature) = self.lookup_method(source, &method)? else {
                return Some(false);
            };
            if !self.identical(method.signature, signature)? {
                return Some(false);
            }
        }
        Some(true)
    }

    /// A concrete asserted type must implement the source interface. An
    /// interface target follows go/types assertableTo even when methods
    /// conflict. The compiler's stricter impossible-interface diagnostics are
    /// not modeled here. Do not apply selector auto-addressing.
    pub(crate) fn assertion(
        &mut self,
        source: Operand,
        target: TypeId,
        int_bits: u32,
    ) -> Option<Operand> {
        self.step()?;
        if !self.is_interface(source.ty)? {
            return None;
        }
        if !self.is_interface(target)?
            && !self.assign_interface(
                Operand {
                    mode: super::Mode::Value,
                    ty: target,
                    value: None,
                },
                source.ty,
                int_bits,
            )?
        {
            return None;
        }
        Some(Operand {
            mode: super::Mode::CommaOk,
            ty: target,
            value: None,
        })
    }

    /// Case variables narrow only for exactly one non-nil type. Duplicate
    /// aliases/structural identities and nil across the entire switch, as well
    /// as repeated defaults, are invalid. Only the selected clause projects.
    pub(crate) fn type_switch(
        &mut self,
        source: Operand,
        targets: &[Option<TypeId>],
        selected: Range<usize>,
        defaults: usize,
        int_bits: u32,
    ) -> Option<Operand> {
        self.step()?;
        if targets.len() > self.active_node_limit()
            || defaults > 1
            || !self.is_interface(source.ty)?
        {
            return None;
        }
        for (index, target) in targets.iter().enumerate() {
            for previous in &targets[..index] {
                self.step()?;
                if match (target, previous) {
                    (Some(a), Some(b)) => self.identical(*a, *b)?,
                    (None, None) => true,
                    _ => false,
                } {
                    return None;
                }
            }
            if let Some(target) = target {
                self.assertion(source, *target, int_bits)?;
            }
        }
        Some(Operand {
            mode: super::Mode::Variable,
            ty: if let [Some(target)] = targets.get(selected)? {
                *target
            } else {
                source.ty
            },
            value: None,
        })
    }
}
