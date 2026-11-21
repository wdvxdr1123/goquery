//! Complete unified interfaces (methods, embedded interfaces, unions, type terms).
use super::ty::Term;
use super::{
    BasicKind, CheckOutcome, MAX_BYTES, MAX_DEPTH, Operand, Ty, TypeArena, TypeId, UnknownReason,
};
use crate::syntax::{InterfaceMemberNode, Lexer, TokenKind, TypeNodeId, TypeSyntax, TypeTreeKind};
use std::collections::{BTreeMap, HashSet};
use std::ops::Range;
use std::path::Path;

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
    /// Parse a type parameter constraint (bare term, union, `any`, `comparable`,
    /// named interface, or interface literal) into a single unified interface `TypeId`.
    pub(super) fn parse_constraint_checked(
        &mut self,
        origin: &Path,
        source: &str,
        file_offset: Option<usize>,
        environment: &[(&str, TypeId)],
        depth: usize,
    ) -> CheckOutcome<TypeId> {
        if self.step().is_none() || depth >= MAX_DEPTH || source.len() > MAX_BYTES {
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
        let parsed = crate::syntax::parse_constraint(source);
        if !parsed.diagnostics.is_empty() || parsed.tree_overflowed() {
            return CheckOutcome::Unknown(UnknownReason::SyntaxError);
        }
        let Some(root) = parsed.root() else {
            return CheckOutcome::Unknown(UnknownReason::SyntaxError);
        };
        self.lower_constraint_node(
            origin,
            source,
            &parsed,
            root,
            file_offset,
            environment,
            depth,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn lower_constraint_node(
        &mut self,
        origin: &'a Path,
        source: &str,
        parsed: &TypeSyntax,
        mut node_id: TypeNodeId,
        file_offset: Option<usize>,
        environment: &[(&str, TypeId)],
        depth: usize,
    ) -> CheckOutcome<TypeId> {
        if depth >= MAX_DEPTH {
            return CheckOutcome::Unknown(UnknownReason::BudgetExceeded);
        }
        let mut unwrap_depth = 0;
        while let Some(node) = parsed.tree_node(node_id) {
            match node.kind {
                TypeTreeKind::Parenthesized(inner) => {
                    unwrap_depth += 1;
                    if unwrap_depth >= MAX_DEPTH {
                        return CheckOutcome::Unknown(UnknownReason::BudgetExceeded);
                    }
                    node_id = inner;
                }
                _ => break,
            }
        }
        let Some(node) = parsed.tree_node(node_id) else {
            return CheckOutcome::Unknown(UnknownReason::SyntaxError);
        };
        let span = node.byte_range();
        let Some(text) = source.get(span.clone()) else {
            return CheckOutcome::Unknown(UnknownReason::SyntaxError);
        };
        let node_offset = file_offset.map(|base| base + span.start);
        match node.kind {
            TypeTreeKind::Missing => CheckOutcome::Unknown(UnknownReason::SyntaxError),
            TypeTreeKind::Union { ref terms } => {
                let term_ids = parsed.type_node_list(terms.clone());
                if term_ids.len() > super::sets::MAX_TERMS {
                    return CheckOutcome::Unknown(UnknownReason::BudgetExceeded);
                }
                let mut lowered = Vec::with_capacity(term_ids.len());
                for &term_id in term_ids {
                    match self.lower_union_term_list(
                        origin,
                        source,
                        parsed,
                        term_id,
                        file_offset,
                        environment,
                        depth + 1,
                        &mut lowered,
                    ) {
                        CheckOutcome::Ok(()) => {}
                        CheckOutcome::Invalid(()) => return CheckOutcome::Invalid(()),
                        CheckOutcome::Unknown(reason) => return CheckOutcome::Unknown(reason),
                    }
                }
                let Some(union_ty) = self.store_union(lowered) else {
                    return CheckOutcome::Unknown(UnknownReason::BudgetExceeded);
                };
                match self.store_interface_nodes(
                    origin,
                    Vec::new(),
                    vec![union_ty],
                    true,
                    false,
                    true,
                    false,
                ) {
                    Some(id) => CheckOutcome::Ok(id),
                    None => CheckOutcome::Unknown(UnknownReason::BudgetExceeded),
                }
            }
            TypeTreeKind::Approx(_) => {
                let term = match self.lower_single_term(
                    origin,
                    source,
                    parsed,
                    node_id,
                    file_offset,
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
                match self.store_interface_nodes(
                    origin,
                    Vec::new(),
                    vec![union_ty],
                    true,
                    false,
                    true,
                    false,
                ) {
                    Some(id) => CheckOutcome::Ok(id),
                    None => CheckOutcome::Unknown(UnknownReason::BudgetExceeded),
                }
            }
            TypeTreeKind::Interface { ref members } => self.lower_interface_members_checked(
                origin,
                source,
                parsed,
                members.clone(),
                file_offset,
                environment,
                depth + 1,
            ),
            TypeTreeKind::Name { .. }
                if matches!(text, "any" | "comparable")
                    && self.universe_constraint(origin, text, environment) == Some(true) =>
            {
                let id = if text == "comparable" {
                    self.builtin_comparable_interface(origin)
                } else {
                    self.builtin_interface(origin, false)
                };
                match id {
                    Some(id) => CheckOutcome::Ok(id),
                    None => CheckOutcome::Unknown(UnknownReason::BudgetExceeded),
                }
            }
            _ => {
                if let Some((_, param_ty)) = environment.iter().find(|(name, _)| *name == text)
                    && matches!(self.node(*param_ty), Some(Ty::TypeParam(_)))
                {
                    return CheckOutcome::Invalid(());
                }
                let ty = match self.parse_in_checked(origin, text, node_offset, environment) {
                    CheckOutcome::Ok(ty) => ty,
                    CheckOutcome::Invalid(()) => return CheckOutcome::Invalid(()),
                    CheckOutcome::Unknown(reason) => return CheckOutcome::Unknown(reason),
                };
                match self.interface_constructor(ty) {
                    Some(true) => CheckOutcome::Ok(ty),
                    Some(false) => {
                        let Some(unaliased) = self.unalias(ty) else {
                            return CheckOutcome::Unknown(UnknownReason::MissingMetadata);
                        };
                        if matches!(self.node(unaliased), Some(Ty::TypeParam(_))) {
                            return CheckOutcome::Invalid(());
                        }
                        if self.underlying(unaliased).is_none() {
                            return CheckOutcome::Unknown(UnknownReason::MissingMetadata);
                        }
                        match self.store_interface_nodes(
                            origin,
                            Vec::new(),
                            vec![unaliased],
                            true,
                            false,
                            true,
                            false,
                        ) {
                            Some(id) => CheckOutcome::Ok(id),
                            None => CheckOutcome::Unknown(UnknownReason::BudgetExceeded),
                        }
                    }
                    None => CheckOutcome::Unknown(UnknownReason::MissingMetadata),
                }
            }
        }
    }

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
        if !parsed.diagnostics.is_empty() || parsed.tree_overflowed() {
            return CheckOutcome::Unknown(UnknownReason::SyntaxError);
        }
        let Some(root) = parsed.root() else {
            return CheckOutcome::Unknown(UnknownReason::SyntaxError);
        };
        let Some(TypeTreeKind::Interface { members }) = parsed.tree_node(root).map(|n| &n.kind)
        else {
            return CheckOutcome::Unknown(UnknownReason::SyntaxError);
        };
        self.lower_interface_members_checked(
            origin,
            source,
            &parsed,
            members.clone(),
            file_offset,
            environment,
            depth,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn lower_interface_members_checked(
        &mut self,
        origin: &'a Path,
        source: &str,
        parsed: &TypeSyntax,
        members_range: Range<u32>,
        file_offset: Option<usize>,
        environment: &[(&str, TypeId)],
        depth: usize,
    ) -> CheckOutcome<TypeId> {
        if depth >= MAX_DEPTH {
            return CheckOutcome::Unknown(UnknownReason::BudgetExceeded);
        }
        let mut methods = Vec::new();
        let mut embeddeds = Vec::new();
        let mut has_type_elements = false;
        let mut comparable = false;
        let mut explicit = HashSet::new();
        for member in parsed.interface_members(members_range) {
            match member {
                InterfaceMemberNode::Method {
                    name: name_span,
                    sig,
                } => {
                    let name_range = name_span.start as usize..name_span.end as usize;
                    let Some(name) = source.get(name_range.clone()) else {
                        return CheckOutcome::Unknown(UnknownReason::SyntaxError);
                    };
                    if name == "_" || !explicit.insert(name) {
                        return CheckOutcome::Invalid(());
                    }
                    let Some(sig_node) = parsed.sig_node(*sig) else {
                        return CheckOutcome::Unknown(UnknownReason::SyntaxError);
                    };
                    let sig_range = sig_node.span.start as usize..sig_node.span.end as usize;
                    let Some(header) = source.get(sig_range.clone()) else {
                        return CheckOutcome::Unknown(UnknownReason::SyntaxError);
                    };
                    let signature = crate::syntax::parse_method_signature(header);
                    let Some(signature) = self.signature(
                        origin,
                        header,
                        file_offset.map(|base| base + sig_range.start),
                        &signature,
                        environment,
                        depth + 1,
                    ) else {
                        return CheckOutcome::Unknown(UnknownReason::Unsupported);
                    };
                    let selection = file_offset
                        .and_then(|base| {
                            Some([
                                u32::try_from(base + name_range.start).ok()?,
                                u32::try_from(base + name_range.end).ok()?,
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
                }
                InterfaceMemberNode::Embedded(emb_node) => {
                    match self.lower_interface_embedded_node(
                        origin,
                        source,
                        parsed,
                        *emb_node,
                        file_offset,
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
            false,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn lower_interface_embedded_node(
        &mut self,
        origin: &'a Path,
        source: &str,
        parsed: &TypeSyntax,
        mut node_id: TypeNodeId,
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
        let mut unwrap_depth = 0;
        while let Some(node) = parsed.tree_node(node_id) {
            match node.kind {
                TypeTreeKind::Parenthesized(inner) => {
                    unwrap_depth += 1;
                    if unwrap_depth >= MAX_DEPTH {
                        return CheckOutcome::Unknown(UnknownReason::BudgetExceeded);
                    }
                    node_id = inner;
                }
                _ => break,
            }
        }
        let Some(node) = parsed.tree_node(node_id) else {
            return CheckOutcome::Unknown(UnknownReason::SyntaxError);
        };
        let span = node.byte_range();
        let Some(text) = source.get(span.clone()) else {
            return CheckOutcome::Unknown(UnknownReason::SyntaxError);
        };
        let element_offset = file_offset.map(|base| base + span.start);
        match node.kind {
            TypeTreeKind::Missing => CheckOutcome::Unknown(UnknownReason::SyntaxError),
            TypeTreeKind::Union { ref terms } => {
                let term_ids = parsed.type_node_list(terms.clone());
                if term_ids.len() > super::sets::MAX_TERMS {
                    return CheckOutcome::Unknown(UnknownReason::BudgetExceeded);
                }
                *has_type_elements = true;
                let mut lowered = Vec::with_capacity(term_ids.len());
                for &term_id in term_ids {
                    match self.lower_union_term_list(
                        origin,
                        source,
                        parsed,
                        term_id,
                        file_offset,
                        environment,
                        depth + 1,
                        &mut lowered,
                    ) {
                        CheckOutcome::Ok(()) => {}
                        CheckOutcome::Invalid(()) => return CheckOutcome::Invalid(()),
                        CheckOutcome::Unknown(reason) => return CheckOutcome::Unknown(reason),
                    }
                }
                let Some(union_ty) = self.store_union(lowered) else {
                    return CheckOutcome::Unknown(UnknownReason::BudgetExceeded);
                };
                embeddeds.push(union_ty);
                CheckOutcome::Ok(())
            }
            TypeTreeKind::Approx(_) => {
                let term = match self.lower_single_term(
                    origin,
                    source,
                    parsed,
                    node_id,
                    file_offset,
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
                CheckOutcome::Ok(())
            }
            TypeTreeKind::Name { .. }
                if matches!(text, "any" | "comparable")
                    && self.universe_constraint(origin, text, environment) == Some(true) =>
            {
                if text == "comparable" {
                    *comparable = true;
                    *has_type_elements = true;
                }
                CheckOutcome::Ok(())
            }
            _ => {
                if let Some((_, param_ty)) = environment.iter().find(|(name, _)| *name == text)
                    && matches!(self.node(*param_ty), Some(Ty::TypeParam(_)))
                {
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
                        if iface.has_type_elements
                            || iface.comparable
                            || !iface.embeddeds.is_empty()
                        {
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
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn lower_union_term_list(
        &mut self,
        origin: &'a Path,
        source: &str,
        parsed: &TypeSyntax,
        mut node_id: TypeNodeId,
        file_offset: Option<usize>,
        environment: &[(&str, TypeId)],
        depth: usize,
        out: &mut Vec<Term>,
    ) -> CheckOutcome<()> {
        if depth >= MAX_DEPTH {
            return CheckOutcome::Unknown(UnknownReason::BudgetExceeded);
        }
        let mut unwrap_depth = 0;
        while let Some(node) = parsed.tree_node(node_id) {
            match node.kind {
                TypeTreeKind::Parenthesized(inner) => {
                    unwrap_depth += 1;
                    if unwrap_depth >= MAX_DEPTH {
                        return CheckOutcome::Unknown(UnknownReason::BudgetExceeded);
                    }
                    node_id = inner;
                }
                _ => break,
            }
        }
        if let Some(TypeTreeKind::Union { terms }) = parsed.tree_node(node_id).map(|n| &n.kind) {
            for &sub_id in parsed.type_node_list(terms.clone()) {
                match self.lower_union_term_list(
                    origin,
                    source,
                    parsed,
                    sub_id,
                    file_offset,
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
        let term = match self.lower_single_term(
            origin,
            source,
            parsed,
            node_id,
            file_offset,
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

    #[allow(clippy::too_many_arguments)]
    fn lower_single_term(
        &mut self,
        origin: &'a Path,
        source: &str,
        parsed: &TypeSyntax,
        mut node_id: TypeNodeId,
        file_offset: Option<usize>,
        environment: &[(&str, TypeId)],
        depth: usize,
    ) -> CheckOutcome<Term> {
        if depth >= MAX_DEPTH {
            return CheckOutcome::Unknown(UnknownReason::BudgetExceeded);
        }
        let mut approximate = false;
        let mut unwrap_depth = 0;
        while let Some(node) = parsed.tree_node(node_id) {
            match node.kind {
                TypeTreeKind::Parenthesized(inner) => {
                    unwrap_depth += 1;
                    if unwrap_depth >= MAX_DEPTH {
                        return CheckOutcome::Unknown(UnknownReason::BudgetExceeded);
                    }
                    node_id = inner;
                }
                TypeTreeKind::Approx(inner) if !approximate => {
                    approximate = true;
                    node_id = inner;
                }
                TypeTreeKind::Approx(_) => {
                    return CheckOutcome::Unknown(UnknownReason::SyntaxError);
                }
                _ => break,
            }
        }
        let Some(node) = parsed.tree_node(node_id) else {
            return CheckOutcome::Unknown(UnknownReason::SyntaxError);
        };
        let span = node.byte_range();
        let Some(term_str) = source.get(span.clone()) else {
            return CheckOutcome::Unknown(UnknownReason::SyntaxError);
        };
        let term_offset = file_offset.map(|base| base + span.start);
        if matches!(node.kind, TypeTreeKind::Name { .. })
            && matches!(term_str, "any" | "comparable")
            && self.universe_constraint(origin, term_str, environment) == Some(true)
        {
            if approximate || term_str == "comparable" {
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
        if let Some((_, param_ty)) = environment.iter().find(|(name, _)| *name == term_str)
            && matches!(self.node(*param_ty), Some(Ty::TypeParam(_)))
        {
            return CheckOutcome::Invalid(());
        }
        if let TypeTreeKind::Interface { ref members } = node.kind {
            if approximate {
                return CheckOutcome::Invalid(());
            }
            let iface = match self.lower_interface_members_checked(
                origin,
                source,
                parsed,
                members.clone(),
                file_offset,
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
        if is_iface {
            if approximate {
                return CheckOutcome::Invalid(());
            }
            return CheckOutcome::Ok(Term {
                ty,
                approximate: false,
            });
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

    pub(super) fn builtin_comparable_interface(&mut self, origin: &Path) -> Option<TypeId> {
        let origin = self.workspace.files.get_key_value(origin)?.0.as_path();
        self.store_interface_nodes(origin, Vec::new(), Vec::new(), true, true, false, false)
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
            self.store_interface_nodes(origin, methods, Vec::new(), false, false, false, !error)?;
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
        self.store_unified_interface_checked(origin, methods, Vec::new(), false, false, false)
    }

    pub(super) fn store_unified_interface_checked(
        &mut self,
        origin: &'a Path,
        methods: Vec<Method<'a>>,
        embeddeds: Vec<TypeId>,
        has_type_elements: bool,
        comparable: bool,
        implicit: bool,
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
            implicit,
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
