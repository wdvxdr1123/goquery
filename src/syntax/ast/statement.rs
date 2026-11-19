use crate::syntax::ast::expression::{Diagnostic, Expr, ExprId};
use crate::syntax::ast::signature::Parameter;
use crate::syntax::ast::types::TypeReference;
use crate::syntax::token::TokenKind;
use std::ops::Range;

/// An index into a query-local flat statement arena.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StmtId(pub(crate) usize);

impl StmtId {
    pub fn index(self) -> usize {
        self.0
    }
}

/// Statements preserve lexical block boundaries without retaining source text.
#[derive(Debug)]
#[non_exhaustive]
pub enum StmtKind {
    Block {
        statements: Vec<StmtId>,
    },
    Expression {
        expression: ExprId,
    },
    Assignment {
        left: Vec<ExprId>,
        operator: Range<usize>,
        right: Vec<ExprId>,
    },
    Return {
        values: Vec<ExprId>,
    },
    Increment {
        expression: ExprId,
        operator: Range<usize>,
    },
    Send {
        channel: ExprId,
        value: ExprId,
    },
    Go {
        call: ExprId,
    },
    Defer {
        call: ExprId,
    },
    Declaration {
        keyword: Range<usize>,
        names: Vec<Range<usize>>,
        annotation: Option<Range<usize>>,
        values: Vec<ExprId>,
    },
    Missing,
    DeclarationGroup {
        declarations: Vec<StmtId>,
    },
    Select {
        cases: Vec<StmtId>,
    },
    CommunicationCase {
        communication: Option<StmtId>,
        statements: Vec<StmtId>,
    },
    RangeClause {
        left: Vec<ExprId>,
        operator: Option<Range<usize>>,
        value: ExprId,
    },
    RangeFor {
        clause: StmtId,
        body: StmtId,
    },
    Branch {
        keyword: TokenKind,
        label: Option<Range<usize>>,
    },
    Label {
        name: Range<usize>,
        statement: Option<StmtId>,
    },
    If {
        init: Option<StmtId>,
        condition: ExprId,
        then_block: StmtId,
        otherwise: Option<StmtId>,
    },
    For {
        init: Option<StmtId>,
        condition: Option<ExprId>,
        post: Option<StmtId>,
        body: StmtId,
    },
    Switch {
        init: Option<StmtId>,
        tag: Option<ExprId>,
        cases: Vec<StmtId>,
    },
    /// The guard statement retains the optional short-declared case binding.
    TypeSwitch {
        init: Option<StmtId>,
        guard: StmtId,
        cases: Vec<StmtId>,
    },
    Case {
        values: Vec<ExprId>,
        statements: Vec<StmtId>,
        default: bool,
    },
}

/// A statement and its byte range in the original source snapshot.
#[derive(Debug)]
#[non_exhaustive]
pub struct Statement {
    pub kind: StmtKind,
    pub span: Range<usize>,
}

/// On-demand body syntax with flat storage for both statements and expressions.
#[derive(Debug)]
#[non_exhaustive]
pub struct BlockTree {
    pub(crate) root: StmtId,
    pub(crate) statements: Vec<Statement>,
    pub(crate) expressions: Vec<Expr>,
    pub(crate) diagnostics: Vec<Diagnostic>,
    pub(crate) type_references: Vec<TypeReference>,
    pub(crate) type_parameters: Vec<LocalTypeParameters>,
    /// Delimiter offsets, sorted by statement ID. Most statements need no entry.
    pub(crate) delimiters: Vec<(StmtId, usize)>,
}

impl BlockTree {
    /// Reuse expression nodes as a checkable region without reparsing synthetic
    /// var/const text or shifting source coordinates.
    pub(crate) fn from_expression(tree: crate::syntax::ExpressionTree) -> Self {
        let span = tree
            .nodes
            .get(tree.root.index())
            .map(|expr| expr.span.clone())
            .unwrap_or(0..0);
        Self {
            root: StmtId(0),
            statements: vec![Statement {
                kind: StmtKind::Expression {
                    expression: tree.root,
                },
                span,
            }],
            expressions: tree.nodes,
            diagnostics: tree.diagnostics,
            type_references: tree.type_references,
            type_parameters: Vec::new(),
            delimiters: Vec::new(),
        }
    }

    pub fn root(&self) -> StmtId {
        self.root
    }

    pub fn statements(&self) -> &[Statement] {
        &self.statements
    }

    pub fn statement(&self, id: StmtId) -> Option<&Statement> {
        self.statements.get(id.index())
    }

    /// Returns the byte offset of a declaration's top-level `=`, if present.
    pub fn declaration_equals(&self, id: StmtId) -> Option<usize> {
        match self.statement(id)?.kind {
            StmtKind::Declaration { .. } => self.delimiter(id),
            _ => None,
        }
    }

    /// Returns the byte offset immediately after a case clause's colon.
    pub fn clause_body_start(&self, id: StmtId) -> Option<usize> {
        match self.statement(id)?.kind {
            StmtKind::Case { .. } | StmtKind::CommunicationCase { .. } => {
                self.delimiter(id).map(|colon| colon + 1)
            }
            _ => None,
        }
    }

    fn delimiter(&self, id: StmtId) -> Option<usize> {
        let index = self
            .delimiters
            .binary_search_by_key(&id.0, |(id, _)| id.0)
            .ok()?;
        Some(self.delimiters[index].1)
    }

    pub fn expressions(&self) -> &[Expr] {
        &self.expressions
    }

    pub fn expression(&self, id: ExprId) -> Option<&Expr> {
        self.expressions.get(id.index())
    }

    /// Recognizes a receive expression through any enclosing parentheses.
    /// The operator predicate reads the span in this tree's source snapshot.
    pub(crate) fn is_receive(
        &self,
        mut id: ExprId,
        is_arrow: impl FnOnce(&Range<usize>) -> bool,
    ) -> bool {
        while let Some(expr) = self.expression(id) {
            match &expr.kind {
                crate::syntax::ExprKind::Parenthesized { expression } => id = *expression,
                crate::syntax::ExprKind::Unary { operator, .. } => return is_arrow(operator),
                _ => return false,
            }
        }
        false
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

    pub fn type_parameters(&self) -> &[LocalTypeParameters] {
        &self.type_parameters
    }
}

/// Sparse generic declaration metadata, without enlarging every statement node.
#[derive(Debug)]
#[non_exhaustive]
pub struct LocalTypeParameters {
    pub declaration: StmtId,
    pub parameters: Vec<Parameter>,
}
