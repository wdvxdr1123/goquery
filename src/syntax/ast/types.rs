//! Type references are source spans, without a retained recursive type tree.
use crate::syntax::ast::expression::{Diagnostic, Expr};
use std::ops::Range;

/// A type-name occurrence with an optional file-local import qualifier.
#[derive(Debug)]
#[non_exhaustive]
pub struct TypeReference {
    pub qualifier: Option<Range<usize>>,
    pub name: Range<usize>,
}

/// A name declared inside a function type or interface method signature.
#[derive(Debug)]
#[non_exhaustive]
pub struct SignatureName {
    pub name: Range<usize>,
    pub kind: SignatureNameKind,
    pub annotation: Range<usize>,
    pub variadic: bool,
}

/// Input parameters and named results have distinct semantic categories.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SignatureNameKind {
    Parameter,
    Result,
}

/// A struct field, with its containing opening-brace byte offset.
#[derive(Debug)]
#[non_exhaustive]
pub struct TypeField {
    /// Explicit field name, or the unqualified written embedded type name.
    pub name: Range<usize>,
    pub annotation: Range<usize>,
    pub container: usize,
    pub embedded: bool,
    /// Undecoded string literal, shared by all names in a grouped declaration.
    pub tag: Option<Range<usize>>,
}

/// An explicit interface method or embedded type/type term, with byte spans.
#[derive(Debug)]
#[non_exhaustive]
pub struct InterfaceMember {
    pub name: Option<Range<usize>>,
    /// Method parameter/result signature, or the complete embedded type term.
    pub annotation: Range<usize>,
    pub container: usize,
}

/// On-demand type syntax, including expressions used as array lengths.
#[derive(Debug)]
#[non_exhaustive]
pub struct TypeSyntax {
    pub span: Range<usize>,
    pub references: Vec<TypeReference>,
    pub signature_names: Vec<SignatureName>,
    pub fields: Vec<TypeField>,
    pub interface_members: Vec<InterfaceMember>,
    pub(crate) expressions: Vec<Expr>,
    pub(crate) diagnostics: Vec<Diagnostic>,
}

impl TypeSyntax {
    pub fn expressions(&self) -> &[Expr] {
        &self.expressions
    }

    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    pub fn into_diagnostics(self) -> Vec<Diagnostic> {
        self.diagnostics
    }
}
