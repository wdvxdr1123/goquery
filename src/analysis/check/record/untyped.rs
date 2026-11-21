//! Bounded per-region `untyped` delayed-typing table and context-type propagation.

use std::collections::HashMap;

use crate::analysis::engine::budget::{MAX_DEPTH, MAX_NODES};
use crate::analysis::engine::check::RegionChecker;
use crate::analysis::engine::check::record::push_expr_type;
use crate::analysis::engine::check::region::RecordMask;
use crate::analysis::engine::types::{BasicKind, Mode, Operand, TypeId};
use crate::syntax::{ExprId, ExprKind};

/// Entry in [`UntypedTable`] mirroring `go/types.exprInfo`.
#[derive(Clone, Copy)]
pub(super) struct UntypedEntry {
    pub(super) span: [u32; 2],
    pub(super) is_lhs_shift: bool,
    pub(super) op: Operand,
}

/// Bounded per-region table of untyped constant/nil/shift expressions awaiting context typing.
pub(in crate::analysis::engine::check) struct UntypedTable {
    entries: HashMap<usize, UntypedEntry>,
}

impl UntypedTable {
    pub(in crate::analysis::engine::check) fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    pub(in crate::analysis::engine::check) fn insert(
        &mut self,
        id: ExprId,
        span: [u32; 2],
        op: Operand,
    ) -> bool {
        let key = id.index();
        if self.entries.len() < MAX_NODES || self.entries.contains_key(&key) {
            let is_lhs_shift = self.entries.get(&key).is_some_and(|e| e.is_lhs_shift);
            self.entries.insert(
                key,
                UntypedEntry {
                    span,
                    is_lhs_shift,
                    op,
                },
            );
            true
        } else {
            false
        }
    }

    pub(in crate::analysis::engine::check) fn mark_lhs_shift(&mut self, id: ExprId) {
        if let Some(entry) = self.entries.get_mut(&id.index()) {
            entry.is_lhs_shift = true;
        }
    }

    pub(super) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

enum UntypedConversion {
    Converted(Operand),
    Unknown,
    Invalid,
}

impl<'a, 'b> RegionChecker<'a, 'b> {
    /// Update an untyped expression and its homogeneous untyped subexpressions
    /// once a surrounding assignment, declaration, return, shift, or call fixes `target_ty`.
    pub(in crate::analysis::engine::check) fn update_expr_type(
        &mut self,
        id: ExprId,
        target_ty: TypeId,
        int_bits: u32,
    ) {
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
        let _ = self.update_expr_type_inner(id, target_ty, int_bits, 0);
    }

    fn update_expr_type_inner(
        &mut self,
        id: ExprId,
        target_ty: TypeId,
        int_bits: u32,
        depth: usize,
    ) -> bool {
        if depth >= MAX_DEPTH {
            return true;
        }
        let Some(entry) = self.untyped.entries.remove(&id.index()) else {
            return true;
        };
        let Some(expr) = self.body.expression(id) else {
            return true;
        };
        let is_constant = entry.op.mode == Mode::Constant && entry.op.value.is_some();
        let child_ok = match &expr.kind {
            ExprKind::Parenthesized { expression } => {
                let inner = *expression;
                self.update_expr_type_inner(inner, target_ty, int_bits, depth + 1)
            }
            ExprKind::Unary { operator, operand } if !is_constant => {
                let op_text = self.text(operator);
                let inner = *operand;
                if matches!(op_text, "+" | "-" | "^" | "!") {
                    self.update_expr_type_inner(inner, target_ty, int_bits, depth + 1)
                } else {
                    true
                }
            }
            ExprKind::Binary {
                left,
                operator,
                right,
            } if !is_constant => {
                let op_text = self.text(operator);
                let (l, r) = (*left, *right);
                if matches!(op_text, "<<" | ">>") {
                    // Shift result type depends only on LHS; RHS was checked independently.
                    self.update_expr_type_inner(l, target_ty, int_bits, depth + 1)
                } else if matches!(
                    op_text,
                    "+" | "-" | "*" | "/" | "%" | "&" | "|" | "^" | "&^" | "&&" | "||"
                ) {
                    let ok_l = self.update_expr_type_inner(l, target_ty, int_bits, depth + 1);
                    let ok_r = self.update_expr_type_inner(r, target_ty, int_bits, depth + 1);
                    ok_l && ok_r
                } else {
                    true
                }
            }
            _ => true,
        };
        if !child_ok {
            return false;
        }
        match self.convert_untyped_entry(entry, target_ty, int_bits) {
            UntypedConversion::Converted(converted) => {
                self.record_type_and_value(entry.span, converted);
                true
            }
            UntypedConversion::Unknown => {
                self.record_type_and_value(entry.span, entry.op);
                true
            }
            UntypedConversion::Invalid => false,
        }
    }

    fn convert_untyped_entry(
        &mut self,
        entry: UntypedEntry,
        target_ty: TypeId,
        int_bits: u32,
    ) -> UntypedConversion {
        let Some(source_kind) = self.arena.basic_kind(entry.op.ty) else {
            return UntypedConversion::Unknown;
        };
        if !source_kind.untyped() {
            return UntypedConversion::Converted(entry.op);
        }
        if source_kind == BasicKind::UntypedNil {
            return if entry.is_lhs_shift {
                UntypedConversion::Invalid
            } else {
                // Preserve UntypedNil in Info.Types, matching go/types (go.dev/issue/13061).
                UntypedConversion::Converted(entry.op)
            };
        }
        let effective_target = if self.arena.is_interface(target_ty) == Some(true) {
            let defaulted = if entry.is_lhs_shift {
                self.arena.basic(BasicKind::Int)
            } else {
                self.arena.default_operand(entry.op, int_bits)
            };
            let Some(ty) = defaulted else {
                return UntypedConversion::Unknown;
            };
            ty
        } else {
            target_ty
        };
        let Some(target_kind) = self.arena.basic_kind(effective_target) else {
            // Non-basic target (e.g., type parameter or unresolved): preserve
            // three-valued Unknown discipline without inventing or dropping types.
            return UntypedConversion::Unknown;
        };
        if entry.is_lhs_shift && !target_kind.integer() {
            return UntypedConversion::Invalid;
        }
        if entry.op.mode != Mode::Constant {
            let compatible = match source_kind {
                BasicKind::UntypedBool => target_kind.boolean(),
                BasicKind::UntypedInt
                | BasicKind::UntypedRune
                | BasicKind::UntypedFloat
                | BasicKind::UntypedComplex => target_kind.numeric_rank().is_some(),
                BasicKind::UntypedString => target_kind.string(),
                _ => false,
            };
            if !compatible {
                return UntypedConversion::Invalid;
            }
        }
        match self
            .arena
            .assign_basic(entry.op, effective_target, int_bits)
        {
            Some(converted) => UntypedConversion::Converted(converted),
            None => {
                if entry.op.mode == Mode::Constant && entry.op.value.is_none() {
                    UntypedConversion::Unknown
                } else {
                    UntypedConversion::Invalid
                }
            }
        }
    }

    pub(in crate::analysis::engine::check) fn flush_untyped(&mut self) {
        if !self.record_mask.contains(RecordMask::TYPES) {
            self.untyped.entries.clear();
            return;
        }
        let expr_types = &mut self.region.expr_types;
        for (_, entry) in self.untyped.entries.drain() {
            push_expr_type(expr_types, entry.span, entry.op);
        }
    }
}

impl Drop for RegionChecker<'_, '_> {
    fn drop(&mut self) {
        if !self.untyped.is_empty() {
            self.flush_untyped();
        }
    }
}
