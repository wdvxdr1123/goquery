use crate::syntax::ast::statement::{LocalTypeParameters, StmtId, StmtKind};
use crate::syntax::parser::signature::ParameterContext;
use crate::syntax::parser::statement::block::BodyParser;
use crate::syntax::token::{Token, TokenKind};

impl BodyParser<'_> {
    pub(super) fn declaration(&mut self) -> StmtId {
        let keyword = self.parser.tokens.next().expect("declaration keyword");
        if self.eat(TokenKind::LParen) {
            let mut declarations = Vec::new();
            while let Some(token) = self.parser.tokens.peek().cloned() {
                if token.kind == TokenKind::RParen {
                    break;
                }
                if self.eat(TokenKind::Semicolon) {
                    self.parser.error(token.span, "expected declaration spec");
                    continue;
                }
                declarations.push(self.declaration_spec(&keyword));
                if self.eat(TokenKind::Semicolon) {
                    continue;
                }
                if self
                    .parser
                    .tokens
                    .peek()
                    .is_some_and(|t| t.kind == TokenKind::RParen)
                {
                    break;
                }
                self.parser
                    .error(token.span, "expected grouped declaration separator");
                self.parser.tokens.next();
            }
            let end = self.parser.close(
                TokenKind::RParen,
                "expected ')' after declarations",
                keyword.span.end,
            );
            return self.add(
                StmtKind::DeclarationGroup { declarations },
                keyword.span.start..end,
            );
        }
        let declaration = self.declaration_spec(&keyword);
        self.statements[declaration.0].span.start = keyword.span.start;
        declaration
    }

    fn declaration_spec(&mut self, keyword: &Token) -> StmtId {
        let start = self
            .parser
            .tokens
            .peek()
            .map_or(keyword.span.end, |token| token.span.start);
        let spelling = &self.parser.source[keyword.span.clone()];
        let mut names = Vec::new();
        loop {
            if let Some(name) = self
                .parser
                .tokens
                .peek()
                .filter(|t| t.kind == TokenKind::Ident)
                .cloned()
            {
                self.parser.tokens.next();
                names.push(name.span);
            } else {
                self.parser
                    .error(keyword.span.clone(), "expected local declaration name");
                break;
            }
            if !self.eat(TokenKind::Comma) {
                break;
            }
        }
        let mut end = names.last().map_or(keyword.span.end, |name| name.end);
        if spelling == "type" {
            if names.len() != 1 {
                self.parser
                    .error(keyword.span.clone(), "type declaration requires one name");
            }
            let type_parameters = self
                .parser
                .tokens
                .peek()
                .filter(|token| token.kind == TokenKind::LBrack)
                .and_then(|token| {
                    crate::syntax::parser::type_parameter_end(
                        &self.parser.source[token.span.start..],
                    )
                });
            let type_parameters = type_parameters.map(|_| {
                self.parser
                    .parameter_list(ParameterContext::TypeParameters, 0)
                    .0
            });
            if self
                .parser
                .tokens
                .peek()
                .is_some_and(|t| &self.parser.source[t.span.clone()] == "=")
            {
                self.parser.tokens.next();
            }
            let annotation = self.parser.type_syntax(0);
            end = annotation.end;
            let declaration = self.add(
                StmtKind::Declaration {
                    keyword: keyword.span.clone(),
                    names,
                    annotation: Some(annotation),
                    values: Vec::new(),
                },
                start..end,
            );
            if let Some(parameters) = type_parameters {
                self.type_parameters.push(LocalTypeParameters {
                    declaration,
                    parameters,
                });
            }
            return declaration;
        }
        let annotation = if self.parser.tokens.peek().is_some_and(|t| {
            !matches!(
                t.kind,
                TokenKind::Semicolon | TokenKind::RBrace | TokenKind::RParen
            ) && &self.parser.source[t.span.clone()] != "="
        }) {
            let span = self.parser.type_syntax(0);
            end = span.end;
            Some(span)
        } else {
            None
        };
        let values = if self
            .parser
            .tokens
            .peek()
            .is_some_and(|t| &self.parser.source[t.span.clone()] == "=")
        {
            self.parser.tokens.next();
            self.expression_list()
        } else {
            Vec::new()
        };
        if let Some(value) = values.last() {
            end = self.parser.nodes[value.0].span.end;
        }
        if annotation.is_none() && values.is_empty() && keyword.kind != TokenKind::Const {
            self.parser.error(
                keyword.span.clone(),
                "expected type or initializer in declaration",
            );
        }
        self.add(
            StmtKind::Declaration {
                keyword: keyword.span.clone(),
                names,
                annotation,
                values,
            },
            start..end,
        )
    }
}
