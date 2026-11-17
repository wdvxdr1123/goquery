//! Lexical occurrences and candidate roles; semantic facts belong to the checker.
mod build;
mod classify;
mod lexical;
mod literals;

pub(crate) use build::{Name, Occurrences, Role};
pub(crate) use lexical::identifier_at;
