//! Bounded compiler diagnostics. Query adapters own only syntax and import edits.
mod arity;
mod declarations;
mod locals;
mod names;
mod operands;

pub(crate) use declarations::file_diagnostics;

use super::RegionChecker;
use std::ops::Range;

impl RegionChecker<'_, '_> {
    pub(super) fn compiler_error(&mut self, span: &Range<usize>, code: Code, message: &str) {
        if self.body.diagnostics().is_empty() {
            self.region.push_diagnostic(Diagnostic {
                span: self.abs_range(span),
                kind: Kind::Compiler(code),
                message: message.to_owned(),
                unnecessary: false,
                related: Vec::new(),
            });
        }
    }
}

/// Push diagnostics are bounded; the parser already truncates at this limit.
pub(crate) const MAX_DIAGNOSTICS: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Code {
    UndeclaredName,
    UndeclaredImportedName,
    UnusedVar,
    UnusedImport,
    DuplicateDecl,
    DuplicateMethod,
    IncompatibleAssign,
    InvalidCond,
    InvalidPostDecl,
    InvalidSelectCase,
    BadDecl,
    RepeatedDecl,
    UnassignableOperand,
    NoNewVar,
    MultiValAssignOp,
    WrongAssignCount,
    WrongResultCount,
    WrongArgCount,
    NonVariadicDotDotDot,
    TooManyValues,
    InvalidDotDotDot,
    InvalidLen,
    InvalidCap,
    InvalidMake,
    SwappedMakeArgs,
    InvalidAppend,
    InvalidCopy,
    InvalidClear,
    InvalidClose,
    InvalidDelete,
    InvalidComplex,
    InvalidReal,
    InvalidImag,
    InvalidMinMaxOperand,
    MismatchedTypes,
    NotAType,
    UntypedNilUse,
    NonIndexableOperand,
    NonSliceableOperand,
    InvalidSliceExpr,
    SwappedSliceIndices,
    InvalidIndex,
    NumericOverflow,
    TruncatedFloat,
    InvalidConstVal,
    InvalidConversion,
    DivByZero,
    InvalidShiftOperand,
    InvalidShiftCount,
}

impl Code {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::UndeclaredName => "UndeclaredName",
            Self::UndeclaredImportedName => "UndeclaredImportedName",
            Self::UnusedVar => "UnusedVar",
            Self::UnusedImport => "UnusedImport",
            Self::DuplicateDecl => "DuplicateDecl",
            Self::DuplicateMethod => "DuplicateMethod",
            Self::IncompatibleAssign => "IncompatibleAssign",
            Self::InvalidCond => "InvalidCond",
            Self::InvalidPostDecl => "InvalidPostDecl",
            Self::InvalidSelectCase => "InvalidSelectCase",
            Self::BadDecl => "BadDecl",
            Self::RepeatedDecl => "RepeatedDecl",
            Self::UnassignableOperand => "UnassignableOperand",
            Self::NoNewVar => "NoNewVar",
            Self::MultiValAssignOp => "MultiValAssignOp",
            Self::WrongAssignCount => "WrongAssignCount",
            Self::WrongResultCount => "WrongResultCount",
            Self::WrongArgCount => "WrongArgCount",
            Self::NonVariadicDotDotDot => "NonVariadicDotDotDot",
            Self::TooManyValues => "TooManyValues",
            Self::InvalidDotDotDot => "InvalidDotDotDot",
            Self::InvalidLen => "InvalidLen",
            Self::InvalidCap => "InvalidCap",
            Self::InvalidMake => "InvalidMake",
            Self::SwappedMakeArgs => "SwappedMakeArgs",
            Self::InvalidAppend => "InvalidAppend",
            Self::InvalidCopy => "InvalidCopy",
            Self::InvalidClear => "InvalidClear",
            Self::InvalidClose => "InvalidClose",
            Self::InvalidDelete => "InvalidDelete",
            Self::InvalidComplex => "InvalidComplex",
            Self::InvalidReal => "InvalidReal",
            Self::InvalidImag => "InvalidImag",
            Self::InvalidMinMaxOperand => "InvalidMinMaxOperand",
            Self::MismatchedTypes => "MismatchedTypes",
            Self::NotAType => "NotAType",
            Self::UntypedNilUse => "UntypedNilUse",
            Self::NonIndexableOperand => "NonIndexableOperand",
            Self::NonSliceableOperand => "NonSliceableOperand",
            Self::InvalidSliceExpr => "InvalidSliceExpr",
            Self::SwappedSliceIndices => "SwappedSliceIndices",
            Self::InvalidIndex => "InvalidIndex",
            Self::NumericOverflow => "NumericOverflow",
            Self::TruncatedFloat => "TruncatedFloat",
            Self::InvalidConstVal => "InvalidConstVal",
            Self::InvalidConversion => "InvalidConversion",
            Self::DivByZero => "DivByZero",
            Self::InvalidShiftOperand => "InvalidShiftOperand",
            Self::InvalidShiftCount => "InvalidShiftCount",
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) enum Kind {
    Compiler(Code),
    Syntax,
}

/// A same-file related location, as reported by go/types for redeclarations.
#[derive(Clone)]
pub(crate) struct Related {
    pub(crate) span: Range<usize>,
    pub(crate) message: String,
}

#[derive(Clone)]
pub(crate) struct Diagnostic {
    pub(crate) span: Range<usize>,
    pub(crate) kind: Kind,
    pub(crate) message: String,
    pub(crate) unnecessary: bool,
    pub(crate) related: Vec<Related>,
}
