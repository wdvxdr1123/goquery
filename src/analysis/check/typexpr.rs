//! Type-expression checking, reference recording, and AST lowering in [`RegionChecker`].

use std::ops::Range;

use crate::analysis::engine::check::region::RecordMask;
use crate::analysis::engine::check::{ExprOutcome, RegionChecker, collect_visible_type_env};
use crate::analysis::engine::obj::{MemberDeclId, MethodOrigin, ObjectRef};
use crate::analysis::engine::scope::tree::LocalBindingKind;
use crate::analysis::engine::types::{Mode, Operand, TypeArena, TypeId};
use crate::syntax::{self, BlockTree, ExprId, ExprKind, Signature, StmtId, TypeNodeId};

impl<'a, 'b> RegionChecker<'a, 'b> {
    pub(in crate::analysis::engine::check) fn sig_constraint_refs_valid(
        &mut self,
        sig_slice: &str,
        sig_base: usize,
        ann_range: &Range<usize>,
    ) -> bool {
        let Some(text) = sig_slice.get(ann_range.clone()) else {
            return false;
        };
        if text.trim().is_empty() {
            return false;
        }
        let parsed = syntax::parse_constraint(text);
        if !parsed.diagnostics.is_empty() {
            return false;
        }
        let abs_start = sig_base + ann_range.start;
        for tr in &parsed.references {
            let Some(name) = text.get(tr.name.clone()) else {
                return false;
            };
            if let Some(qual_r) = &tr.qualifier {
                let Some(qual) = text.get(qual_r.clone()) else {
                    return false;
                };
                if self.env.type_param_env.iter().any(|(n, _)| n == qual)
                    || self.env.type_formals.iter().any(|(n, _, _)| n == qual)
                {
                    return false;
                }
                if !self
                    .resolve_pkg_qualified_type_use(qual, name, abs_start, qual_r, &tr.name, true)
                {
                    return false;
                }
            } else {
                let name_span = [
                    (abs_start + tr.name.start) as u32,
                    (abs_start + tr.name.end) as u32,
                ];
                if self.record_formal_type_param_use(name, name_span)
                    || self.env.type_param_env.iter().any(|(n, _)| n == name)
                {
                    continue;
                }
                if !self.resolve_unqualified_type_use(name, abs_start, &tr.name, true, true) {
                    return false;
                }
            }
        }
        true
    }

    pub(crate) fn sig_all_refs_valid(
        &mut self,
        sig_slice: &str,
        sig_base: usize,
        sig: &Signature,
    ) -> bool {
        if !sig.diagnostics.is_empty() || !sig.type_parameters.is_empty() {
            return false;
        }
        for param in &sig.parameters {
            if param.annotation.is_empty()
                || !self.sig_type_refs_valid(sig_slice, sig_base, &param.annotation)
            {
                return false;
            }
        }
        for res in &sig.results {
            if res.annotation.is_empty()
                || !self.sig_type_refs_valid(sig_slice, sig_base, &res.annotation)
            {
                return false;
            }
        }
        true
    }

    pub(in crate::analysis::engine::check) fn sig_type_refs_valid(
        &mut self,
        sig_slice: &str,
        sig_base: usize,
        ann_range: &Range<usize>,
    ) -> bool {
        self.parse_and_check_sig_type(sig_slice, sig_base, ann_range)
            .is_some()
    }

    /// Parse a signature parameter/result type once, verify references and
    /// details, and return the parsed `TypeSyntax` + pre-evaluated constant
    /// array-length expressions for single-pass lowering without re-parsing.
    pub(in crate::analysis::engine::check) fn parse_and_check_sig_type(
        &mut self,
        sig_slice: &str,
        sig_base: usize,
        ann_range: &Range<usize>,
    ) -> Option<(syntax::TypeSyntax, Vec<(ExprId, Operand)>)> {
        let text = sig_slice.get(ann_range.clone())?;
        let mut parsed = syntax::parse_type(text);
        if !parsed.diagnostics.is_empty() {
            self.region.uses_complete = false;
            return None;
        }
        let abs_start = sig_base + ann_range.start;
        let ok = self.check_type_references(text, abs_start, &parsed);
        let const_exprs = self.check_type_annotation_details_inner(text, abs_start, &mut parsed);
        ok.then_some((parsed, const_exprs))
    }

    pub(crate) fn resolve_type_syntax_span(&mut self, span: &Range<usize>) -> Option<TypeId> {
        let text = self.text(span);
        let abs_start = self.body_base + span.start;
        self.resolve_type_text_in(text, abs_start)
    }

    pub(crate) fn resolve_type_text_in(&mut self, text: &str, abs_offset: usize) -> Option<TypeId> {
        if text.trim().is_empty() {
            return None;
        }
        let mut parsed = syntax::parse_type(text);
        if !parsed.diagnostics.is_empty() {
            self.region.uses_complete = false;
            return None;
        }

        let refs_ok = self.check_type_references(text, abs_offset, &parsed);
        let const_exprs = self.check_type_annotation_details_inner(text, abs_offset, &mut parsed);
        if !refs_ok {
            return None;
        }

        self.resolve_type_text_parsed(text, abs_offset, &parsed, &const_exprs)
    }

    pub(in crate::analysis::engine::check) fn lower_parsed_type_in_env(
        &mut self,
        text: &str,
        abs_offset: usize,
        parsed: &syntax::TypeSyntax,
        env_pairs: &[(&str, TypeId)],
        const_exprs: &[(ExprId, Operand)],
    ) -> Option<TypeId> {
        let file = self.file;
        let file_offset = Some(self.file_offset(abs_offset));
        let decls = &mut *self.decls;
        let mut const_eval = |arena: &mut TypeArena<'a>,
                              origin: &std::path::Path,
                              source: &str,
                              parsed: &syntax::TypeSyntax,
                              expr_id: ExprId| {
            const_exprs
                .iter()
                .find(|(id, _)| *id == expr_id)
                .map(|(_, op)| *op)
                .or_else(|| decls.eval_const_expr(arena, origin, source, parsed, expr_id))
        };
        let ty = self
            .arena
            .lower_type_syntax_with_eval(
                file,
                text,
                parsed,
                file_offset,
                env_pairs,
                &mut const_eval,
            )
            .ok()?;
        self.arena.validate_type(ty).map(|()| ty)
    }

    fn resolve_type_text_parsed(
        &mut self,
        text: &str,
        abs_offset: usize,
        parsed: &syntax::TypeSyntax,
        const_exprs: &[(ExprId, Operand)],
    ) -> Option<TypeId> {
        let env_pairs = collect_visible_type_env(self.region, &self.env, self.source, abs_offset);
        let file = self.file;
        let file_offset = Some(self.file_offset(abs_offset));
        let decls = &mut *self.decls;
        let mut const_eval = |arena: &mut TypeArena<'a>,
                              origin: &std::path::Path,
                              source: &str,
                              parsed: &syntax::TypeSyntax,
                              expr_id: ExprId| {
            const_exprs
                .iter()
                .find(|(id, _)| *id == expr_id)
                .map(|(_, op)| *op)
                .or_else(|| decls.eval_const_expr(arena, origin, source, parsed, expr_id))
        };
        let ty = self
            .arena
            .lower_type_syntax_with_eval(
                file,
                text,
                parsed,
                file_offset,
                &env_pairs,
                &mut const_eval,
            )
            .ok()?;
        self.arena.validate_type(ty).map(|()| ty)
    }

    fn lower_parsed_type_node(
        &mut self,
        text: &str,
        abs_offset: usize,
        parsed: &syntax::TypeSyntax,
        node_id: TypeNodeId,
        const_exprs: &[(ExprId, Operand)],
    ) -> Option<TypeId> {
        let env_pairs = collect_visible_type_env(self.region, &self.env, self.source, abs_offset);
        let file = self.file;
        let file_offset = Some(self.file_offset(abs_offset));
        let decls = &mut *self.decls;
        let mut const_eval = |arena: &mut TypeArena<'a>,
                              origin: &std::path::Path,
                              source: &str,
                              parsed: &syntax::TypeSyntax,
                              expr_id: ExprId| {
            const_exprs
                .iter()
                .find(|(id, _)| *id == expr_id)
                .map(|(_, op)| *op)
                .or_else(|| decls.eval_const_expr(arena, origin, source, parsed, expr_id))
        };
        let ty = self
            .arena
            .lower_type_node_with_eval(
                file,
                text,
                parsed,
                node_id,
                file_offset,
                &env_pairs,
                &mut const_eval,
            )
            .ok()?;
        self.arena.validate_type(ty).map(|()| ty)
    }

    pub(in crate::analysis::engine::check) fn check_type_references(
        &mut self,
        text: &str,
        abs_offset: usize,
        parsed: &syntax::TypeSyntax,
    ) -> bool {
        let mut ok = true;
        for tr in &parsed.references {
            let Some(name) = text.get(tr.name.clone()) else {
                ok = false;
                continue;
            };
            if let Some(qual_r) = &tr.qualifier {
                let Some(qual) = text.get(qual_r.clone()) else {
                    ok = false;
                    continue;
                };
                if self.env.type_param_env.iter().any(|(n, _)| n == qual)
                    || self.env.type_formals.iter().any(|(n, _, _)| n == qual)
                {
                    ok = false;
                    continue;
                }
                if !self
                    .resolve_pkg_qualified_type_use(qual, name, abs_offset, qual_r, &tr.name, true)
                {
                    ok = false;
                }
            } else {
                let allow_comparable = parsed.interface_members.iter().any(|m| {
                    m.name.is_none()
                        && m.annotation.start <= tr.name.start
                        && tr.name.end <= m.annotation.end
                });
                if !self.resolve_unqualified_type_use(
                    name,
                    abs_offset,
                    &tr.name,
                    true,
                    allow_comparable,
                ) {
                    ok = false;
                }
            }
        }
        ok
    }

    pub(in crate::analysis::engine::check) fn check_type_annotation_details(
        &mut self,
        text: &str,
        abs_offset: usize,
        parsed: &mut syntax::TypeSyntax,
    ) {
        let _ = self.check_type_annotation_details_inner(text, abs_offset, parsed);
    }

    fn check_type_annotation_details_inner(
        &mut self,
        text: &str,
        abs_offset: usize,
        parsed: &mut syntax::TypeSyntax,
    ) -> Vec<(ExprId, Operand)> {
        let mut const_exprs = Vec::new();
        let exprs = std::mem::take(&mut parsed.expressions);
        if !exprs.is_empty() {
            let mut is_child = vec![false; exprs.len()];
            let mut mark = |id: ExprId| {
                if let Some(slot) = is_child.get_mut(id.index()) {
                    *slot = true;
                }
            };
            for expr in &exprs {
                match &expr.kind {
                    ExprKind::Name
                    | ExprKind::Literal(_)
                    | ExprKind::Missing
                    | ExprKind::Type
                    | ExprKind::FunctionLiteral { .. } => {}
                    ExprKind::Unary { operand, .. }
                    | ExprKind::Parenthesized {
                        expression: operand,
                    }
                    | ExprKind::Selector { operand, .. }
                    | ExprKind::TypeAssertion { operand, .. }
                    | ExprKind::TypeSwitchGuard { operand } => mark(*operand),
                    ExprKind::Binary { left, right, .. }
                    | ExprKind::KeyValue {
                        key: left,
                        value: right,
                    } => {
                        mark(*left);
                        mark(*right);
                    }
                    ExprKind::Composite {
                        literal_type,
                        elements,
                    } => {
                        if let Some(lt) = literal_type {
                            mark(*lt);
                        }
                        for &el in elements {
                            mark(el);
                        }
                    }
                    ExprKind::Call {
                        function,
                        arguments,
                        ..
                    } => {
                        mark(*function);
                        for &arg in arguments {
                            mark(arg);
                        }
                    }
                    ExprKind::Index { operand, arguments } => {
                        mark(*operand);
                        for &arg in arguments {
                            mark(arg);
                        }
                    }
                    ExprKind::Slice {
                        operand,
                        low,
                        high,
                        max,
                        ..
                    } => {
                        mark(*operand);
                        if let Some(id) = low {
                            mark(*id);
                        }
                        if let Some(id) = high {
                            mark(*id);
                        }
                        if let Some(id) = max {
                            mark(*id);
                        }
                    }
                }
            }
            let expr_tree = BlockTree {
                root: StmtId(0),
                statements: Vec::new(),
                expressions: exprs,
                diagnostics: Vec::new(),
                type_references: Vec::new(),
                type_parameters: Vec::new(),
                delimiters: Vec::new(),
            };
            let roots: Vec<_> = is_child
                .iter()
                .enumerate()
                .filter_map(|(index, child)| (!child).then_some(ExprId(index)))
                .collect();
            self.region.scopes.include_type_expressions(
                self.source,
                abs_offset,
                &expr_tree,
                &roots,
                self.cursor_offset,
            );
            self.region.extend_binding_storage();
            let child_env = self.env.clone();
            let mut child = RegionChecker::with_env(
                self.arena,
                self.decls,
                self.file,
                self.source,
                abs_offset,
                &expr_tree,
                self.region,
                self.iota,
                self.cursor_offset,
                child_env,
            );
            for root in roots {
                if let ExprOutcome::Value(op) = child.check_expr_outcome(root, None)
                    && op.mode == Mode::Constant
                {
                    const_exprs.push((root, op));
                }
            }
            child.flush_untyped();
            drop(child);
            // Restore `parsed.expressions` so single-pass lowering can inspect
            // array-length `ExprId` nodes without re-parsing.
            parsed.expressions = expr_tree.expressions;
        }

        let owner_sym = self
            .region
            .owner
            .filter(|d| d.symbol.is_type())
            .map(|d| d.symbol);

        let has_tree = !parsed.tree_overflowed() && parsed.root().is_some();

        if has_tree {
            for field in parsed.field_nodes() {
                if field.embedded {
                    continue;
                }
                let Some(name_r) = &field.name else {
                    continue;
                };
                let name_range = name_r.start as usize..name_r.end as usize;
                if text.get(name_range.clone()) == Some("_") {
                    continue;
                }
                let abs_span = [
                    (abs_offset + name_range.start) as u32,
                    (abs_offset + name_range.end) as u32,
                ];
                self.record_def(
                    abs_span,
                    ObjectRef::Field(MemberDeclId {
                        file: self.file,
                        owner: owner_sym,
                        selection: abs_span,
                        embedded: false,
                        builtin: false,
                    }),
                );
                if self.record_mask.contains(RecordMask::TYPES)
                    && let Some(ty) = self.lower_parsed_type_node(
                        text,
                        abs_offset,
                        parsed,
                        field.ty,
                        &const_exprs,
                    )
                {
                    self.record_type_and_value(
                        abs_span,
                        Operand {
                            mode: Mode::Variable,
                            ty,
                            value: None,
                        },
                    );
                }
            }
        } else {
            for field in &parsed.fields {
                if field.embedded || text.get(field.name.clone()) == Some("_") {
                    continue;
                }
                let abs_span = [
                    (abs_offset + field.name.start) as u32,
                    (abs_offset + field.name.end) as u32,
                ];
                self.record_def(
                    abs_span,
                    ObjectRef::Field(MemberDeclId {
                        file: self.file,
                        owner: owner_sym,
                        selection: abs_span,
                        embedded: false,
                        builtin: false,
                    }),
                );
            }
        }

        for member in &parsed.interface_members {
            let Some(name_r) = &member.name else {
                continue;
            };
            if text.get(name_r.clone()) == Some("_") {
                continue;
            }
            let abs_span = [
                (abs_offset + name_r.start) as u32,
                (abs_offset + name_r.end) as u32,
            ];
            self.record_def(
                abs_span,
                ObjectRef::Method(MethodOrigin::Interface(MemberDeclId {
                    file: self.file,
                    owner: owner_sym,
                    selection: abs_span,
                    embedded: false,
                    builtin: false,
                })),
            );
        }

        if has_tree {
            for sig in parsed.sig_nodes() {
                for (params_range, kind) in [
                    (sig.params.clone(), LocalBindingKind::Param),
                    (sig.results.clone(), LocalBindingKind::NamedResult),
                ] {
                    for param in parsed.sig_params(params_range) {
                        let Some(name_r) = &param.name else {
                            continue;
                        };
                        let name_range = name_r.start as usize..name_r.end as usize;
                        if text.get(name_range.clone()) == Some("_") {
                            continue;
                        }
                        let abs_span = [
                            (abs_offset + name_range.start) as u32,
                            (abs_offset + name_range.end) as u32,
                        ];
                        let Some(local_id) = self.region.scopes.signature_name(abs_span, kind)
                        else {
                            self.region.uses_complete = false;
                            continue;
                        };
                        self.region.extend_binding_storage();
                        if let Some(object) = self.local_object(local_id) {
                            self.record_def(abs_span, object);
                        }
                        let ty = self
                            .lower_parsed_type_node(
                                text,
                                abs_offset,
                                parsed,
                                param.ty,
                                &const_exprs,
                            )
                            .and_then(|ty| {
                                if param.variadic {
                                    self.arena.slice_type(ty)
                                } else {
                                    Some(ty)
                                }
                            });
                        if let Some(ty) = ty {
                            self.region.set_local_type(
                                local_id,
                                Operand {
                                    mode: Mode::Variable,
                                    ty,
                                    value: None,
                                },
                            );
                            self.record_type_and_value(
                                abs_span,
                                Operand {
                                    mode: Mode::Variable,
                                    ty,
                                    value: None,
                                },
                            );
                        }
                    }
                }
            }
        } else {
            for sig_name in &parsed.signature_names {
                if text.get(sig_name.name.clone()) == Some("_") {
                    continue;
                }
                let abs_span = [
                    (abs_offset + sig_name.name.start) as u32,
                    (abs_offset + sig_name.name.end) as u32,
                ];
                let kind = match sig_name.kind {
                    syntax::SignatureNameKind::Parameter => LocalBindingKind::Param,
                    syntax::SignatureNameKind::Result => LocalBindingKind::NamedResult,
                };
                let Some(local_id) = self.region.scopes.signature_name(abs_span, kind) else {
                    self.region.uses_complete = false;
                    continue;
                };
                self.region.extend_binding_storage();
                if let Some(object) = self.local_object(local_id) {
                    self.record_def(abs_span, object);
                }
            }
        }
        const_exprs
    }
}
