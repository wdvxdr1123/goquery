//! Statement checking for [`RegionChecker`].

use std::ops::Range;

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
            let mut single_const = None;
            self.check_stmt_inner(stmt_id, &mut single_const);
        }
    }

    pub(crate) fn check_stmt(&mut self, id: StmtId) {
        let mut const_group = None;
        self.check_stmt_inner(id, &mut const_group);
    }

    pub(crate) fn check_decl_spec(&mut self, is_const: bool) {
        let root = self.body.root();
        let Some((names, annotation, values)) =
            self.body.statement(root).and_then(|s| match &s.kind {
                StmtKind::Declaration {
                    names,
                    annotation,
                    values,
                    ..
                } => Some((names.clone(), annotation.clone(), values.clone())),
                _ => None,
            })
        else {
            return;
        };
        if is_const {
            let mut const_group = None;
            self.check_const_decl(&names, annotation.as_ref(), &values, &mut const_group);
        } else {
            self.check_var_decl(&names, annotation.as_ref(), &values);
        }
    }

    fn check_stmt_inner(&mut self, id: StmtId, const_group: &mut Option<ConstGroupState>) {
        self.arena.reset_steps();
        enum Action {
            Block(Vec<StmtId>),
            DeclarationGroup(Vec<StmtId>),
            Expression(ExprId),
            Decl {
                span: Range<usize>,
                keyword: Range<usize>,
                names: Vec<Range<usize>>,
                annotation: Option<Range<usize>>,
                values: Vec<ExprId>,
            },
            Assign {
                left: Vec<ExprId>,
                operator: Range<usize>,
                right: Vec<ExprId>,
            },
            If {
                init: Option<StmtId>,
                condition: ExprId,
                then_block: StmtId,
                otherwise: Option<StmtId>,
            },
            For {
                init: Option<StmtId>,
                condition: Option<ExprId>,
                post: Option<StmtId>,
                body: StmtId,
            },
            RangeFor {
                clause: StmtId,
                body: StmtId,
            },
            Switch {
                init: Option<StmtId>,
                tag: Option<ExprId>,
                cases: Vec<StmtId>,
            },
            TypeSwitch {
                init: Option<StmtId>,
                guard: StmtId,
                cases: Vec<StmtId>,
            },
            Select {
                cases: Vec<StmtId>,
            },
            CommCase {
                communication: Option<StmtId>,
                statements: Vec<StmtId>,
            },
            Return {
                values: Vec<ExprId>,
            },
            Send {
                channel: ExprId,
                value: ExprId,
            },
            Increment {
                expression: ExprId,
            },
            DeferGo {
                call: ExprId,
            },
            Label {
                name: Range<usize>,
                statement: Option<StmtId>,
            },
            Branch {
                label: Option<Range<usize>>,
            },
            CaseFallback {
                values: Vec<ExprId>,
                statements: Vec<StmtId>,
            },
            Missing,
        }

        let Some(stmt) = self.body.statement(id) else {
            return;
        };
        let stmt_span = stmt.span.clone();
        let action = match &stmt.kind {
            StmtKind::Block { statements } => Action::Block(statements.clone()),
            StmtKind::DeclarationGroup { declarations } => {
                Action::DeclarationGroup(declarations.clone())
            }
            StmtKind::Expression { expression } => Action::Expression(*expression),
            StmtKind::Declaration {
                keyword,
                names,
                annotation,
                values,
            } => Action::Decl {
                span: stmt_span,
                keyword: keyword.clone(),
                names: names.clone(),
                annotation: annotation.clone(),
                values: values.clone(),
            },
            StmtKind::Assignment {
                left,
                operator,
                right,
            } => Action::Assign {
                left: left.clone(),
                operator: operator.clone(),
                right: right.clone(),
            },
            StmtKind::If {
                init,
                condition,
                then_block,
                otherwise,
            } => Action::If {
                init: *init,
                condition: *condition,
                then_block: *then_block,
                otherwise: *otherwise,
            },
            StmtKind::For {
                init,
                condition,
                post,
                body,
            } => Action::For {
                init: *init,
                condition: *condition,
                post: *post,
                body: *body,
            },
            StmtKind::RangeFor { clause, body } => Action::RangeFor {
                clause: *clause,
                body: *body,
            },
            StmtKind::RangeClause { .. } => Action::Missing,
            StmtKind::Switch { init, tag, cases } => Action::Switch {
                init: *init,
                tag: *tag,
                cases: cases.clone(),
            },
            StmtKind::TypeSwitch { init, guard, cases } => Action::TypeSwitch {
                init: *init,
                guard: *guard,
                cases: cases.clone(),
            },
            StmtKind::Select { cases } => Action::Select {
                cases: cases.clone(),
            },
            StmtKind::CommunicationCase {
                communication,
                statements,
            } => Action::CommCase {
                communication: *communication,
                statements: statements.clone(),
            },
            StmtKind::Case {
                values, statements, ..
            } => Action::CaseFallback {
                values: values.clone(),
                statements: statements.clone(),
            },
            StmtKind::Return { values } => Action::Return {
                values: values.clone(),
            },
            StmtKind::Send { channel, value } => Action::Send {
                channel: *channel,
                value: *value,
            },
            StmtKind::Increment { expression, .. } => Action::Increment {
                expression: *expression,
            },
            StmtKind::Go { call } | StmtKind::Defer { call } => Action::DeferGo { call: *call },
            StmtKind::Label { name, statement } => Action::Label {
                name: name.clone(),
                statement: *statement,
            },
            StmtKind::Branch { label, .. } => Action::Branch {
                label: label.clone(),
            },
            StmtKind::Missing => Action::Missing,
        };

        match action {
            Action::Missing => {}
            Action::Block(stmts) => self.check_stmt_list(&stmts),
            Action::DeclarationGroup(decls) => {
                let mut group_const = None;
                for decl_id in decls {
                    self.check_stmt_inner(decl_id, &mut group_const);
                }
            }
            Action::Expression(expr_id) => {
                let _ = self.check_expr_outcome(expr_id, None);
            }
            Action::Decl {
                span,
                keyword,
                names,
                annotation,
                values,
            } => match self.text(&keyword) {
                "type" => self.check_type_decl(id, &span, &names, annotation.as_ref()),
                "const" => self.check_const_decl(&names, annotation.as_ref(), &values, const_group),
                "var" => self.check_var_decl(&names, annotation.as_ref(), &values),
                _ => {}
            },
            Action::Assign {
                left,
                operator,
                right,
            } => {
                let op_text = self.text(&operator);
                if op_text == ":=" {
                    self.check_short_var_decl(&left, &right);
                } else {
                    let compound = if op_text == "=" {
                        None
                    } else {
                        Some(&operator)
                    };
                    self.check_assign(&left, compound, &right);
                }
            }
            Action::If {
                init,
                condition,
                then_block,
                otherwise,
            } => {
                if let Some(init_id) = init {
                    self.check_stmt(init_id);
                }
                let expected_bool = self.basic_bool();
                let _ = self.check_expr_outcome(condition, expected_bool);
                self.check_stmt(then_block);
                if let Some(else_id) = otherwise {
                    self.check_stmt(else_id);
                }
            }
            Action::For {
                init,
                condition,
                post,
                body,
            } => {
                if let Some(init_id) = init {
                    self.check_stmt(init_id);
                }
                if let Some(cond_id) = condition {
                    let expected_bool = self.basic_bool();
                    let _ = self.check_expr_outcome(cond_id, expected_bool);
                }
                if let Some(post_id) = post {
                    self.check_stmt(post_id);
                }
                self.check_stmt(body);
            }
            Action::RangeFor { clause, body } => {
                self.check_range_for(clause, body);
            }
            Action::Switch { init, tag, cases } => {
                if let Some(init_id) = init {
                    self.check_stmt(init_id);
                }
                let expected = if let Some(tag_id) = tag {
                    self.check_expr(tag_id, None).map(|op| op.ty)
                } else {
                    self.basic_bool()
                };
                for case_id in cases {
                    let case_data = self.body.statement(case_id).and_then(|s| match &s.kind {
                        StmtKind::Case {
                            values, statements, ..
                        } => Some((values.clone(), statements.clone())),
                        _ => None,
                    });
                    if let Some((values, statements)) = case_data {
                        for val_id in values {
                            let _ = self.check_expr_outcome(val_id, expected);
                        }
                        self.check_stmt_list(&statements);
                    }
                }
            }
            Action::TypeSwitch { init, guard, cases } => {
                if let Some(init_id) = init {
                    self.check_stmt(init_id);
                }
                self.check_type_switch(guard, &cases);
            }
            Action::Select { cases } => {
                for case_id in cases {
                    self.check_stmt(case_id);
                }
            }
            Action::CommCase {
                communication,
                statements,
            } => {
                if let Some(comm_id) = communication {
                    self.check_comm_header(comm_id);
                }
                self.check_stmt_list(&statements);
            }
            Action::CaseFallback { values, statements } => {
                for val_id in values {
                    let _ = self.check_expr_outcome(val_id, None);
                }
                self.check_stmt_list(&statements);
            }
            Action::Return { values } => self.check_return_stmt(&values),
            Action::Send { channel, value } => {
                let ch_op = self.check_expr(channel, None);
                let elem_ty = ch_op.and_then(|op| self.arena.send_element(op.ty));
                let val_op = self.check_expr(value, elem_ty);
                if let (Some(target), Some(val), Some(int_bits)) =
                    (elem_ty, val_op, self.int_bits())
                {
                    let _ = self.arena.assign_value(val, target, int_bits);
                }
            }
            Action::Increment { expression } => {
                let _ = self.check_expr(expression, None);
            }
            Action::DeferGo { call } => {
                let _ = self.check_expr_outcome(call, None);
            }
            Action::Label { name, statement } => {
                let abs_span = self.abs_span(&name);
                let obj = ObjectRef::Label {
                    file: self.file,
                    span: abs_span,
                };
                self.region.defs.push((abs_span, obj));
                if let Some(stmt_id) = statement {
                    self.check_stmt(stmt_id);
                }
            }
            Action::Branch { label } => {
                if let Some(label_r) = label {
                    let name = self.text(&label_r);
                    let abs_span = self.abs_span(&label_r);
                    if let Some(target_span) = self.region.scopes.resolve_label(self.source, name) {
                        self.region.uses.push((
                            abs_span,
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
        stmt_span: &Range<usize>,
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
        let is_alias = self
            .source
            .get(
                abs_name_range.end
                    ..annotation
                        .map(|a| self.body_base + a.start)
                        .unwrap_or(abs_name_range.end),
            )
            .is_some_and(|between| between.contains('='))
            || self
                .text(stmt_span)
                .strip_prefix("type")
                .is_some_and(|rest| rest.trim_start().starts_with(&format!("{name} =")));

        let is_generic_local = self
            .body
            .type_parameters()
            .iter()
            .any(|tp| tp.declaration == stmt_id)
            || self
                .source
                .get(
                    abs_name_range.end
                        ..annotation
                            .map(|a| self.body_base + a.start)
                            .unwrap_or(abs_name_range.end),
                )
                .is_some_and(|between| between.contains('['));

        if is_generic_local
            && let Some(tp_params) = self
                .body
                .type_parameters()
                .iter()
                .find(|tp| tp.declaration == stmt_id)
                .map(|tp| {
                    tp.parameters
                        .iter()
                        .map(|p| (p.name.clone(), p.annotation.clone()))
                        .collect::<Vec<_>>()
                })
        {
            for (tp_name_r, tp_ann_r) in &tp_params {
                if let Some(name_r) = tp_name_r {
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
                if !tp_ann_r.is_empty() {
                    let _ =
                        self.sig_constraint_refs_valid(self.source, 0, &self.abs_range(tp_ann_r));
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
            let name = self.text(name_range);
            if name == "_" {
                continue;
            }
            let Some(local_id) = self.region.scopes.definition_at(abs_span[0]) else {
                continue;
            };
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
                    self.arena.assign_basic(op, target, int_bits)
                } else {
                    Some(op)
                }
            } else {
                None
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

        let (resolved_ops, is_one_to_one): (Vec<Option<Operand>>, bool) = if values.is_empty() {
            (vec![None; names.len()], false)
        } else if names.len() > 1 && values.len() == 1 {
            match self.check_multi_expr(values[0], names.len()) {
                Some(ops) if ops.len() == names.len() => {
                    (ops.into_iter().map(Some).collect(), false)
                }
                _ => (vec![None; names.len()], false),
            }
        } else if names.len() == values.len() {
            let mut ops = Vec::with_capacity(values.len());
            for &val_id in values {
                ops.push(self.check_expr(val_id, ann_ty));
            }
            (ops, true)
        } else {
            for &val_id in values {
                let _ = self.check_expr_outcome(val_id, None);
            }
            (vec![None; names.len()], false)
        };

        for (idx, name_range) in names.iter().enumerate() {
            let abs_span = self.abs_span(name_range);
            let name = self.text(name_range);
            if name == "_" {
                continue;
            }
            let Some(local_id) = self.region.scopes.definition_at(abs_span[0]) else {
                continue;
            };
            let var_ty = if !ann_valid {
                None
            } else if values.is_empty() {
                ann_ty
            } else if let Some(target) = ann_ty {
                if let Some(rhs_op) = resolved_ops.get(idx).and_then(|o| *o) {
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
                resolved_ops.get(idx).and_then(|o| *o).and_then(|rhs_op| {
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

    fn check_short_var_decl(&mut self, lhs: &[ExprId], rhs: &[ExprId]) {
        let Some(int_bits) = self.int_bits() else {
            return;
        };

        let (rhs_ops, is_one_to_one): (Vec<Option<Operand>>, bool) = if lhs.len() > 1
            && rhs.len() == 1
        {
            match self.check_multi_expr(rhs[0], lhs.len()) {
                Some(ops) if ops.len() == lhs.len() => (ops.into_iter().map(Some).collect(), false),
                _ => (vec![None; lhs.len()], false),
            }
        } else if lhs.len() == rhs.len() {
            let mut ops = Vec::with_capacity(rhs.len());
            for &r_id in rhs {
                ops.push(self.check_expr(r_id, None));
            }
            (ops, true)
        } else {
            for &r_id in rhs {
                let _ = self.check_expr_outcome(r_id, None);
            }
            (vec![None; lhs.len()], false)
        };

        for (idx, &lhs_id) in lhs.iter().enumerate() {
            let lhs_span = self
                .body
                .expression(lhs_id)
                .filter(|e| matches!(e.kind, ExprKind::Name))
                .map(|e| e.span.clone());
            let Some(lhs_span) = lhs_span else {
                let _ = self.check_expr_outcome(lhs_id, None);
                continue;
            };
            let name = self.text(&lhs_span);
            if name == "_" {
                continue;
            }
            let abs_span = self.abs_span(&lhs_span);
            let rhs_op = rhs_ops.get(idx).and_then(|o| *o);
            let inferred_ty = rhs_op.and_then(|op| {
                if op.mode == Mode::TypeExpr {
                    None
                } else {
                    self.arena.default_operand(op, int_bits)
                }
            });

            if let Some(local_id) = self.region.scopes.definition_at(abs_span[0]) {
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
                if let Some(obj) = self.local_object(existing_id) {
                    self.region.uses.push((abs_span, obj));
                }
            }
        }
    }

    fn check_assign(&mut self, lhs: &[ExprId], op_range: Option<&Range<usize>>, rhs: &[ExprId]) {
        let Some(int_bits) = self.int_bits() else {
            return;
        };

        let mut lhs_ops = Vec::with_capacity(lhs.len());
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
                lhs_ops.push(self.check_expr(l_id, None));
            }
        }

        if lhs.len() > 1 && rhs.len() == 1 {
            if let Some(rhs_ops) = self.check_multi_expr(rhs[0], lhs.len())
                && rhs_ops.len() == lhs.len()
            {
                for (l_op, r_op) in lhs_ops.iter().zip(rhs_ops.iter()) {
                    if let Some(l) = l_op {
                        let _ = self.check_assignment(*r_op, l.ty, rhs[0], "assignment");
                    }
                }
            }
            return;
        }

        if lhs.len() == rhs.len() {
            for (l_op, &r_id) in lhs_ops.iter().zip(rhs.iter()) {
                let expected = l_op.map(|l| l.ty);
                if let Some(r_op) = self.check_expr(r_id, expected) {
                    if let (Some(op_r), Some(l)) = (op_range, l_op) {
                        let bin_op = self.text(op_r).trim_end_matches('=');
                        let _ = self
                            .arena
                            .binary_expr(bin_op, *l, r_op, Some(l.ty), int_bits);
                    } else if let Some(l) = l_op {
                        let _ = self.check_assignment(r_op, l.ty, r_id, "assignment");
                    }
                }
            }
        } else {
            for &r_id in rhs {
                let _ = self.check_expr_outcome(r_id, None);
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
                let _ = self.arena.assign_value(slot_op, lhs_op.ty, int_bits);
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

    fn check_return_stmt(&mut self, results: &[ExprId]) {
        let expected_results = self.result_types.clone();
        if results.is_empty() {
            return;
        }
        if expected_results.len() > 1
            && results.len() == 1
            && let Some(ops) = self.check_multi_expr(results[0], expected_results.len())
        {
            for (op, expected) in ops.iter().zip(expected_results.iter()) {
                if let Some(target) = *expected {
                    let _ = self.check_assignment(*op, target, results[0], "return statement");
                }
            }
            return;
        }
        for (idx, &res_id) in results.iter().enumerate() {
            let expected = expected_results.get(idx).and_then(|o| *o);
            if let Some(op) = self.check_expr(res_id, expected)
                && let Some(target) = expected
            {
                let _ = self.check_assignment(op, target, res_id, "return statement");
            }
        }
    }

    fn is_receive_expr(&self, mut id: ExprId) -> bool {
        for _ in 0..32 {
            let Some(expr) = self.body.expression(id) else {
                return false;
            };
            match &expr.kind {
                ExprKind::Parenthesized { expression } => id = *expression,
                ExprKind::Unary { operator, .. } => {
                    return self.text(operator) == "<-";
                }
                _ => return false,
            }
        }
        false
    }

    fn check_comm_header(&mut self, comm_id: StmtId) {
        enum CommHeader {
            Valid,
            InvalidExpr(ExprId),
            InvalidAssign(Vec<ExprId>),
            InvalidOther,
        }
        let header = match self.body.statement(comm_id).map(|s| &s.kind) {
            Some(StmtKind::Send { .. }) => CommHeader::Valid,
            Some(StmtKind::Expression { expression }) => {
                if self.is_receive_expr(*expression) {
                    CommHeader::Valid
                } else {
                    CommHeader::InvalidExpr(*expression)
                }
            }
            Some(StmtKind::Assignment {
                operator, right, ..
            }) => {
                if matches!(self.text(operator), "=" | ":=")
                    && right.len() == 1
                    && self.is_receive_expr(right[0])
                {
                    CommHeader::Valid
                } else {
                    CommHeader::InvalidAssign(right.clone())
                }
            }
            _ => CommHeader::InvalidOther,
        };
        match header {
            CommHeader::Valid => self.check_stmt(comm_id),
            CommHeader::InvalidExpr(expr) => {
                let _ = self.check_expr_outcome(expr, None);
            }
            CommHeader::InvalidAssign(right) => {
                for expr in right {
                    let _ = self.check_expr_outcome(expr, None);
                }
            }
            CommHeader::InvalidOther => {}
        }
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
