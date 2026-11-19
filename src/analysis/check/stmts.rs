//! Statement checking for [`RegionChecker`].

use std::collections::BTreeSet;
use std::ops::Range;

use crate::analysis::check::diagnostics::Code;
use crate::analysis::check::{ExprOutcome, FileSymbolLookup, LocalTypeDefEntry, RegionChecker};
use crate::analysis::obj::{DeclId, LocalId, ObjectRef, SymbolOrigin, VarKind};
use crate::analysis::scope::tree::{BindingInit, ScopeKind};
use crate::analysis::types::{LocalFormal, LocalTypeDefinition, Mode, Operand, TypeId};
use crate::syntax::{self, ExprId, ExprKind, StmtId, StmtKind};

enum TypeSwitchCaseItem {
    Nil,
    Type(TypeId),
    Invalid,
}

impl<'a, 'b> RegionChecker<'a, 'b> {
    pub(crate) fn check_stmt_list(&mut self, ids: &[StmtId]) {
        for &stmt_id in ids {
            // Later statements cannot introduce bindings visible at the cursor.
            // Scope construction still sees forward labels in this body.
            if self.cursor_offset.is_some_and(|cursor| {
                self.body
                    .statement(stmt_id)
                    .is_some_and(|stmt| self.body_base + stmt.span.start > cursor)
            }) {
                break;
            }
            let mut single_const = None;
            self.check_stmt_inner(stmt_id, &mut single_const);
        }
    }

    pub(crate) fn check_stmt(&mut self, id: StmtId) {
        let mut const_group = None;
        self.check_stmt_inner(id, &mut const_group);
    }

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
    }

    fn check_stmt_inner(&mut self, id: StmtId, const_group: &mut Option<ConstGroupState>) {
        self.arena.reset_steps();
        let tree = self.body;
        let Some(stmt) = tree.statement(id) else {
            return;
        };
        match &stmt.kind {
            StmtKind::Missing | StmtKind::RangeClause { .. } => {}
            StmtKind::Block { statements } => self.check_stmt_list(statements),
            StmtKind::DeclarationGroup { declarations } => {
                let mut group_const = None;
                for &decl in declarations {
                    self.check_stmt_inner(decl, &mut group_const);
                }
            }
            StmtKind::Expression { expression } => {
                let _ = self.check_expr_outcome(*expression, None);
            }
            StmtKind::Declaration {
                keyword,
                names,
                annotation,
                values,
            } => match self.text(keyword) {
                "type" => self.check_type_decl(id, names, annotation.as_ref()),
                "const" => self.check_const_decl(names, annotation.as_ref(), values, const_group),
                "var" => self.check_var_decl(names, annotation.as_ref(), values),
                _ => {}
            },
            StmtKind::Assignment {
                left,
                operator,
                right,
            } => {
                let op_text = self.text(operator);
                if op_text == ":=" {
                    self.check_short_var_decl(&stmt.span, left, right);
                } else {
                    let compound = (op_text != "=").then_some(operator);
                    self.check_assign(&stmt.span, left, compound, right);
                }
            }
            StmtKind::If {
                init,
                condition,
                then_block,
                otherwise,
            } => {
                if let Some(init) = init {
                    self.check_stmt(*init);
                }
                self.check_condition(*condition, "non-boolean condition in if statement");
                self.check_stmt(*then_block);
                if let Some(otherwise) = otherwise {
                    self.check_stmt(*otherwise);
                }
            }
            StmtKind::For {
                init,
                condition,
                post,
                body,
            } => {
                if let Some(init) = init {
                    self.check_stmt(*init);
                }
                if let Some(condition) = condition {
                    self.check_condition(*condition, "non-boolean condition in for statement");
                }
                if let Some(post) = post {
                    self.check_stmt(*post);
                    if let Some(post) = tree.statement(*post)
                        && let StmtKind::Assignment { operator, .. } = &post.kind
                        && self.text(operator) == ":="
                    {
                        self.compiler_error(
                            &post.span,
                            Code::InvalidPostDecl,
                            "cannot declare in post statement",
                        );
                        // A rejected declaration does not establish an unused variable.
                        self.region.uses_complete = false;
                    }
                }
                self.check_stmt(*body);
            }
            StmtKind::RangeFor { clause, body } => self.check_range_for(*clause, *body),
            StmtKind::Switch { init, tag, cases } => {
                if let Some(init) = init {
                    self.check_stmt(*init);
                }
                let expected = if let Some(tag) = tag {
                    self.check_expr(*tag, None).map(|op| op.ty)
                } else {
                    self.basic_bool()
                };
                for &case in cases {
                    if let Some(syntax::Statement {
                        kind:
                            StmtKind::Case {
                                values, statements, ..
                            },
                        ..
                    }) = tree.statement(case)
                    {
                        self.check_case(values, statements, expected);
                    }
                }
            }
            StmtKind::TypeSwitch { init, guard, cases } => {
                if let Some(init) = init {
                    self.check_stmt(*init);
                }
                self.check_type_switch(*guard, cases);
            }
            StmtKind::Select { cases } => self.check_stmt_list(cases),
            StmtKind::CommunicationCase {
                communication,
                statements,
                ..
            } => {
                if let Some(comm) = communication
                    && !self.check_comm_header(*comm)
                {
                    return;
                }
                self.check_stmt_list(statements);
            }
            StmtKind::Case {
                values, statements, ..
            } => self.check_case(values, statements, None),
            StmtKind::Return { values } => self.check_return_stmt(&stmt.span, values),
            StmtKind::Send { channel, value } => {
                let ch_op = self.check_expr(*channel, None);
                let elem_ty = ch_op.and_then(|op| self.arena.send_element(op.ty));
                let val_op = self.check_expr(*value, elem_ty);
                if let (Some(target), Some(val), Some(int_bits)) =
                    (elem_ty, val_op, self.int_bits())
                {
                    let _ = self.arena.assignable(val, target, int_bits);
                }
            }
            StmtKind::Increment { expression, .. } => {
                let _ = self.check_expr(*expression, None);
            }
            StmtKind::Go { call } | StmtKind::Defer { call } => {
                let _ = self.check_expr_outcome(*call, None);
            }
            StmtKind::Label { name, statement } => {
                let abs_span = self.abs_span(name);
                let obj = ObjectRef::Label {
                    file: self.file,
                    span: abs_span,
                };
                self.region.defs.push((abs_span, obj));
                if let Some(statement) = statement {
                    self.check_stmt(*statement);
                }
            }
            StmtKind::Branch { label, .. } => {
                if let Some(label) = label {
                    let name = self.text(label);
                    let span = self.abs_span(label);
                    if let Some(target_span) = self.region.scopes.resolve_label(self.source, name) {
                        self.region.uses.push((
                            span,
                            ObjectRef::Label {
                                file: self.file,
                                span: target_span,
                            },
                        ));
                    }
                }
            }
        }
    }

    fn check_case(&mut self, values: &[ExprId], statements: &[StmtId], expected: Option<TypeId>) {
        for &value in values {
            let _ = self.check_expr_outcome(value, expected);
        }
        self.check_stmt_list(statements);
    }

    fn check_condition(&mut self, condition: ExprId, message: &str) {
        let expected = self.basic_bool();
        let ExprOutcome::Value(operand) = self.check_expr_outcome(condition, expected) else {
            return;
        };
        if operand.mode.is_value()
            && self
                .arena
                .basic_kind(operand.ty)
                .is_some_and(|kind| !kind.boolean())
            && let Some(expr) = self.body.expression(condition)
        {
            self.compiler_error(&expr.span, Code::InvalidCond, message);
        }
    }

    fn check_type_switch(&mut self, guard: StmtId, cases: &[StmtId]) {
        let Some(int_bits) = self.int_bits() else {
            return;
        };

        let (guard_var_span, guard_operand_expr) = match self.body.statement(guard).map(|s| &s.kind)
        {
            Some(StmtKind::Assignment { left, right, .. }) => {
                let var_span = left
                    .first()
                    .and_then(|&id| self.body.expression(id))
                    .filter(|e| matches!(e.kind, ExprKind::Name))
                    .map(|e| e.span.clone());
                let rhs = right.first().copied();
                (var_span, rhs)
            }
            Some(StmtKind::Expression { expression }) => (None, Some(*expression)),
            _ => (None, None),
        };

        let unwrapped_guard = guard_operand_expr.map(|id| match self.body.expression(id) {
            Some(e) => match &e.kind {
                ExprKind::TypeSwitchGuard { operand } => *operand,
                _ => id,
            },
            None => id,
        });

        let guard_abs_span = unwrapped_guard
            .and_then(|id| self.body.expression(id))
            .map(|e| self.abs_span(&e.span));
        let source_op = unwrapped_guard.and_then(|expr_id| self.check_expr(expr_id, None));

        struct CaseClause {
            case_id: StmtId,
            span: Range<usize>,
            range: Range<usize>,
            statements: Vec<StmtId>,
        }

        let mut all_targets: Vec<Option<TypeId>> = Vec::new();
        let mut all_cases_valid = true;
        let mut clauses = Vec::with_capacity(cases.len());
        let mut default_count = 0usize;

        for &case_id in cases {
            let Some((case_span, values, statements, is_default)) =
                self.body.statement(case_id).and_then(|s| match &s.kind {
                    StmtKind::Case {
                        values,
                        statements,
                        default,
                        ..
                    } => Some((s.span.clone(), values.clone(), statements.clone(), *default)),
                    _ => None,
                })
            else {
                continue;
            };
            if is_default {
                default_count += 1;
            }
            let start = all_targets.len();
            for val_id in values {
                match self.resolve_type_switch_case_item(val_id) {
                    TypeSwitchCaseItem::Nil => all_targets.push(None),
                    TypeSwitchCaseItem::Type(ty) => {
                        all_targets.push(Some(ty));
                    }
                    TypeSwitchCaseItem::Invalid => {
                        all_cases_valid = false;
                        all_targets.push(None);
                    }
                }
            }
            let end = all_targets.len();
            clauses.push(CaseClause {
                case_id,
                span: case_span,
                range: start..end,
                statements,
            });
        }

        for clause in clauses {
            let clause_op = if all_cases_valid {
                source_op.and_then(|src| {
                    self.arena.type_switch(
                        src,
                        &all_targets,
                        clause.range.clone(),
                        default_count,
                        int_bits,
                    )
                })
            } else {
                None
            };

            if let Some(ref var_name_range) = guard_var_span {
                let var_name = self.text(var_name_range);
                let var_abs_span = self.abs_span(var_name_range);
                let case_abs_span = self.abs_span(&clause.span);
                if var_name != "_" {
                    let bound_id = self
                        .region
                        .scopes
                        .bindings
                        .iter()
                        .position(|b| {
                            matches!(
                                b.init,
                                BindingInit::TypeSwitch {
                                    guard_stmt,
                                    case_stmt,
                                } if guard_stmt == guard && case_stmt == clause.case_id
                            )
                        })
                        .map(|i| LocalId(i as u32));

                    if let Some(local_id) = bound_id {
                        self.mark_active(local_id);
                        let obj = ObjectRef::Var {
                            origin: SymbolOrigin::Local {
                                file: self.file,
                                id: local_id,
                                span: var_abs_span,
                            },
                            kind: VarKind::Local,
                        };
                        self.region.implicits.push((case_abs_span, obj));
                        if let Some(g_span) = guard_abs_span {
                            self.region.set_local_init(local_id, g_span);
                        }
                        if let Some(op) = clause_op {
                            self.region.set_local_type(
                                local_id,
                                Operand {
                                    mode: Mode::Variable,
                                    ty: op.ty,
                                    value: None,
                                },
                            );
                        }
                    }
                }
            }

            self.check_stmt_list(&clause.statements);
        }
    }

    fn resolve_type_switch_case_item(&mut self, expr_id: ExprId) -> TypeSwitchCaseItem {
        let unwrapped = self.unwrap_parens(expr_id);
        let expr_span = self
            .body
            .expression(unwrapped)
            .filter(|e| matches!(e.kind, ExprKind::Name))
            .map(|e| e.span.clone());
        if let Some(span) = expr_span
            && self.text(&span) == "nil"
        {
            let abs_start = self.body_base + span.start;
            let shadowed = self
                .region
                .scopes
                .resolve_at(self.source, abs_start as u32, "nil")
                .is_some_and(|id| self.is_active(id))
                || self.find_import("nil").is_some()
                || !matches!(
                    self.resolve_file_or_package_symbol("nil"),
                    FileSymbolLookup::NotFound
                );
            if !shadowed {
                self.region
                    .uses
                    .push((self.abs_span(&span), ObjectRef::Nil));
                return TypeSwitchCaseItem::Nil;
            }
        }
        match self.check_expr_outcome(expr_id, None) {
            ExprOutcome::Value(op)
                if op.mode == Mode::TypeExpr && self.arena.validate_type(op.ty).is_some() =>
            {
                TypeSwitchCaseItem::Type(op.ty)
            }
            _ => TypeSwitchCaseItem::Invalid,
        }
    }

    pub(crate) fn unwrap_parens(&self, mut id: ExprId) -> ExprId {
        while let Some(inner) = self.body.expression(id).and_then(|e| match &e.kind {
            ExprKind::Parenthesized { expression } => Some(*expression),
            _ => None,
        }) {
            id = inner;
        }
        id
    }

    fn check_type_decl(
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
                                self.region.defs.push((tp_span, obj));
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
                self.declaring_type = local_id;
                let _ = self.validate_local_type_rhs(ann_text, abs_ann_start);
                self.declaring_type = None;
            } else if !ann_text.is_empty() {
                if let Some(lid) = local_id {
                    self.local_type_defs.push(LocalTypeDefEntry {
                        local_id: lid,
                        name: name.to_string(),
                        selection: abs_name_range,
                        annotation: ann_text.to_string(),
                        annotation_start: abs_ann_start,
                        alias: is_alias,
                    });
                    self.declaring_type = Some(lid);
                }
                if self.validate_local_type_rhs(ann_text, abs_ann_start) {
                    let (visible_defs, extra_env) = self.visible_type_env(abs_ann_start);
                    let defs: Vec<LocalTypeDefinition<'_>> = visible_defs
                        .iter()
                        .map(|e| LocalTypeDefinition {
                            name: e.name.as_str(),
                            selection: self.file_offset(e.selection.start)
                                ..self.file_offset(e.selection.end),
                            annotation: e.annotation.as_str(),
                            annotation_start: self.file_offset(e.annotation_start),
                            alias: e.alias,
                        })
                        .collect();
                    let env_pairs: Vec<(&str, TypeId)> =
                        extra_env.iter().map(|(n, t)| (n.as_str(), *t)).collect();
                    resolved_ty = self
                        .arena
                        .declared_scope_type_in(self.file, name, None, &defs, &env_pairs)
                        .filter(|&ty| self.arena.validate_type(ty).is_some());
                }
                self.declaring_type = None;
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
            self.region.defs.push((abs_name_span, obj));
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
            self.type_formals = formals_owned.clone();
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
                self.region.defs.push((
                    span,
                    ObjectRef::TypeParam {
                        owner: self.type_param_owner,
                        index: idx as u16,
                        span,
                    },
                ));
                if all_constraints_valid
                    && let Some(tp_ty) = self
                        .arena
                        .parse_parameterized(self.file, name, None, &formals, None)
                {
                    self.type_param_env.push((name.to_string(), tp_ty));
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

    fn check_const_decl(
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
                    if converted.is_none() {
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
            self.region.defs.push((abs_span, obj));
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

    fn check_var_decl(
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
                        self.arena.default_operand(rhs_op, int_bits)
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

    fn is_call(&self, expression: ExprId) -> bool {
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

    fn check_short_var_decl(&mut self, span: &Range<usize>, lhs: &[ExprId], rhs: &[ExprId]) {
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
                    self.arena.default_operand(op, int_bits)
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
                        self.region.uses.push((abs_span, obj));
                    }
                    continue;
                }
                let existing = self.region.local_type(existing_id);
                self.region.writes.push(abs_span);
                if let (Some(target), Some(op)) = (existing, rhs_op) {
                    let _ = self.check_assignment(
                        op,
                        target.ty,
                        rhs[idx.min(rhs.len() - 1)],
                        "assignment",
                    );
                }
                if let Some(obj) = object {
                    self.region.uses.push((abs_span, obj));
                }
            } else {
                valid_lhs = false;
            }
        }
        if valid_lhs && !new_variable {
            self.compiler_error(span, Code::NoNewVar, "no new variables on left side of :=");
        }
    }

    fn check_assign(
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

    fn check_range_for(&mut self, clause_id: StmtId, body_id: StmtId) {
        let Some(int_bits) = self.int_bits() else {
            return;
        };

        let Some((lhs, define, iterable)) =
            self.body.statement(clause_id).and_then(|s| match &s.kind {
                StmtKind::RangeClause {
                    left,
                    operator,
                    value,
                } => {
                    let define = operator
                        .as_ref()
                        .is_some_and(|op_r| self.text(op_r) == ":=");
                    Some((left.clone(), define, *value))
                }
                _ => None,
            })
        else {
            self.check_stmt(body_id);
            return;
        };

        let iter_op = self.check_expr(iterable, None);
        if define && iter_op.is_none() {
            // The checker cannot establish which range bindings are valid
            // when the iteration operand is unavailable.
            self.region.uses_complete = false;
        }
        let count = lhs.len();

        for (idx, &lhs_id) in lhs.iter().enumerate() {
            let slot_ty = iter_op.and_then(|op| self.arena.range_slot(op, idx, count, int_bits));
            let name_span = self
                .body
                .expression(lhs_id)
                .filter(|e| matches!(e.kind, ExprKind::Name))
                .map(|e| e.span.clone());
            if name_span.as_ref().is_some_and(|sp| self.text(sp) == "_") {
                if define
                    && let Some(sp) = &name_span
                    && let Some(ty) = slot_ty
                {
                    self.region.expr_types.push((
                        self.abs_span(sp),
                        Operand {
                            mode: Mode::Variable,
                            ty,
                            value: None,
                        },
                    ));
                }
                continue;
            }
            if define && let Some(span) = name_span {
                let abs_span = self.abs_span(&span);
                let name = self.text(&span);
                if let Some(local_id) = self.region.scopes.definition_at(abs_span[0]) {
                    self.record_local_var_def(local_id, abs_span, VarKind::Local, slot_ty);
                } else if let Some(existing_id) =
                    self.region
                        .scopes
                        .resolve_at(self.source, abs_span[0], name)
                    && self.is_active(existing_id)
                    && let Some(obj) = self.local_object(existing_id)
                {
                    self.region.uses.push((abs_span, obj));
                    self.region.writes.push(abs_span);
                }
                continue;
            }
            self.record_write_only(lhs_id);
            if let Some(lhs_op) = self.check_expr(lhs_id, slot_ty)
                && let Some(s_ty) = slot_ty
            {
                let slot_op = Operand {
                    mode: Mode::Value,
                    ty: s_ty,
                    value: None,
                };
                let _ = self.arena.assignable(slot_op, lhs_op.ty, int_bits);
            }
        }

        self.check_stmt(body_id);
    }

    /// Selectors, indexes and dereferences read their receiver; only a bare
    /// (possibly parenthesized) name is write-only in an ordinary assignment.
    fn record_write_only(&mut self, id: ExprId) {
        if let Some(expr) = self.body.expression(self.unwrap_parens(id))
            && matches!(expr.kind, ExprKind::Name)
        {
            self.region.writes.push(self.abs_span(&expr.span));
        }
    }

    fn check_return_stmt(&mut self, span: &Range<usize>, results: &[ExprId]) {
        let expected_count = self.result_types.len();
        if results.is_empty() {
            if expected_count != 0 && !self.named_results {
                self.return_count(span, results, &[]);
            }
            return;
        }
        if let [expr] = results
            && (expected_count != 1 || self.is_call(*expr))
        {
            let expected = if expected_count == 1 {
                self.result_types[0]
            } else {
                None
            };
            if let Some(ops) = self.check_value_pack(*expr, false, expected) {
                if ops.len() == expected_count {
                    for (index, op) in ops.into_iter().enumerate() {
                        if let Some(target) = self.result_types[index] {
                            let _ = self.check_assignment(op, target, *expr, "return statement");
                        }
                    }
                } else {
                    self.return_count(span, results, &ops);
                }
            }
            return;
        }
        if results.len() != expected_count {
            let mut actual = Vec::with_capacity(results.len());
            for &expr in results {
                let operand = self.check_expr(expr, None).filter(|op| op.mode.is_value());
                actual.push(operand);
            }
            if let Some(actual) = actual.into_iter().collect::<Option<Vec<_>>>() {
                self.return_count(span, results, &actual);
            }
            return;
        }
        for (index, &expr) in results.iter().enumerate() {
            let expected = self.result_types[index];
            if let Some(op) = self.check_expr(expr, expected)
                && let Some(target) = expected
            {
                let _ = self.check_assignment(op, target, expr, "return statement");
            }
        }
    }

    fn check_comm_header(&mut self, comm: StmtId) -> bool {
        let tree = self.body;
        let Some(statement) = tree.statement(comm) else {
            return false;
        };
        let receive = match &statement.kind {
            StmtKind::Send { .. } => {
                self.check_stmt(comm);
                return true;
            }
            StmtKind::Expression { expression } => Some(*expression),
            StmtKind::Assignment { right, .. } if right.len() == 1 => Some(right[0]),
            _ => None,
        };
        if receive.is_some_and(|expr| tree.is_receive(expr, |span| self.text(span) == "<-")) {
            self.check_stmt(comm);
            return true;
        }
        self.compiler_error(
            &statement.span,
            Code::InvalidSelectCase,
            "select case must be send or receive (possibly with assignment)",
        );
        self.region.uses_complete = false;
        false
    }

    pub(crate) fn closure_scope_present(&self, abs_sig_start: u32, abs_body_end: u32) -> bool {
        self.region.scopes.nodes.iter().any(|n| {
            matches!(n.kind, ScopeKind::FuncSignature) && n.extent == [abs_sig_start, abs_body_end]
        })
    }
}

struct ConstGroupState {
    iota: u32,
    annotation: Option<Range<usize>>,
    values: Vec<ExprId>,
}
