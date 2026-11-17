//! Composite key identity projected through the request's native type graph.
use super::Semantics;
use crate::analysis::obj::{MemberDeclId, ObjectRef};
use crate::analysis::types::Mode;
use std::ops::Range;
use std::path::Path;

#[derive(Clone, Copy)]
pub(crate) enum LiteralKey<'a> {
    Field(MemberDeclId<'a>),
    Expression,
    Unknown,
}

impl<'a> Semantics<'a> {
    /// `projections` runs from the innermost elided literal to its written
    /// container. Every step must have an available native container type;
    /// missing imports and exhausted budgets provide no field identity.
    pub(crate) fn literal_key_at(
        &mut self,
        file: &Path,
        source: &str,
        span: Range<usize>,
        projections: &[bool],
        name: &str,
    ) -> LiteralKey<'a> {
        self.project_literal_key(file, source, span, projections, name)
            .unwrap_or(LiteralKey::Unknown)
    }

    fn project_literal_key(
        &mut self,
        file: &Path,
        source: &str,
        span: Range<usize>,
        projections: &[bool],
        name: &str,
    ) -> Option<LiteralKey<'a>> {
        if span.len() > 16 * 1024 || projections.len() > 64 {
            return None;
        }
        let span = [
            u32::try_from(span.start).ok()?,
            u32::try_from(span.end).ok()?,
        ];
        let (region, arena) = self.region_and_arena_at(file, source, span[0], None)?;
        let operand = region.expr_type(span)?;
        if operand.mode != Mode::TypeExpr {
            return None;
        }
        let mut ty = operand.ty;
        for key in projections.iter().rev() {
            let (key_ty, value_ty) = arena.composite_container_types(ty)?;
            ty = if *key { key_ty? } else { value_ty };
            ty = arena.elided_composite_element_type(ty)?;
        }
        if arena.composite_container_types(ty).is_some() {
            return Some(LiteralKey::Expression);
        }
        let selection = arena.literal_field(ty, file, name)?;
        let ObjectRef::Field(field) = selection.obj else {
            return None;
        };
        Some(LiteralKey::Field(field))
    }
}
