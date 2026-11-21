//! Range-for clause checking and write-only target recording.

use crate::analysis::engine::check::RegionChecker;
use crate::analysis::engine::obj::VarKind;
use crate::analysis::engine::types::{Mode, Operand};
use crate::syntax::{ExprId, ExprKind, StmtId, StmtKind};

impl<'a, 'b> RegionChecker<'a, 'b> {
    pub(super) fn check_range_for(&mut self, clause_id: StmtId, body_id: StmtId) {
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
                    self.record_type_and_value(
                        self.abs_span(sp),
                        Operand {
                            mode: Mode::Variable,
                            ty,
                            value: None,
                        },
                    );
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
                    self.record_use(abs_span, obj);
                    self.record_write(abs_span);
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
    pub(super) fn record_write_only(&mut self, id: ExprId) {
        if let Some(expr) = self.body.expression(self.unwrap_parens(id))
            && matches!(expr.kind, ExprKind::Name)
        {
            self.record_write(self.abs_span(&expr.span));
        }
    }
}
