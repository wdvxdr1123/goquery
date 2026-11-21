//! Request-local checked lexical scope trees and Go universe/type-name helpers.
pub(crate) mod named;
pub(crate) mod occurrences;
pub(crate) mod signature;
pub(crate) mod tree;
pub(crate) mod universe;

pub(crate) use named::{named_root, named_type};
pub(crate) use signature::receiver_parameters;
