//! Named function headers and reusable parameter-list parsing.
mod header;
mod parameters;

pub use header::{parse_literal_signature, parse_signature};
pub(in crate::syntax::parser) use parameters::ParameterContext;
