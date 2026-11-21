//! Display the already-instantiated signature; no source type substitution.
use super::label::{CallableSignature, signature_from_native};
use crate::analysis::sema::{RegionView, TypeHandle};
use std::path::Path;

pub(in crate::analysis::query) fn format_instantiated_signature(
    consumer: &Path,
    view: &mut RegionView<'_, '_>,
    name: &str,
    instantiated: TypeHandle<'_>,
) -> Option<(CallableSignature, bool)> {
    let callable = view.callable_view(instantiated, consumer)?;
    signature_from_native(name, &callable)
}
