//! Value assignment checks through the structural type arena.
use super::{Operand, TypeArena, TypeId};

impl TypeArena<'_> {
    pub(crate) fn assign_value(
        &mut self,
        source: Operand,
        target: TypeId,
        int_bits: u32,
    ) -> Option<bool> {
        self.assignable(source, target, int_bits)
    }
}
