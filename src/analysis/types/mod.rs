//! Query-local type construction, operands and bounded lazy type relations.
mod arena;
mod basic;
mod builtin;
mod complex;
mod constant;
mod constraints;
mod conversion;
mod declaration;
mod form;
mod format;
mod generics;
mod index;
mod interface;
mod iteration;
mod layout;
mod methods;
mod named;
mod operand;
mod ordered;
mod parameters;
mod relation;
mod schema;
mod selection;
mod sets;
mod signature;
mod signature_names;
mod structure;
mod syntax;
mod ty;
mod values;

pub(crate) use arena::{ArenaWatermark, TypeArena};
pub(crate) use basic::BasicKind;
pub(crate) use format::Qualifier;
use interface::Method;
pub(crate) use layout::{FieldLayout, Layout, StructLayout, align_to};
pub(crate) use operand::{Constant, Mode, Operand};
pub(crate) use parameters::LocalFormal;
pub(crate) use selection::{PromotionIndices, Selection, SelectionKind};
use structure::Field;
pub(crate) use ty::TypeId;
use ty::{Direction, Ty};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tags {
    Include,
    Ignore,
}

use crate::analysis::budget::{MAX_BYTES, MAX_DEPTH, MAX_NODES};

pub(crate) use form::{TypeForm, type_form_in_scope};
pub(crate) use generics::completion_inferable;
pub(crate) use signature_names::CallableNames;
pub(crate) use syntax::{declaration_body, unparenthesized_type};

#[cfg(test)]
pub(crate) use complex::ComplexPart;
pub(crate) use declaration::{LocalTypeDefinition, TypeDeclaration};
