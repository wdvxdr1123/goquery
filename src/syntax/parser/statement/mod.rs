//! On-demand block, simple-statement and control-flow parsing.
mod block;
mod control;
mod declaration;
mod select;
mod simple;

pub(in crate::syntax::parser) use block::declaration_parser;
pub use block::parse_block;
