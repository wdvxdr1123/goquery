//! Shared receive/comma-ok operands; closed-source and type graphs stay local.
use super::{Inference, ScopedType};
use crate::analysis::types::{Operand, TypeArena};
use crate::syntax::{ExprId, ExprKind};

impl<'workspace> Inference<'workspace, '_> {
    pub(super) fn receive_operand(
        &self,
        arena: &mut TypeArena<'_>,
        id: ExprId,
        result: usize,
        count: usize,
    ) -> Option<Operand> {
        if self.depth >= 32 {
            return None;
        }
        let ExprKind::Unary { operator, operand } = &self.unparenthesized(id).kind else {
            return None;
        };
        if self.text(operator) != "<-" {
            return None;
        }
        let input = self
            .next()
            .value_operand(arena, *operand)
            .or_else(|| self.next().parameterized_operand(arena, *operand))?;
        arena.receive_slot(input, result, count)
    }

    pub(super) fn scoped_receive(
        &self,
        id: ExprId,
        result: usize,
        count: usize,
    ) -> Option<ScopedType<'workspace>> {
        let mut arena = TypeArena::new(self.workspace);
        let value = self.receive_operand(&mut arena, id, result, count)?;
        self.scoped_native_type(
            &mut arena,
            value.ty,
            self.file,
            self.base + self.unparenthesized(id).span.start,
        )
    }
}
