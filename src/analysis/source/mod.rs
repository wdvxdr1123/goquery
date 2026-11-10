//! Handwritten query-local names and lexical identities; no retained syntax trees.
mod build;
mod classify;
mod lexical;
mod literals;

pub(crate) use build::{Name, Role, SourceAnalysis};
pub(crate) use lexical::identifier_at;
