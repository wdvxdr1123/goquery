//! Single-pass AST lowering of interface literals, constraints, unions, and terms.
use super::Method;
use crate::analysis::engine::types::lower::{ConstExprEval, default_const_eval};
use crate::analysis::engine::types::ty::Term;
use crate::analysis::engine::types::{
    CheckOutcome, MAX_BYTES, MAX_DEPTH, Ty, TypeArena, TypeId, UnknownReason, sets,
};
use crate::syntax::{InterfaceMemberNode, TypeNodeId, TypeSyntax, TypeTreeKind};
use std::collections::HashSet;
use std::ops::Range;
use std::path::Path;

impl<'a> TypeArena<'a> {
    /// Parse a type parameter constraint (bare term, union, `any`, `comparable`,
    /// named interface, or interface literal) into a single unified interface `TypeId`.
    pub(in crate::analysis::engine::types) fn parse_constraint_checked(
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
        if parsed.tree_overflowed() {
            return CheckOutcome::Unknown(UnknownReason::BudgetExceeded);
        }
        if !parsed.diagnostics.is_empty() {
            return CheckOutcome::Unknown(UnknownReason::SyntaxError);
        }
        let Some(root) = parsed.root() else {
            return CheckOutcome::Unknown(UnknownReason::BudgetExceeded);
        };
        let mut eval = default_const_eval;
        self.lower_constraint_node(
            origin,
            source,
            &parsed,
            root,
            file_offset,
            environment,
            &mut eval,
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
        const_eval: ConstExprEval<'_, 'a>,
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
        let Some(text) = source.get(span) else {
            return CheckOutcome::Unknown(UnknownReason::SyntaxError);
        };
        match node.kind {
            TypeTreeKind::Missing => CheckOutcome::Unknown(UnknownReason::SyntaxError),
            TypeTreeKind::Union { ref terms } => {
                let term_ids = parsed.type_node_list(terms.clone());
                if term_ids.len() > sets::MAX_TERMS {
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
                        const_eval,
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
                    const_eval,
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
                const_eval,
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
                let ty = match self.lower_type_tree_node(
                    origin,
                    source,
                    parsed,
                    node_id,
                    file_offset,
                    environment,
                    const_eval,
                    depth + 1,
                ) {
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
                        // Defer generic argument bound checking until Pass 2 once
                        // all sibling type-parameter bounds are attached (e.g.
                        // `[S ParamMap[K], K comparable]`).
                        let has_underlying = if self.named_info(unaliased).is_some() {
                            self.expand_origin_rhs(unaliased).is_some()
                        } else {
                            self.underlying(unaliased).is_some()
                        };
                        if !has_underlying {
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

    #[allow(clippy::too_many_arguments)]
    pub(in crate::analysis::engine::types) fn lower_interface_members_checked(
        &mut self,
        origin: &'a Path,
        source: &str,
        parsed: &TypeSyntax,
        members_range: Range<u32>,
        file_offset: Option<usize>,
        environment: &[(&str, TypeId)],
        const_eval: ConstExprEval<'_, 'a>,
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
                    let signature = match self.lower_sig_node_checked(
                        origin,
                        source,
                        parsed,
                        *sig,
                        file_offset,
                        environment,
                        const_eval,
                        depth + 1,
                    ) {
                        CheckOutcome::Ok(sig_ty) => sig_ty,
                        CheckOutcome::Invalid(()) => {
                            return CheckOutcome::Unknown(UnknownReason::Unsupported);
                        }
                        CheckOutcome::Unknown(reason) => return CheckOutcome::Unknown(reason),
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
                        const_eval,
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
        const_eval: ConstExprEval<'_, 'a>,
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
        let Some(text) = source.get(span) else {
            return CheckOutcome::Unknown(UnknownReason::SyntaxError);
        };
        match node.kind {
            TypeTreeKind::Missing => CheckOutcome::Unknown(UnknownReason::SyntaxError),
            TypeTreeKind::Union { ref terms } => {
                let term_ids = parsed.type_node_list(terms.clone());
                if term_ids.len() > sets::MAX_TERMS {
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
                        const_eval,
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
                    const_eval,
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
                let ty = match self.lower_type_tree_node(
                    origin,
                    source,
                    parsed,
                    node_id,
                    file_offset,
                    environment,
                    const_eval,
                    depth + 1,
                ) {
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
        const_eval: ConstExprEval<'_, 'a>,
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
                    const_eval,
                    depth + 1,
                    out,
                ) {
                    CheckOutcome::Ok(()) => {}
                    other => return other,
                }
                if out.len() > sets::MAX_TERMS {
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
            const_eval,
            depth + 1,
        ) {
            CheckOutcome::Ok(term) => term,
            CheckOutcome::Invalid(()) => return CheckOutcome::Invalid(()),
            CheckOutcome::Unknown(reason) => return CheckOutcome::Unknown(reason),
        };
        if out.len() >= sets::MAX_TERMS {
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
        const_eval: ConstExprEval<'_, 'a>,
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
        let Some(term_str) = source.get(span) else {
            return CheckOutcome::Unknown(UnknownReason::SyntaxError);
        };
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
                const_eval,
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
        let ty = match self.lower_type_tree_node(
            origin,
            source,
            parsed,
            node_id,
            file_offset,
            environment,
            const_eval,
            depth + 1,
        ) {
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
        let has_underlying = if self.named_info(unaliased).is_some() {
            self.expand_origin_rhs(unaliased).is_some()
        } else {
            self.underlying(unaliased).is_some()
        };
        if !has_underlying {
            return CheckOutcome::Unknown(UnknownReason::MissingMetadata);
        }
        CheckOutcome::Ok(Term {
            ty: unaliased,
            approximate,
        })
    }
}
