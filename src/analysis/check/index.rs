//! Share typed indexing between value operands and scoped navigation.
use super::{Inference, ScopedType, Selection};
use crate::analysis::types::{Mode, Operand, TypeArena};
use crate::syntax::{ExprId, ExprKind};

impl<'workspace> Inference<'workspace, '_> {
    pub(super) fn indexed_operand(
        &self,
        arena: &mut TypeArena<'_>,
        id: ExprId,
        result: usize,
        count: usize,
    ) -> Option<Operand> {
        if self.depth >= 32 {
            return None;
        }
        let ExprKind::Index { operand, arguments } = &self.unparenthesized(id).kind else {
            return None;
        };
        if arguments.len() != 1 {
            return None;
        }
        let input = self.container_operand(arena, *operand)?;
        let index = self.next().value_operand(arena, arguments[0])?;
        arena.index(input, index, result, count, self.workspace.integer_bits()?)
    }

    fn container_operand(&self, arena: &mut TypeArena<'_>, operand: ExprId) -> Option<Operand> {
        let input = self.next().value_operand(arena, operand)?;
        let offset = self.base + self.expressions[operand.0].span.start;
        if !arena
            .basic_kind(input.ty)
            .is_some_and(|kind| kind.untyped())
            && let Some(text) = arena.render(input.ty, self.file)
            && !self.concrete_spelling(&text, offset)
        {
            // A compatibility spelling cannot impersonate a local formal. A
            // native formal graph must resolve back to its canonical TypeIds.
            let projected = self.parameterized_type(arena, &text, offset)?;
            arena.identical(input.ty, projected)?.then_some(())?;
        }
        Some(input)
    }

    pub(super) fn sliced_operand(&self, arena: &mut TypeArena<'_>, id: ExprId) -> Option<Operand> {
        if self.depth >= 32 {
            return None;
        }
        let ExprKind::Slice {
            operand,
            low,
            high,
            max,
            full,
        } = &self.unparenthesized(id).kind
        else {
            return None;
        };
        let mut input = self.container_operand(arena, *operand)?;
        if input.mode != Mode::Variable
            && arena.is_array(input.ty)?
            && self.next().addressable_place(arena, *operand)?
        {
            input.mode = Mode::Variable;
        }
        let mut indices = [None; 3];
        for (slot, id) in indices.iter_mut().zip([low, high, max]) {
            if let Some(id) = id {
                *slot = Some(self.next().value_operand(arena, *id)?);
            }
        }
        arena.slice(input, indices, *full, self.workspace.integer_bits()?)
    }

    fn addressable_place(&self, arena: &mut TypeArena<'_>, id: ExprId) -> Option<bool> {
        if self.depth >= 32 {
            return None;
        }
        let expression = self.unparenthesized(id);
        match &expression.kind {
            ExprKind::Unary { operator, .. } if self.text(operator) == "*" => Some(true),
            ExprKind::Selector { operand, .. } => {
                if !matches!(self.next().selection(id), Some(Selection::Member(member)) if member.kind == crate::syntax::MemberKind::Field)
                {
                    return Some(false);
                }
                let mut owner = self.next().value_operand(arena, *operand)?;
                if owner.mode != Mode::Variable && self.next().addressable_place(arena, *operand)? {
                    owner.mode = Mode::Variable;
                }
                arena.field_addressable(owner)
            }
            _ => Some(false),
        }
    }

    pub(super) fn scoped_slice(
        &self,
        id: ExprId,
        result: usize,
        count: usize,
    ) -> Option<ScopedType<'workspace>> {
        if result != 0 || count != 1 {
            return None;
        }
        let mut arena = TypeArena::new(self.workspace);
        let operand = self.sliced_operand(&mut arena, id)?;
        self.scoped_native_type(
            &mut arena,
            operand.ty,
            self.file,
            self.base + self.expressions[id.0].span.start,
        )
    }

    pub(super) fn scoped_index(
        &self,
        id: ExprId,
        result: usize,
        count: usize,
    ) -> Option<ScopedType<'workspace>> {
        let mut arena = TypeArena::new(self.workspace);
        if result == 0
            && count == 1
            && let Some(operand) = self.function_operand(&mut arena, id)
        {
            return self.scoped(arena.render_resolved(operand.ty, self.file)?);
        }
        let operand = self.indexed_operand(&mut arena, id, result, count)?;
        self.scoped_native_type(
            &mut arena,
            operand.ty,
            self.file,
            self.base + self.expressions[id.0].span.start,
        )
    }
}
