use crate::syntax::ast::expression::{ExprId, ExprKind};
use crate::syntax::ast::statement::{BlockTree, LocalTypeParameters, Statement, StmtId, StmtKind};
use crate::syntax::parser::expression::Parser;
use crate::syntax::token::TokenKind;
use std::ops::Range;

/// Parses a body beginning with `{`. This initial statement grammar supports
/// nested blocks, simple statements, returns and single local declarations.
/// Unsupported syntax is diagnosed and recovered at a statement boundary.
pub fn parse_block(source: &str) -> BlockTree {
    let mut body = BodyParser {
        parser: Parser::new(source),
        statements: Vec::new(),
        type_parameters: Vec::new(),
        range_header: false,
    };
    let root = body.block(0);
    while body.eat(TokenKind::Semicolon) {}
    if let Some(token) = body.parser.tokens.next() {
        body.parser
            .error(token.span, "unexpected token after block");
    }
    BlockTree {
        root,
        statements: body.statements,
        expressions: body.parser.nodes,
        diagnostics: body.parser.diagnostics,
        type_references: body.parser.type_references,
        type_parameters: body.type_parameters,
    }
}

/// Reuse declaration grammar without keeping its statement arena between specs.
pub(in crate::syntax::parser) fn declaration_parser(parser: Parser<'_>) -> Parser<'_> {
    let mut body = BodyParser {
        parser,
        statements: Vec::new(),
        type_parameters: Vec::new(),
        range_header: false,
    };
    body.declaration();
    body.parser
}

pub(super) struct BodyParser<'a> {
    pub(super) range_header: bool,
    pub(super) parser: Parser<'a>,
    pub(super) statements: Vec<Statement>,
    pub(super) type_parameters: Vec<LocalTypeParameters>,
}

impl BodyParser<'_> {
    pub(super) fn add(&mut self, kind: StmtKind, span: Range<usize>) -> StmtId {
        let id = StmtId(self.statements.len());
        self.statements.push(Statement { kind, span });
        id
    }

    pub(super) fn eat(&mut self, kind: TokenKind) -> bool {
        if self.parser.tokens.peek().is_some_and(|t| t.kind == kind) {
            self.parser.tokens.next();
            true
        } else {
            false
        }
    }

    pub(super) fn block(&mut self, depth: usize) -> StmtId {
        let start = self
            .parser
            .tokens
            .peek()
            .map_or(self.parser.source.len(), |t| t.span.start);
        if depth >= 128 || !self.eat(TokenKind::LBrace) {
            self.parser.error(
                start..start,
                "expected block or block nesting limit exceeded",
            );
            return self.add(StmtKind::Missing, start..start);
        }
        let mut statements = Vec::new();
        while let Some(token) = self.parser.tokens.peek() {
            if token.kind == TokenKind::RBrace {
                break;
            }
            if self.eat(TokenKind::Semicolon) {
                continue;
            }
            let before = self
                .parser
                .tokens
                .peek()
                .expect("statement token")
                .span
                .start;
            statements.push(self.statement(depth + 1));
            if self.eat(TokenKind::Semicolon) {
                continue;
            }
            if self
                .parser
                .tokens
                .peek()
                .is_some_and(|t| t.kind == TokenKind::RBrace)
            {
                break;
            }
            let span = self
                .parser
                .tokens
                .peek()
                .map_or(self.parser.source.len()..self.parser.source.len(), |t| {
                    t.span.clone()
                });
            self.parser.error(span, "expected statement separator");
            // Always make progress, including for unsupported control flow.
            while let Some(next) = self.parser.tokens.peek() {
                if matches!(next.kind, TokenKind::Semicolon | TokenKind::RBrace)
                    && next.span.start > before
                {
                    break;
                }
                self.parser.tokens.next();
            }
        }
        let end = self.parser.close(
            TokenKind::RBrace,
            "expected '}' after block",
            self.parser.source.len(),
        );
        self.add(StmtKind::Block { statements }, start..end)
    }

    pub(super) fn expression_list(&mut self) -> Vec<ExprId> {
        let mut values = vec![self.parser.expression(1, 0)];
        while self.eat(TokenKind::Comma) {
            values.push(self.parser.expression(1, 0));
        }
        values
    }

    pub(super) fn statement(&mut self, depth: usize) -> StmtId {
        let Some(token) = self.parser.tokens.peek().cloned() else {
            let end = self.parser.source.len();
            self.parser.error(end..end, "expected statement");
            return self.add(StmtKind::Missing, end..end);
        };
        if token.kind == TokenKind::RBrace {
            self.parser.error(token.span.clone(), "expected statement");
            return self.add(StmtKind::Missing, token.span.start..token.span.start);
        }
        if token.kind == TokenKind::Select {
            return self.select(depth);
        }
        if matches!(
            token.kind,
            TokenKind::If | TokenKind::For | TokenKind::Switch
        ) {
            return self.control(depth);
        }
        if matches!(
            token.kind,
            TokenKind::Break | TokenKind::Continue | TokenKind::Goto | TokenKind::Fallthrough
        ) {
            self.parser.tokens.next();
            let label = if token.kind != TokenKind::Fallthrough
                && self
                    .parser
                    .tokens
                    .peek()
                    .is_some_and(|t| t.kind == TokenKind::Ident)
            {
                Some(self.parser.tokens.next().expect("branch label").span)
            } else {
                None
            };
            if token.kind == TokenKind::Goto && label.is_none() {
                self.parser.error(token.span.clone(), "goto requires label");
            }
            let end = label.as_ref().map_or(token.span.end, |l| l.end);
            return self.add(
                StmtKind::Branch {
                    keyword: token.kind,
                    label,
                },
                token.span.start..end,
            );
        }
        if token.kind == TokenKind::Ident
            && self
                .parser
                .tokens
                .clone()
                .nth(1)
                .is_some_and(|t| t.kind == TokenKind::Colon)
        {
            self.parser.tokens.next();
            self.parser.tokens.next();
            if depth >= 128 {
                self.parser
                    .error(token.span.clone(), "label nesting limit exceeded");
                return self.add(StmtKind::Missing, token.span);
            }
            let statement = if self
                .parser
                .tokens
                .peek()
                .is_none_or(|t| matches!(t.kind, TokenKind::RBrace | TokenKind::Semicolon))
            {
                None
            } else {
                Some(self.statement(depth + 1))
            };
            // Like go/ast, a label covers its implicit/explicit empty statement:
            // `L: ;` ends after the semicolon and `L:` before `}` ends at the
            // closing brace (implicit empty statements end at their token).
            let end = statement.map_or_else(
                || {
                    self.parser
                        .tokens
                        .peek()
                        .map_or(token.span.end, |next| match next.kind {
                            TokenKind::Semicolon => next.span.end,
                            TokenKind::RBrace => next.span.start,
                            _ => token.span.end,
                        })
                },
                |id| self.statements[id.0].span.end,
            );
            return self.add(
                StmtKind::Label {
                    name: token.span.clone(),
                    statement,
                },
                token.span.start..end,
            );
        }
        if token.kind == TokenKind::LBrace {
            return self.block(depth);
        }
        let spelling = &self.parser.source[token.span.clone()];
        if matches!(spelling, "var" | "const" | "type") {
            return self.declaration();
        }
        if spelling == "return" {
            self.parser.tokens.next();
            let values = if self
                .parser
                .tokens
                .peek()
                .is_none_or(|t| matches!(t.kind, TokenKind::Semicolon | TokenKind::RBrace))
            {
                Vec::new()
            } else {
                self.expression_list()
            };
            let end = values
                .last()
                .map_or(token.span.end, |id| self.parser.nodes[id.0].span.end);
            return self.add(StmtKind::Return { values }, token.span.start..end);
        }
        if matches!(spelling, "go" | "defer") {
            self.parser.tokens.next();
            let call = self.parser.expression(1, 0);
            if !matches!(self.parser.nodes[call.0].kind, ExprKind::Call { .. }) {
                self.parser.error(
                    self.parser.nodes[call.0].span.clone(),
                    "go/defer requires an unparenthesized call",
                );
            }
            let end = self.parser.nodes[call.0].span.end;
            let kind = if spelling == "go" {
                StmtKind::Go { call }
            } else {
                StmtKind::Defer { call }
            };
            return self.add(kind, token.span.start..end);
        }
        self.simple_statement()
    }
}
