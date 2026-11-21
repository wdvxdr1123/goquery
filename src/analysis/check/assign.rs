//! Declaration, assignment, short-variable, and range-clause checking.

use std::collections::BTreeSet;
use std::ops::Range;

use crate::analysis::engine::check::diagnostics::Code;
use crate::analysis::engine::check::{RegionChecker, collect_visible_type_env};
use crate::analysis::engine::obj::{DeclId, ObjectRef, SymbolOrigin, VarKind};
use crate::analysis::engine::types::{LocalFormal, Mode, Operand, TypeId};
use crate::syntax::{self, ExprId, ExprKind, StmtId, StmtKind};

pub(super) struct ConstGroupState {
    iota: u32,
    annotation: Option<Range<usize>>,
    values: Vec<ExprId>,
}

impl<'a, 'b> RegionChecker<'a, 'b> {
    pub(crate) fn check_decl_spec(&mut self, is_const: bool) {
        let tree = self.body;
        let Some(syntax::Statement {
            kind:
                StmtKind::Declaration {
                    names,
                    annotation,
                    values,
                    ..
                },
            ..
        }) = tree.statement(tree.root())
        else {
            return;
        };
        if is_const {
            self.check_const_decl(names, annotation.as_ref(), values, &mut None);
        } else {
            self.check_var_decl(names, annotation.as_ref(), values);
        }
        self.flush_untyped();
    }

    pub(super) fn check_type_decl(
        &mut self,
        stmt_id: StmtId,
        names: &[Range<usize>],
        annotation: Option<&Range<usize>>,
    ) {
        let Some(name_range) = names.first() else {
            return;
        };
        let abs_name_span = self.abs_span(name_range);
        let abs_name_range = self.abs_range(name_range);
        let name = self.text(name_range);
        if name == "_" {
            if let Some(ann) = annotation {
                let _ = self.resolve_type_syntax_span(ann);
            }
            return;
        }
        let local_id = self.region.scopes.definition_at(abs_name_span[0]);
        let body = self.body;
        let is_alias = body.declaration_equals(stmt_id).is_some();
        let type_parameters = body
            .type_parameters()
            .iter()
            .find(|tp| tp.declaration == stmt_id);
        let is_generic_local = type_parameters.is_some();

        if let Some(type_parameters) = type_parameters {
            for parameter in &type_parameters.parameters {
                if let Some(name_r) = &parameter.name {
                    let tp_name = self.text(name_r);
                    if !tp_name.is_empty() && tp_name != "_" {
                        let tp_span = self.abs_span(name_r);
                        if let Some(tp_lid) = self.region.scopes.definition_at(tp_span[0]) {
                            self.mark_active(tp_lid);
                            if let Some(obj) = self.local_object(tp_lid) {
                                self.record_def(tp_span, obj);
                            }
                        }
                    }
                }
                if !parameter.annotation.is_empty() {
                    let _ = self.sig_constraint_refs_valid(
                        self.source,
                        0,
                        &self.abs_range(&parameter.annotation),
                    );
                }
            }
        }

        let mut resolved_ty = None;
        if let Some(ann_r) = annotation {
            let raw_ann = self.text(ann_r);
            let ann_text = raw_ann.trim();
            let lead = raw_ann.len() - raw_ann.trim_start().len();
            let abs_ann_start = self.body_base + ann_r.start + lead;
            if !ann_text.is_empty() && is_generic_local {
                self.env.declaring_type = local_id.map(|lid| (lid, None));
                let _ = self.validate_local_type_rhs(ann_text, abs_ann_start);
                self.env.declaring_type = None;
            } else if !ann_text.is_empty() {
                let file_sel =
                    self.file_offset(abs_name_range.start)..self.file_offset(abs_name_range.end);
                let file_ann = self.file_offset(abs_ann_start)
                    ..self.file_offset(abs_ann_start + ann_text.len());
                let local_ty = self
                    .arena
                    .register_local_type(self.file, name, file_sel, ann_text, file_ann, is_alias);
                if let Some(lid) = local_id {
                    self.env.declaring_type = Some((lid, local_ty));
                }
                let refs_ok = self.validate_local_type_rhs(ann_text, abs_ann_start);
                if let Some(ty) = local_ty {
                    if refs_ok {
                        let env_pairs = collect_visible_type_env(
                            self.region,
                            &self.env,
                            self.source,
                            abs_ann_start,
                        );
                        let rhs = self.arena.parse_in_at(
                            self.file,
                            ann_text,
                            self.file_offset(abs_ann_start),
                            &env_pairs,
                        );
                        self.arena.set_local_type_rhs(ty, rhs);
                        if rhs.is_some() && self.arena.validate_type(ty).is_some() {
                            resolved_ty = Some(ty);
                        } else {
                            self.arena.set_local_type_rhs(ty, None);
                        }
                    } else {
                        self.arena.set_local_type_rhs(ty, None);
                    }
                }
                self.env.declaring_type = None;
            }
        }

        if let Some(lid) = local_id {
            self.mark_active(lid);
            let obj = ObjectRef::TypeName {
                origin: SymbolOrigin::Local {
                    file: self.file,
                    id: lid,
                    span: abs_name_span,
                },
                is_alias,
            };
            self.record_def(abs_name_span, obj);
            if let Some(ty) = resolved_ty {
                self.region.set_local_type(
                    lid,
                    Operand {
                        mode: Mode::TypeExpr,
                        ty,
                        value: None,
                    },
                );
            }
        }
    }

    pub(crate) fn check_top_level_type_spec(&mut self, sym: &'a crate::index::IndexedSymbol) {
        self.set_type_param_owner(DeclId {
            file: self.file,
            symbol: sym,
        });
        let annotation = self
            .body
            .statement(self.body.root())
            .and_then(|s| match &s.kind {
                StmtKind::Declaration { annotation, .. } => annotation.clone(),
                _ => None,
            });
        if let Some(params) = self.body.type_parameters().first().map(|tp| {
            tp.parameters
                .iter()
                .map(|p| (p.name.clone(), p.annotation.clone()))
                .collect::<Vec<_>>()
        }) {
            let mut formals_owned: Vec<(String, usize, Option<String>)> =
                Vec::with_capacity(params.len());
            for (name_range, ann_range) in &params {
                let Some(name_range) = name_range else {
                    continue;
                };
                let name = self.text(name_range);
                if name.is_empty() || name == "_" {
                    continue;
                }
                let bound = (!ann_range.is_empty()).then(|| self.text(ann_range).to_string());
                formals_owned.push((name.to_string(), self.body_base + name_range.start, bound));
            }
            self.env.type_formals = formals_owned.clone();
            let formals: Vec<LocalFormal<'_>> = formals_owned
                .iter()
                .map(|(name, selection, bound)| LocalFormal {
                    name: name.as_str(),
                    selection: *selection,
                    bound: bound.as_deref(),
                })
                .collect();

            let mut all_constraints_valid = self.body.diagnostics().is_empty();
            for (name_range, ann_range) in &params {
                let is_blank = name_range.as_ref().is_none_or(|nr| {
                    let n = self.text(nr);
                    n.is_empty() || n == "_"
                });
                if is_blank {
                    continue;
                }
                if ann_range.is_empty()
                    || !self.sig_constraint_refs_valid(self.source, 0, &self.abs_range(ann_range))
                {
                    all_constraints_valid = false;
                }
            }

            for (idx, (name_range, _)) in params.iter().enumerate() {
                let Some(name_range) = name_range else {
                    continue;
                };
                let name = self.text(name_range);
                if name.is_empty() || name == "_" {
                    continue;
                }
                let span = self.abs_span(name_range);
                self.record_def(
                    span,
                    ObjectRef::TypeParam {
                        owner: self.env.type_param_owner,
                        index: idx as u16,
                        span,
                    },
                );
                if all_constraints_valid
                    && let Some(tp_ty) = self
                        .arena
                        .parse_parameterized(self.file, name, None, &formals, None)
                {
                    self.env.type_param_env.push((name.to_string(), tp_ty));
                }
            }
        }
        if let Some(ann_r) = annotation {
            let ann_text = self.source.get(self.abs_range(&ann_r)).unwrap_or("");
            let abs_ann_start = self.body_base + ann_r.start;
            let mut parsed = syntax::parse_type(ann_text);
            if parsed.diagnostics.is_empty() {
                let _ = self.check_type_references(ann_text, abs_ann_start, &parsed);
                self.check_type_annotation_details(ann_text, abs_ann_start, &mut parsed);
            }
        }
        self.flush_untyped();
    }

    fn validate_local_type_rhs(&mut self, text: &str, abs_offset: usize) -> bool {
        let mut parsed = syntax::parse_type(text);
        if !parsed.diagnostics.is_empty() {
            self.region.uses_complete = false;
            return false;
        }
        let ok = self.check_type_references(text, abs_offset, &parsed);
        self.check_type_annotation_details(text, abs_offset, &mut parsed);
        ok
    }

    pub(super) fn check_const_decl(
        &mut self,
        names: &[Range<usize>],
        annotation: Option<&Range<usize>>,
        values: &[ExprId],
        const_group: &mut Option<ConstGroupState>,
    ) {
        let Some(int_bits) = self.int_bits() else {
            return;
        };

        if !values.is_empty() && names.len() != values.len() {
            self.declaration_count(names, values);
        }
        let prev_state = const_group.take();
        let (iota_val, prev_ann, effective_values): (u32, Option<Range<usize>>, Vec<ExprId>) =
            if let Some(prev) = prev_state {
                let next_iota = prev.iota + 1;
                if values.is_empty() {
                    (next_iota, prev.annotation.clone(), prev.values.clone())
                } else {
                    (next_iota, annotation.cloned(), values.to_vec())
                }
            } else {
                (0, annotation.cloned(), values.to_vec())
            };

        let ann_invalid = prev_ann.is_some();
        let ann_ty = prev_ann
            .as_ref()
            .and_then(|r| self.resolve_type_syntax_span(r));

        let saved_iota = self.iota;
        self.iota = Some(iota_val);

        let mut rhs_ops = Vec::with_capacity(effective_values.len());
        for &val_id in &effective_values {
            let op = self.check_expr(val_id, ann_ty);
            rhs_ops.push(op);
        }
        self.iota = saved_iota;

        let arity_ok = names.len() == rhs_ops.len();

        for (idx, name_range) in names.iter().enumerate() {
            let abs_span = self.abs_span(name_range);
            let rhs_op = if arity_ok {
                rhs_ops.get(idx).and_then(|o| *o)
            } else {
                None
            };

            let final_op = if ann_invalid && ann_ty.is_none() {
                None
            } else if let Some(op) = rhs_op {
                if op.mode != Mode::Constant {
                    None
                } else if let Some(target) = ann_ty {
                    let converted = self.arena.assign_basic(op, target, int_bits);
                    if converted.is_some() {
                        self.update_expr_type(effective_values[idx], target, int_bits);
                    } else {
                        self.check_assignment(
                            op,
                            target,
                            effective_values[idx],
                            "constant declaration",
                        );
                    }
                    converted
                } else {
                    Some(op)
                }
            } else {
                None
            };

            if self.text(name_range) == "_" {
                continue;
            }
            let Some(local_id) = self.region.scopes.definition_at(abs_span[0]) else {
                continue;
            };

            self.mark_active(local_id);
            let obj = ObjectRef::Const(SymbolOrigin::Local {
                file: self.file,
                id: local_id,
                span: abs_span,
            });
            self.record_def(abs_span, obj);
            if let Some(op) = final_op {
                self.region.set_local_type(local_id, op);
                if arity_ok && let Some(expr) = self.body.expression(effective_values[idx]) {
                    self.region
                        .set_local_init(local_id, self.abs_span(&expr.span));
                }
            }
        }

        *const_group = Some(ConstGroupState {
            iota: iota_val,
            annotation: prev_ann,
            values: effective_values,
        });
    }

    pub(super) fn check_var_decl(
        &mut self,
        names: &[Range<usize>],
        annotation: Option<&Range<usize>>,
        values: &[ExprId],
    ) {
        let Some(int_bits) = self.int_bits() else {
            return;
        };

        let has_ann = annotation.is_some();
        let ann_ty = annotation.and_then(|r| self.resolve_type_syntax_span(r));
        let ann_valid = !has_ann || ann_ty.is_some();

        let declared_mismatch =
            values.len() > names.len() || (values.len() > 1 && values.len() < names.len());
        if declared_mismatch {
            self.declaration_count(names, values);
        }
        let resolved_ops = self.check_initializers(names.len(), values, ann_ty);
        if !declared_mismatch {
            self.check_initializer_count(names.len(), values, &resolved_ops);
        }
        let arity_matches = resolved_ops.len() == names.len();
        let is_one_to_one = arity_matches && values.len() == names.len();

        for (idx, name_range) in names.iter().enumerate() {
            let abs_span = self.abs_span(name_range);
            let name = self.text(name_range);
            if name == "_" {
                continue;
            }
            let Some(local_id) = self.region.scopes.definition_at(abs_span[0]) else {
                continue;
            };
            let input = arity_matches
                .then(|| resolved_ops.get(idx).copied().flatten())
                .flatten();
            let var_ty = if !ann_valid {
                None
            } else if values.is_empty() {
                ann_ty
            } else if let Some(target) = ann_ty {
                if let Some(rhs_op) = input {
                    let _ = self.check_assignment(
                        rhs_op,
                        target,
                        values[idx.min(values.len() - 1)],
                        "variable declaration",
                    );
                }
                // The declaration's explicit type remains known even when its
                // initializer is incomplete or has an assignment error.
                Some(target)
            } else {
                input.and_then(|rhs_op| {
                    if rhs_op.mode == Mode::TypeExpr {
                        None
                    } else {
                        let defaulted = self.arena.default_operand(rhs_op, int_bits);
                        if let (Some(ty), true) = (defaulted, is_one_to_one) {
                            self.update_expr_type(values[idx], ty, int_bits);
                        }
                        defaulted
                    }
                })
            };

            self.record_local_var_def(local_id, abs_span, VarKind::Local, var_ty);
            if is_one_to_one
                && var_ty.is_some()
                && let Some(expr) = self.body.expression(values[idx])
            {
                self.region
                    .set_local_init(local_id, self.abs_span(&expr.span));
            }
        }
    }

    pub(super) fn is_call(&self, expression: ExprId) -> bool {
        self.body
            .expression(self.unwrap_parens(expression))
            .is_some_and(|expression| matches!(expression.kind, ExprKind::Call { .. }))
    }

    fn check_initializers(
        &mut self,
        count: usize,
        values: &[ExprId],
        expected: Option<TypeId>,
    ) -> Vec<Option<Operand>> {
        if let [value] = values
            && (count != 1 || self.is_call(*value))
        {
            return self
                .check_value_pack(*value, count == 2, expected)
                .unwrap_or_default()
                .into_iter()
                .map(Some)
                .collect();
        }
        let expected = (count == values.len()).then_some(expected).flatten();
        values
            .iter()
            .map(|&value| self.check_expr(value, expected))
            .collect()
    }

    fn check_initializer_count(
        &mut self,
        variables: usize,
        expressions: &[ExprId],
        values: &[Option<Operand>],
    ) {
        if !values.is_empty()
            && variables != values.len()
            && values
                .iter()
                .all(|value| value.is_some_and(|operand| operand.mode.is_value()))
        {
            self.assignment_count(expressions, variables, values.len());
        }
    }

    pub(super) fn check_short_var_decl(
        &mut self,
        span: &Range<usize>,
        lhs: &[ExprId],
        rhs: &[ExprId],
    ) {
        let Some(int_bits) = self.int_bits() else {
            return;
        };

        let rhs_ops = self.check_initializers(lhs.len(), rhs, None);
        self.check_initializer_count(lhs.len(), rhs, &rhs_ops);
        let arity_matches = lhs.len() == rhs_ops.len();
        let is_one_to_one = arity_matches && lhs.len() == rhs.len();
        let mut names = BTreeSet::new();
        let mut new_variable = false;
        let mut valid_lhs = self.region.scopes.complete && self.cursor_offset.is_none();

        for (idx, &lhs_id) in lhs.iter().enumerate() {
            let lhs_span = self
                .body
                .expression(lhs_id)
                .filter(|e| matches!(e.kind, ExprKind::Name))
                .map(|e| e.span.clone());
            let Some(lhs_span) = lhs_span else {
                valid_lhs = false;
                if let Some(span) = self.body.expression(lhs_id).map(|expr| expr.span.clone()) {
                    let text = self.text(&span);
                    if text.len() <= 1024 {
                        self.compiler_error(
                            &span,
                            Code::BadDecl,
                            &format!("non-name {text} on left side of :="),
                        );
                    }
                }
                self.record_write_only(lhs_id);
                let _ = self.check_expr_outcome(lhs_id, None);
                continue;
            };
            let name = self.text(&lhs_span);
            if name == "_" {
                continue;
            }
            if lhs.len() > 1 && !names.insert(name) {
                valid_lhs = false;
                if name.len() <= 1024 {
                    self.compiler_error(
                        &lhs_span,
                        Code::RepeatedDecl,
                        &format!("{name} repeated on left side of :="),
                    );
                }
                continue;
            }
            let abs_span = self.abs_span(&lhs_span);
            let rhs_op = if arity_matches {
                rhs_ops.get(idx).copied().flatten()
            } else {
                None
            };
            let inferred_ty = rhs_op.and_then(|op| {
                if op.mode == Mode::TypeExpr {
                    None
                } else {
                    let defaulted = self.arena.default_operand(op, int_bits);
                    if let (Some(ty), true) = (defaulted, is_one_to_one) {
                        self.update_expr_type(rhs[idx], ty, int_bits);
                    }
                    defaulted
                }
            });

            if let Some(local_id) = self.region.scopes.definition_at(abs_span[0]) {
                new_variable = true;
                self.record_local_var_def(local_id, abs_span, VarKind::Local, inferred_ty);
                if is_one_to_one
                    && inferred_ty.is_some()
                    && let Some(expr) = self.body.expression(rhs[idx])
                {
                    self.region
                        .set_local_init(local_id, self.abs_span(&expr.span));
                }
            } else if let Some(existing_id) =
                self.region
                    .scopes
                    .resolve_at(self.source, abs_span[0], name)
                && self.is_active(existing_id)
            {
                let object = self.local_object(existing_id);
                if !matches!(object, Some(ObjectRef::Var { .. })) {
                    valid_lhs = false;
                    if name.len() <= 1024 {
                        self.compiler_error(
                            &lhs_span,
                            Code::UnassignableOperand,
                            &format!("cannot assign to {name}"),
                        );
                    }
                    if let Some(obj) = object {
                        self.record_use(abs_span, obj);
                    }
                    continue;
                }
                let existing = self.region.local_type(existing_id);
                self.record_write(abs_span);
                if let (Some(target), Some(op)) = (existing, rhs_op) {
                    let _ = self.check_assignment(
                        op,
                        target.ty,
                        rhs[idx.min(rhs.len() - 1)],
                        "assignment",
                    );
                }
                if let Some(obj) = object {
                    self.record_use(abs_span, obj);
                }
            } else {
                valid_lhs = false;
            }
        }
        if valid_lhs && !new_variable {
            self.compiler_error(span, Code::NoNewVar, "no new variables on left side of :=");
        }
    }

    pub(super) fn check_assign(
        &mut self,
        span: &Range<usize>,
        lhs: &[ExprId],
        op_range: Option<&Range<usize>>,
        rhs: &[ExprId],
    ) {
        if let Some(operator) = op_range
            && (lhs.len() != 1 || rhs.len() != 1)
        {
            let op = self.text(operator);
            self.compiler_error(
                span,
                Code::MultiValAssignOp,
                &format!("assignment operation {op} requires single-valued expressions"),
            );
            self.region.uses_complete = false;
            return;
        }
        let Some(int_bits) = self.int_bits() else {
            return;
        };

        let mut lhs_ops = Vec::with_capacity(lhs.len());
        let mut valid_lhs = true;
        for &l_id in lhs {
            let is_blank = self
                .body
                .expression(l_id)
                .filter(|e| matches!(e.kind, ExprKind::Name))
                .is_some_and(|e| self.text(&e.span) == "_");
            if is_blank {
                lhs_ops.push(None);
            } else {
                if op_range.is_none() {
                    self.record_write_only(l_id);
                }
                let input = self.check_expr(l_id, None);
                valid_lhs &= input.is_some_and(|op| op.mode.is_value());
                lhs_ops.push(input);
            }
        }

        if let [value] = rhs
            && op_range.is_none()
            && (lhs.len() != 1 || self.is_call(*value))
        {
            let expected = if lhs.len() == 1 {
                lhs_ops[0].map(|operand| operand.ty)
            } else {
                None
            };
            if let Some(rhs_ops) = self.check_value_pack(*value, lhs.len() == 2, expected) {
                if rhs_ops.len() != lhs.len() {
                    self.assignment_count(rhs, lhs.len(), rhs_ops.len());
                } else {
                    for (l_op, r_op) in lhs_ops.iter().zip(rhs_ops) {
                        if let Some(l) = l_op {
                            let _ = self.check_assignment(r_op, l.ty, *value, "assignment");
                        }
                    }
                }
            }
            return;
        }

        if lhs.len() == rhs.len() {
            for ((l_op, &l_id), &r_id) in lhs_ops.iter().zip(lhs).zip(rhs.iter()) {
                let expected = l_op.map(|l| l.ty);
                if let Some(r_op) = self.check_expr(r_id, expected) {
                    if let (Some(op_r), Some(l)) = (op_range, l_op) {
                        let bin_op = self.text(op_r).trim_end_matches('=');
                        if let Some(Err(error)) =
                            self.arena
                                .checked_binary_expr(bin_op, *l, r_op, Some(l.ty), int_bits)
                        {
                            self.binary_error(op_r, (l_id, *l), (r_id, r_op), error);
                        }
                    } else if let Some(l) = l_op {
                        let _ = self.check_assignment(r_op, l.ty, r_id, "assignment");
                    }
                }
            }
        } else {
            let mut valid_rhs = true;
            for &r_id in rhs {
                valid_rhs &= self
                    .check_expr(r_id, None)
                    .is_some_and(|op| op.mode.is_value());
            }
            if valid_lhs && valid_rhs {
                self.assignment_count(rhs, lhs.len(), rhs.len());
            }
        }
    }
}
