//! Document-wide LSP features and Go documentation rendering.
mod comment;
mod markdown;
pub(crate) use comment::{Target, comments};
pub(crate) use markdown::markdown;
pub(super) mod highlight;
pub(super) mod link;
pub(super) mod symbol;
