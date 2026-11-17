//! Bounded compiler diagnostics. Query adapters own only syntax and import edits.
mod declarations;
mod locals;
mod names;

pub(crate) use declarations::file_diagnostics;

use super::RegionChecker;
use crate::analysis::types::{Mode, Operand, TypeId};
use crate::syntax::ExprId;
use std::ops::Range;

impl RegionChecker<'_, '_> {
    /// A failed or budget-limited relation is unknown. Only an explicit false
    /// between validated, typed value operands proves an assignment error.
    pub(super) fn check_assignment(
        &mut self,
        source: Operand,
        target: TypeId,
        expr: ExprId,
        context: &str,
    ) -> Option<bool> {
        let int_bits = self.int_bits()?;
        let result = self.arena.assign_value(source, target, int_bits);
        if result != Some(false)
            || !source.mode.is_value()
            || !self.body.diagnostics().is_empty()
            || self
                .arena
                .basic_kind(source.ty)
                .is_some_and(|kind| kind.untyped())
            || self.arena.validate_type(source.ty).is_none()
            || self.arena.validate_type(target).is_none()
        {
            return result;
        }
        let Some(expression) = self.body.expression(expr) else {
            return result;
        };
        let text = self.text(&expression.span);
        if text.len() > 1024 {
            return result;
        }
        let Some(from) = self.arena.render_presentation(source.ty, self.file) else {
            return result;
        };
        let Some(to) = self.arena.render_presentation(target, self.file) else {
            return result;
        };
        let description = if source.mode == Mode::Variable {
            "variable of type"
        } else {
            "value of type"
        };
        self.region.push_diagnostic(Diagnostic {
            span: self.abs_range(&expression.span),
            kind: Kind::Compiler(Code::IncompatibleAssign),
            message: format!("cannot use {text} ({description} {from}) as {to} value in {context}"),
            unnecessary: false,
            related: Vec::new(),
        });
        result
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
