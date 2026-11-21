//! Unified semantic fact recording and bounded `untyped` delayed-typing table.

use std::collections::HashMap;

use crate::analysis::engine::budget::{MAX_DEPTH, MAX_NODES};
use crate::analysis::engine::check::region::{CallSiteInfo, InstanceInfo, RecordMask};
use crate::analysis::engine::check::{ExprOutcome, RegionChecker};
use crate::analysis::engine::obj::{ObjectRef, SymbolOrigin};
use crate::analysis::engine::scope::tree::LocalBindingKind;
use crate::analysis::engine::types::{BasicKind, Operand, Selection, TypeId};
use crate::syntax::{ExprId, ExprKind};

/// Bounded per-region table of untyped constant/nil expressions awaiting context typing.
pub(super) struct UntypedTable {
    entries: HashMap<usize, Operand>,
}

impl UntypedTable {
    pub(super) fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    pub(super) fn insert(&mut self, id: ExprId, op: Operand) {
        let key = id.index();
        if self.entries.len() < MAX_NODES || self.entries.contains_key(&key) {
            self.entries.insert(key, op);
        }
    }
}

impl<'a, 'b> RegionChecker<'a, 'b> {
    pub(super) fn record_def(&mut self, span: [u32; 2], obj: ObjectRef<'a>) {
        if self.record_mask.contains(RecordMask::DEFS) {
            self.region.defs.push((span, obj));
        }
    }

    pub(super) fn record_use(&mut self, span: [u32; 2], obj: ObjectRef<'a>) {
        if self.record_mask.contains(RecordMask::USES) {
            self.region.uses.push((span, obj));
        } else if self.record_mask.contains(RecordMask::DIAGNOSTICS)
            && self.region.uses_complete
            && let ObjectRef::Var {
                origin: SymbolOrigin::Local { id, .. },
                ..
            } = obj
            && self
                .region
                .scopes
                .bindings
                .get(id.0 as usize)
                .is_some_and(|binding| {
                    matches!(
                        binding.kind,
                        LocalBindingKind::Var
                            | LocalBindingKind::ShortVar
                            | LocalBindingKind::RangeKey
                            | LocalBindingKind::RangeVal
                            | LocalBindingKind::TypeSwitchVar
                    )
                })
        {
            self.region.local_var_uses.push((span, id));
        }
    }

    pub(super) fn record_implicit(&mut self, span: [u32; 2], obj: ObjectRef<'a>) {
        if self.record_mask.contains(RecordMask::IMPLICITS) {
            self.region.implicits.push((span, obj));
        }
    }

    pub(super) fn record_selection(&mut self, span: [u32; 2], selection: Selection<'a>) {
        if self.record_mask.contains(RecordMask::SELECTIONS) {
            self.region.selections.push((span, selection));
        }
    }

    pub(super) fn record_instance(&mut self, span: [u32; 2], info: InstanceInfo<'a>) {
        if self.record_mask.contains(RecordMask::INSTANCES) {
            self.region.instances.push((span, info));
        }
    }

    pub(super) fn record_call(&mut self, info: CallSiteInfo<'a>) {
        if self.record_mask.contains(RecordMask::CALLS) {
            self.region.calls.push(info);
        }
    }

    pub(super) fn record_write(&mut self, span: [u32; 2]) {
        if self.record_mask.contains(RecordMask::WRITES)
            || (self.record_mask.contains(RecordMask::DIAGNOSTICS) && self.region.uses_complete)
        {
            self.region.writes.push(span);
        }
    }

    pub(super) fn record_type_and_value(&mut self, span: [u32; 2], op: Operand) {
        if self.record_mask.contains(RecordMask::TYPES) {
            self.region.expr_types.push((span, op));
        }
    }

    pub(super) fn record_expr_outcome(&mut self, id: ExprId, outcome: &ExprOutcome<'a, 'b>) {
        if !self.record_mask.contains(RecordMask::TYPES) {
            return;
        }
        if let ExprOutcome::Value(op) = outcome
            && let Some(expr) = self.body.expression(id)
        {
            let span = self.abs_span(&expr.span);
            self.record_type_and_value(span, *op);
            if op.mode.is_value() && self.arena.basic_kind(op.ty).is_some_and(BasicKind::untyped) {
                self.untyped.insert(id, *op);
            }
        }
    }

    /// Update an untyped expression and its homogeneous untyped subexpressions
    /// once a surrounding assignment, declaration, return, or call fixes `target_ty`.
    pub(super) fn update_expr_type(&mut self, id: ExprId, target_ty: TypeId, int_bits: u32) {
        if !self.record_mask.contains(RecordMask::TYPES) {
            return;
        }
        if self
            .arena
            .basic_kind(target_ty)
            .is_some_and(BasicKind::untyped)
        {
            return;
        }
        self.update_expr_type_inner(id, target_ty, int_bits, 0);
    }

    fn update_expr_type_inner(
        &mut self,
        id: ExprId,
        target_ty: TypeId,
        int_bits: u32,
        depth: usize,
    ) {
        if depth >= MAX_DEPTH {
            return;
        }
        let Some(op) = self.untyped.entries.remove(&id.index()) else {
            return;
        };
        let Some(converted) = self.arena.assign_basic(op, target_ty, int_bits) else {
            return;
        };
        let Some(expr) = self.body.expression(id) else {
            return;
        };
        let span = self.abs_span(&expr.span);
        self.record_type_and_value(span, converted);
        match &expr.kind {
            ExprKind::Parenthesized { expression } => {
                let inner = *expression;
                self.update_expr_type_inner(inner, target_ty, int_bits, depth + 1);
            }
            ExprKind::Unary { operator, operand } => {
                let op_text = self.text(operator);
                let inner = *operand;
                if matches!(op_text, "+" | "-" | "^") {
                    self.update_expr_type_inner(inner, target_ty, int_bits, depth + 1);
                }
            }
            ExprKind::Binary {
                left,
                operator,
                right,
            } => {
                let op_text = self.text(operator);
                let (l, r) = (*left, *right);
                if matches!(
                    op_text,
                    "+" | "-" | "*" | "/" | "%" | "&" | "|" | "^" | "&^" | "&&" | "||"
                ) {
                    self.update_expr_type_inner(l, target_ty, int_bits, depth + 1);
                    self.update_expr_type_inner(r, target_ty, int_bits, depth + 1);
                }
            }
            _ => {}
        }
    }

    pub(super) fn flush_untyped(&mut self) {
        self.untyped.entries.clear();
    }
}
