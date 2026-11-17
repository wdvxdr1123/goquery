//! Shared semantic queries over declaration metadata and temporary body arenas.
//! This layer depends on workspace/index/text, never on feature or server routing.
mod budget;
mod check;
mod obj;
pub(crate) mod query;
mod resolve;
mod scope;
mod sema;
#[cfg(test)]
mod type_oracle;
mod types;
