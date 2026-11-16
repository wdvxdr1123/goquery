//! Query-local type construction, operands and bounded lazy type relations.
mod arena;
mod basic;
mod builtin;
mod complex;
mod constraints;
mod conversion;
mod declaration;
pub(crate) mod fingerprint;
mod form;
mod format;
mod generics;
mod index;
mod interface;
mod iteration;
pub(crate) mod lookup;
mod methods;
mod named;
mod operand;
mod ordered;
mod parameters;
mod relation;
mod selection;
mod sets;
mod signature;
mod structure;
mod syntax;
mod values;

pub(crate) use arena::{ArenaWatermark, TypeArena, TypeId};
use arena::{Direction, Ty};
pub(crate) use basic::BasicKind;
pub(crate) use format::Qualifier;
use interface::Method;
pub(crate) use operand::{Constant, Mode, Operand};
pub(crate) use parameters::LocalFormal;
pub(crate) use selection::{PromotionIndices, Selection, SelectionKind};
use structure::Field;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tags {
    Include,
    Ignore,
}

use crate::analysis::budget::{MAX_BYTES, MAX_DEPTH, MAX_NODES};

pub(crate) use form::{
    TypeForm, TypePresentation, declared_type_presentation, local_type_presentation,
    type_form_in_scope,
};
pub(crate) use generics::completion_inferable;
pub(crate) use syntax::{declaration_body, type_parentheses, unparenthesized_type};

#[cfg(test)]
pub(crate) use complex::ComplexPart;
pub(crate) use declaration::{LocalTypeDefinition, TypeDeclaration};
