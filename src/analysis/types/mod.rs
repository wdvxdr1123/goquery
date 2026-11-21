//! Query-local type construction, operands and bounded lazy type relations.
mod basic;
mod bigint;
mod builtin;
mod complex;
mod constant;
mod constraints;
mod declaration;
mod eval;
mod form;
mod format;
mod generics;
mod index;
mod interface;
mod iteration;
mod lookup;
mod lower;
mod method_set;
mod named;
mod operand;
mod ordered;
mod parameters;
mod pool;
mod predicates;
mod schema;
mod selection;
mod sets;
mod signature;
mod signature_names;
mod sizes;
mod store;
mod structure;
mod ty;
mod unify;

pub(crate) use basic::BasicKind;
pub(crate) use builtin::{AppendError, CopyError, MakeError, ProcedureError};
pub(crate) use complex::ComplexError;
pub(crate) use constant::{ConstAssignError, Constant};
#[allow(unused_imports)]
pub(crate) use eval::{DepthGuard, DepthKind, ModeGuard, TypeEval};
pub(crate) use format::Qualifier;
pub(crate) use index::{IndexError, IndexValueError, SliceError};
use interface::Method;
use lookup::MethodLookup;
pub(crate) use operand::{BinaryError, Mode, Operand};
pub(crate) use ordered::MinMaxError;
pub(crate) use parameters::LocalFormal;
pub(crate) use predicates::{AssignmentError, ConversionError};
pub(crate) use selection::{PromotionIndices, Selection, SelectionKind};
pub(crate) use sizes::{FieldLayout, Layout, StructLayout, align_to};
pub(crate) use store::TypeStore as TypeArena;
pub(crate) use store::TypeStore;
#[allow(unused_imports)]
pub(crate) use store::{AllocTierGuard, ProjectionGuard};
use structure::Field;
pub(crate) use ty::{CheckOutcome, TypeId, UnknownReason};
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

use crate::analysis::engine::budget::{MAX_BYTES, MAX_DEPTH, MAX_NODES};

pub(crate) use form::{TypeForm, type_form_in_scope};
pub(crate) use generics::completion_inferable;
pub(crate) use lower::{declaration_body, unparenthesized_type};
pub(crate) use signature_names::CallableNames;

pub(crate) use declaration::{LocalTypeDefinition, TypeDeclaration};
