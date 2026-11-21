//! Shared semantic queries over declaration metadata and temporary body arenas.
//! This layer depends on workspace/index/text, never on feature or server routing.
mod engine;
use engine::{budget, resolve, sema};
pub(crate) mod query;
