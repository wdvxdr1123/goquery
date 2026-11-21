//! On-demand type syntax and aggregate members.
mod members;
mod signature;
mod syntax;
mod tree;

#[cfg(test)]
mod tests;

pub use syntax::{parse_constraint, parse_type, parse_type_prefix};
