//! Display the already-instantiated signature; no source type substitution.
use super::label::{CallableSignature, signature_from_native};
use crate::analysis::sema::CallableView;
use crate::analysis::types::{TypeArena, TypeId};
use std::path::Path;

pub(in crate::analysis::query) fn format_instantiated_signature(
    consumer: &Path,
    arena: &mut TypeArena<'_>,
    name: &str,
    instantiated: TypeId,
) -> Option<(CallableSignature, bool)> {
    signature_from_native(
        name,
        &CallableView::from_type(arena, instantiated, consumer)?,
    )
}
