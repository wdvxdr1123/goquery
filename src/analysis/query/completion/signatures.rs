//! Source names decorate free function signatures; receiver types stay native.
use crate::analysis::query::signatures::label::{
    CallableSignature, MAX_LABEL_BYTES, signature_from_parsed,
};
use crate::index::IndexedSymbol as Symbol;
use crate::text::SourceMap;
use crate::workspace::Workspace;
use std::path::Path;

pub(crate) struct SignatureSource<'a> {
    pub file: &'a Path,
    pub origin: &'a Path,
    pub source: &'a str,
    pub map: &'a SourceMap<'a>,
}

pub(super) fn signature_in_source(
    workspace: &Workspace,
    location: &SignatureSource<'_>,
    function: &Symbol,
) -> Option<(CallableSignature, bool)> {
    let start = location.map.offset(function.range.start)?;
    let end = function
        .body()
        .map(|body| body.start)
        .or_else(|| location.map.offset(function.range.end))?;
    let header = location.source.get(start..end)?;
    if header.len() > MAX_LABEL_BYTES {
        return None;
    }
    signature_from_parsed(
        workspace,
        location.file,
        location.origin,
        header,
        &crate::syntax::parse_signature(header),
        &function.name,
    )
}
