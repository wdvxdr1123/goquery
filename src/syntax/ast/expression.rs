use crate::syntax::ast::types::TypeReference;
use crate::syntax::token::{Token, TokenKind};
use std::num::NonZeroU32;
use std::ops::Range;

/// An index into `ExpressionTree::nodes`; nodes are stored children-first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExprId(pub(crate) usize);

impl ExprId {
    pub fn index(self) -> usize {
        self.0
    }
}

#[derive(Debug)]
#[non_exhaustive]
/// Expression shape. Text (including operator spelling) remains in the source.
pub enum ExprKind {
    Name,
    Literal(TokenKind),
    Missing,
    /// A type used in expression syntax; semantic checks decide its valid use.
    Type,
    Composite {
        literal_type: Option<ExprId>,
        elements: Vec<ExprId>,
    },
    KeyValue {
        key: ExprId,
        value: ExprId,
    },
    /// Function bodies remain opaque until block syntax is requested.
    FunctionLiteral {
        signature: Range<usize>,
        body: Range<usize>,
    },
    Unary {
        operator: Range<usize>,
        operand: ExprId,
    },
    Binary {
        left: ExprId,
        operator: Range<usize>,
        right: ExprId,
    },
    Parenthesized {
        expression: ExprId,
    },
    Selector {
        operand: ExprId,
        field: Range<usize>,
    },
    Call {
        function: ExprId,
        arguments: Vec<ExprId>,
        variadic: bool,
        /// Byte offset of the spread token. A spread follows an argument.
        ellipsis: Option<NonZeroU32>,
    },
    /// Syntax alone does not distinguish indexing from generic instantiation.
    Index {
        operand: ExprId,
        arguments: Vec<ExprId>,
    },
    Slice {
        operand: ExprId,
        low: Option<ExprId>,
        high: Option<ExprId>,
        max: Option<ExprId>,
        full: bool,
    },
    /// The asserted type is validated on demand and retained as a source span.
    TypeAssertion {
        operand: ExprId,
        asserted_type: Range<usize>,
    },
    /// A syntactic guard; statement parsing must restrict it to a type switch.
    TypeSwitchGuard {
        operand: ExprId,
    },
}

#[derive(Debug)]
#[non_exhaustive]
/// One arena node covering a UTF-8 byte range in the original source.
pub struct Expr {
    pub kind: ExprKind,
    pub span: Range<usize>,
}

#[cfg(target_pointer_width = "64")]
const _: () = assert!(std::mem::size_of::<Expr>() <= 80);

#[derive(Debug)]
#[non_exhaustive]
/// A recoverable parser error referring to the same source snapshot as the tree.
pub struct Diagnostic {
    pub span: Range<usize>,
    pub message: &'static str,
    pub(crate) expected: Option<Expected>,
}

#[derive(Debug)]
pub(crate) struct Expected {
    pub(crate) description: &'static str,
    pub(crate) found: Option<Token>,
}

impl Diagnostic {
    /// Format an expectation using the token already observed by the parser.
    pub(crate) fn display_message(&self, source: &str) -> String {
        let Some(expected) = &self.expected else {
            return self.message.to_owned();
        };
        let found = match &expected.found {
            None => "'EOF'".to_owned(),
            Some(token) if token.inserted => "newline".to_owned(),
            Some(token)
                if matches!(
                    token.kind,
                    TokenKind::Ident
                        | TokenKind::Int
                        | TokenKind::Float
                        | TokenKind::Imag
                        | TokenKind::Char
                        | TokenKind::String
                ) =>
            {
                source[token.span.clone()].to_owned()
            }
            Some(token) => format!("'{}'", &source[token.span.clone()]),
        };
        format!("{}, found {found}", expected.description)
    }

    pub(crate) fn rebase(&mut self, base: usize) {
        self.span = base + self.span.start..base + self.span.end;
        if let Some(token) = self.expected.as_mut().and_then(|e| e.found.as_mut()) {
            token.span = base + token.span.start..base + token.span.end;
        }
    }
}
#[derive(Debug)]
#[non_exhaustive]
/// Owns flat nodes without owning source text or recursively boxed subtrees.
pub struct ExpressionTree {
    pub(crate) nodes: Vec<Expr>,
    pub(crate) root: ExprId,
    pub(crate) diagnostics: Vec<Diagnostic>,
    pub(crate) type_references: Vec<TypeReference>,
}

impl ExpressionTree {
    pub fn root(&self) -> ExprId {
        self.root
    }

    pub fn nodes(&self) -> &[Expr] {
        &self.nodes
    }

    pub fn node(&self, id: ExprId) -> Option<&Expr> {
        self.nodes.get(id.index())
    }

    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    pub fn into_diagnostics(self) -> Vec<Diagnostic> {
        self.diagnostics
    }

    pub fn type_references(&self) -> &[TypeReference] {
        &self.type_references
    }
}
