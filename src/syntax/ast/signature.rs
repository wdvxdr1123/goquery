//! Query-local function signatures; names and types borrow source by byte span.
use crate::syntax::ast::expression::{Diagnostic, Expr};
use crate::syntax::ast::types::TypeReference;
use std::ops::Range;

/// One parameter after distributing grouped names over their shared type.
#[derive(Debug)]
#[non_exhaustive]
pub struct Parameter {
    pub name: Option<Range<usize>>,
    pub annotation: Range<usize>,
    pub variadic: bool,
}

/// A named function or method header, without its body or owned source text.
#[derive(Debug)]
#[non_exhaustive]
pub struct Signature {
    pub receiver: Vec<Parameter>,
    pub type_parameters: Vec<Parameter>,
    pub parameters: Vec<Parameter>,
    pub results: Vec<Parameter>,
    pub(crate) diagnostics: Vec<Diagnostic>,
    pub type_references: Vec<TypeReference>,
    pub(crate) expressions: Vec<Expr>,
}

impl Signature {
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    pub fn into_diagnostics(self) -> Vec<Diagnostic> {
        self.diagnostics
    }

    pub fn expressions(&self) -> &[Expr] {
        &self.expressions
    }
}
