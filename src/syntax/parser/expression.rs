//! Pratt expressions in a flat arena, parsed only when requested by a consumer.
use crate::syntax::ast::expression::{Diagnostic, Expr, ExprId, ExprKind, ExpressionTree};
use crate::syntax::lexer::Lexer;
use crate::syntax::token::TokenKind;
use std::iter::Peekable;
use std::ops::Range;

const MAX_EXPRESSION_DEPTH: usize = 128;
pub(super) const MAX_DIAGNOSTICS: usize = 4096;

/// Parses one expression. Invalid/incomplete input produces missing nodes and
/// diagnostics. Nesting is bounded to avoid exhausting the process stack.
pub fn parse_expression(source: &str) -> ExpressionTree {
    let mut parser = Parser::new(source);
    let root = parser.expression(1, 0);
    while parser
        .tokens
        .peek()
        .is_some_and(|token| token.kind == TokenKind::Semicolon)
    {
        parser.tokens.next();
    }
    if let Some(token) = parser.tokens.next() {
        parser.error(token.span, "unexpected token after expression");
    }
    ExpressionTree {
        nodes: parser.nodes,
        root,
        diagnostics: parser.diagnostics,
        type_references: parser.type_references,
    }
}

pub(super) struct Parser<'a> {
    pub(super) restricted_header: bool,
    pub(super) source: &'a str,
    pub(super) tokens: Peekable<Lexer<'a>>,
    pub(super) nodes: Vec<Expr>,
    pub(super) diagnostics: Vec<Diagnostic>,
    pub(super) type_references: Vec<crate::syntax::ast::types::TypeReference>,
    pub(super) signature_names: Vec<crate::syntax::ast::types::SignatureName>,
    pub(super) fields: Vec<crate::syntax::ast::types::TypeField>,
    pub(super) interface_members: Vec<crate::syntax::ast::types::InterfaceMember>,
}

impl<'a> Parser<'a> {
    pub(super) fn new(source: &'a str) -> Self {
        Self {
            source,
            tokens: Lexer::new(source).peekable(),
            nodes: Vec::new(),
            diagnostics: Vec::new(),
            type_references: Vec::new(),
            signature_names: Vec::new(),
            fields: Vec::new(),
            interface_members: Vec::new(),
            restricted_header: false,
        }
    }
}

impl Parser<'_> {
    pub(super) fn add(&mut self, kind: ExprKind, span: Range<usize>) -> ExprId {
        let id = ExprId(self.nodes.len());
        self.nodes.push(Expr { kind, span });
        id
    }

    pub(super) fn error(&mut self, span: Range<usize>, message: &'static str) {
        if self.diagnostics.len() < MAX_DIAGNOSTICS {
            self.diagnostics.push(Diagnostic { span, message });
        }
    }

    pub(super) fn expression(&mut self, minimum: u8, depth: usize) -> ExprId {
        if depth >= MAX_EXPRESSION_DEPTH {
            let span = self
                .tokens
                .next()
                .map_or(self.source.len()..self.source.len(), |t| t.span);
            self.error(span.clone(), "expression nesting limit exceeded");
            return self.add(ExprKind::Missing, span);
        }
        let mut left = self.prefix(depth);
        while let Some(token) = self.tokens.peek() {
            let precedence = token.kind.precedence();
            if precedence < minimum {
                break;
            }
            let operator = self.tokens.next().expect("peeked binary operator");
            let right = self.expression(precedence + 1, depth + 1);
            let span = self.nodes[left.0].span.start..self.nodes[right.0].span.end;
            left = self.add(
                ExprKind::Binary {
                    left,
                    operator: operator.span,
                    right,
                },
                span,
            );
        }
        left
    }

    fn prefix(&mut self, depth: usize) -> ExprId {
        let Some(token) = self.tokens.peek().cloned() else {
            let end = self.source.len();
            self.error(end..end, "expected expression");
            return self.add(ExprKind::Missing, end..end);
        };
        let spelling = &self.source[token.span.clone()];
        let expression = if matches!(
            token.kind,
            TokenKind::Add
                | TokenKind::Sub
                | TokenKind::Not
                | TokenKind::Xor
                | TokenKind::Mul
                | TokenKind::And
                | TokenKind::Arrow
                | TokenKind::Tilde
        ) {
            self.tokens.next();
            let operand = self.expression(6, depth + 1);
            let span = token.span.start..self.nodes[operand.0].span.end;
            self.add(
                ExprKind::Unary {
                    operator: token.span,
                    operand,
                },
                span,
            )
        } else {
            match token.kind {
                TokenKind::LBrack
                | TokenKind::Map
                | TokenKind::Struct
                | TokenKind::Interface
                | TokenKind::Chan
                | TokenKind::Func => {
                    let signature = self.type_syntax(depth + 1);
                    if spelling == "func"
                        && self
                            .tokens
                            .peek()
                            .is_some_and(|t| t.kind == TokenKind::LBrace)
                    {
                        let start = self.tokens.peek().expect("peeked function body").span.start;
                        let end = self.type_group(TokenKind::LBrace, TokenKind::RBrace);
                        let span = signature.start..end;
                        self.add(
                            ExprKind::FunctionLiteral {
                                signature,
                                body: start..end,
                            },
                            span,
                        )
                    } else {
                        self.add(ExprKind::Type, signature)
                    }
                }
                TokenKind::Ident => {
                    self.tokens.next();
                    self.add(ExprKind::Name, token.span)
                }
                TokenKind::Int
                | TokenKind::Float
                | TokenKind::Imag
                | TokenKind::String
                | TokenKind::Char => {
                    self.tokens.next();
                    self.add(ExprKind::Literal, token.span)
                }
                TokenKind::LParen => {
                    self.tokens.next();
                    let expression = self.unrestricted_expression(depth + 1);
                    let end = self.close(
                        TokenKind::RParen,
                        "expected ')'",
                        self.nodes[expression.0].span.end,
                    );
                    self.add(
                        ExprKind::Parenthesized { expression },
                        token.span.start..end,
                    )
                }
                _ => {
                    self.error(token.span.clone(), "expected expression");
                    // Preserve delimiters so callers can recover a missing argument.
                    if !matches!(
                        token.kind,
                        TokenKind::RParen
                            | TokenKind::RBrack
                            | TokenKind::Comma
                            | TokenKind::Semicolon
                    ) {
                        self.tokens.next();
                    }
                    self.add(ExprKind::Missing, token.span.start..token.span.start)
                }
            }
        };
        self.postfix(expression, depth)
    }

    pub(super) fn unrestricted_expression(&mut self, depth: usize) -> ExprId {
        let restricted = self.restricted_header;
        self.restricted_header = false;
        let result = self.expression(1, depth);
        self.restricted_header = restricted;
        result
    }

    pub(super) fn close(
        &mut self,
        kind: TokenKind,
        message: &'static str,
        fallback: usize,
    ) -> usize {
        if self.tokens.peek().is_some_and(|t| t.kind == kind) {
            return self
                .tokens
                .next()
                .expect("peeked closing delimiter")
                .span
                .end;
        }
        let span = self
            .tokens
            .peek()
            .map_or(self.source.len()..self.source.len(), |t| t.span.clone());
        self.error(span, message);
        fallback
    }
}
