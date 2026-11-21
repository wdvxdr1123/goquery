//! Cross-file lexical prefilter scans; semantic resolution belongs to `Semantics`.
mod lexical;

pub(crate) use lexical::{identifier_at, scan_name, scan_qualified_name};
