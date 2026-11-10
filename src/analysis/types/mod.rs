//! Query-local type construction, operands and bounded lazy type relations.
mod arena;
mod basic;
mod builtin;
mod complex;
mod constraints;
mod conversion;
mod declaration;
mod form;
mod generics;
mod index;
mod interface;
mod iteration;
mod methods;
mod named;
mod operand;
mod ordered;
mod parameters;
mod projection;
mod relation;
mod sets;
mod signature;
mod structure;
mod syntax;
mod values;

use arena::{DeclRef, Direction, Ty};
pub(crate) use arena::{TypeArena, TypeId};
pub(crate) use basic::BasicKind;
use interface::Method;
pub(crate) use operand::{Constant, Mode, Operand};
pub(crate) use parameters::LocalFormal;
use structure::Field;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tags {
    Include,
    Ignore,
}

use crate::analysis::query::budget::{MAX_BYTES, MAX_DEPTH, MAX_NODES};

pub(crate) use form::{TypeForm, TypePresentation};
pub(crate) use syntax::declaration_body;

#[cfg(test)]
pub(crate) use complex::ComplexPart;
pub(crate) use declaration::{LocalTypeDefinition, LocalTypeName, TypeDeclaration};
