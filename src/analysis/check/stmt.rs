//! Statement, control-flow, switch, select, label, and return checking.

use std::ops::Range;

use crate::analysis::engine::check::assign::ConstGroupState;
use crate::analysis::engine::check::diagnostics::Code;
use crate::analysis::engine::check::region::WriteKind;
use crate::analysis::engine::check::{ExprOutcome, FileSymbolLookup, RegionChecker};
use crate::analysis::engine::obj::{LocalId, ObjectRef, SymbolOrigin, VarKind};
use crate::analysis::engine::scope::tree::{BindingInit, ScopeKind};
use crate::analysis::engine::types::{Mode, Operand, TypeId};
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
                self.record_write_target(*channel, WriteKind::Modify);
                let ch_op = self.check_expr(*channel, None);
                let elem_ty = ch_op.and_then(|op| self.arena.send_element(op.ty));
                let val_op = self.check_expr(*value, elem_ty);
                if let (Some(target), Some(val), Some(int_bits)) =
                    (elem_ty, val_op, self.int_bits())
                    && self.arena.assignable(val, target, int_bits) == Some(true)
                {
                    self.update_expr_type(*value, target, int_bits);
                }
            }
            StmtKind::Increment { expression, .. } => {
                self.record_write_target(*expression, WriteKind::Modify);
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
                self.record_def(abs_span, obj);
                if let Some(statement) = statement {
                    self.check_stmt(*statement);
                }
            }
            StmtKind::Branch { label, .. } => {
                if let Some(label) = label {
                    let name = self.text(label);
                    let span = self.abs_span(label);
                    if let Some(target_span) = self.region.scopes.resolve_label(self.source, name) {
                        self.record_use(
                            span,
                            ObjectRef::Label {
                                file: self.file,
                                span: target_span,
                            },
                        );
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
                    TypeSwitchCaseItem::Type(ty) => all_targets.push(Some(ty)),
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
                            case_scope: crate::analysis::engine::obj::CaseScope::from_span(
                                case_abs_span,
                            ),
                        };
                        self.record_implicit(case_abs_span, obj);
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
                self.record_use(self.abs_span(&span), ObjectRef::Nil);
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

    fn check_return_stmt(&mut self, span: &Range<usize>, results: &[ExprId]) {
        let expected_count = self.env.result_types.len();
        if results.is_empty() {
            if expected_count != 0 && !self.env.named_results {
                self.return_count(span, results, &[]);
            }
            return;
        }
        if let [expr] = results
            && (expected_count != 1 || self.is_call(*expr))
        {
            let expected = if expected_count == 1 {
                self.env.result_types[0]
            } else {
                None
            };
            if let Some(ops) = self.check_value_pack(*expr, false, expected) {
                if ops.len() == expected_count {
                    for (index, op) in ops.into_iter().enumerate() {
                        if let Some(target) = self.env.result_types[index] {
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
            let expected = self.env.result_types[index];
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
