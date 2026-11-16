//! Named function headers and reusable parameter-list parsing.
mod header;
mod parameters;

pub use header::{
    parse_literal_signature, parse_method_signature, parse_signature, parse_type_parameters,
};
pub(in crate::syntax::parser) use parameters::ParameterContext;
