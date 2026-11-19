//! Query-local type construction, operands and bounded lazy type relations.
mod arena;
mod assignment;
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

pub(crate) use arena::{ArenaWatermark, TypeArena};
pub(crate) use assignment::AssignmentError;
pub(crate) use basic::BasicKind;
pub(crate) use builtin::{AppendError, CopyError, MakeError, ProcedureError};
pub(crate) use complex::ComplexError;
pub(crate) use conversion::ConversionError;
pub(crate) use format::Qualifier;
pub(crate) use index::{IndexError, IndexValueError, SliceError};
use interface::Method;
pub(crate) use layout::{FieldLayout, Layout, StructLayout, align_to};
pub(crate) use operand::{BinaryError, ConstAssignError, Constant, Mode, Operand};
pub(crate) use ordered::MinMaxError;
pub(crate) use parameters::LocalFormal;
pub(crate) use selection::{PromotionIndices, Selection, SelectionKind};
use structure::Field;
pub(crate) use ty::TypeId;
use ty::{Direction, Ty};

/// Only an established invalid operand produces a compiler diagnostic.
#[derive(Clone, Copy)]
pub(crate) enum CheckError<E> {
    Unknown,
    Invalid(E),
}

impl<E> CheckError<E> {
    fn map<F>(self, invalid: impl FnOnce(E) -> F) -> CheckError<F> {
        match self {
            Self::Unknown => CheckError::Unknown,
            Self::Invalid(error) => CheckError::Invalid(invalid(error)),
        }
    }
}

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
