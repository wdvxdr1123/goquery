//! Handwritten declaration, expression and type parsing.
mod aggregate;
mod brackets;
mod composite;
mod declarations;
mod expression;
mod file;
mod imports;
mod postfix;
mod signature;
mod statement;
mod types;

pub use aggregate::{aggregate_members, aggregate_offset, type_parameter_end};
pub use declarations::stub;
pub use expression::parse_expression;
pub use file::validate_file;
pub(crate) use file::validate_file_with_imports;
pub(crate) use imports::qualified_prefixes;
pub use signature::{
    parse_literal_signature, parse_method_signature, parse_signature, parse_type_parameters,
};
pub use statement::{DeclSpecKind, parse_block, parse_decl_spec, parse_declaration};
pub use types::{parse_type, parse_type_prefix};
