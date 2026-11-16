//! Bounded completion collection and client-aware insertion edits.
mod collect;
mod members;
mod render;
pub(crate) use render::CompletionConfig;

pub(super) use collect::execute;
