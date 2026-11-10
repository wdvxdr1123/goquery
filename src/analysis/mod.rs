//! Shared semantic queries over declaration metadata and temporary body arenas.
//! This layer depends on workspace/index/text, never on feature or server routing.
pub(crate) mod check;
pub(crate) mod member;
pub(crate) mod method_set;
pub(crate) mod query;
pub(crate) mod resolve;
pub(crate) mod scope;
pub(crate) mod source;
#[cfg(test)]
mod type_oracle;
pub(crate) mod type_resolution;
pub(crate) mod types;
