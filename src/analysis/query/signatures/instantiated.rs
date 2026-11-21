//! Display the already-instantiated signature; no source type substitution.
use super::label::{CallableSignature, signature_from_native};
use crate::analysis::sema::{RegionView, TypeHandle};
use std::path::Path;

pub(in crate::analysis::query) fn format_instantiated_signature<'r>(
    consumer: &Path,
    view: &mut RegionView<'r, '_>,
    name: &str,
    instantiated: TypeHandle<'r>,
) -> Option<(CallableSignature, bool)> {
    let callable = view.callable_view(instantiated, consumer)?;
    signature_from_native(name, &callable)
}
