use crate::syntax::ast::statement::{StmtId, StmtKind};
use crate::syntax::parser::statement::block::BodyParser;
use crate::syntax::token::TokenKind;

impl BodyParser<'_> {
    pub(super) fn select(&mut self, depth: usize) -> StmtId {
        let keyword = self.parser.tokens.next().expect("select keyword");
        if depth >= 128 || !self.eat(TokenKind::LBrace) {
            self.parser.error(
                keyword.span.clone(),
                "expected select body or nesting limit exceeded",
            );
            return self.add(StmtKind::Missing, keyword.span);
        }
        let mut cases = Vec::new();
        while let Some(token) = self.parser.tokens.peek().cloned() {
            if token.kind == TokenKind::RBrace {
                break;
            }
            if self.eat(TokenKind::Semicolon) {
                continue;
            }
            if !matches!(token.kind, TokenKind::Case | TokenKind::Default) {
                self.parser.error(token.span, "expected communication case");
                self.parser.tokens.next();
                continue;
            }
            self.parser.tokens.next();
            let communication = if token.kind == TokenKind::Default {
                None
            } else {
                let statement = self.simple_statement();
                let valid = match &self.statements[statement.0].kind {
                    StmtKind::Send { .. } => true,
                    StmtKind::Expression { .. } => true,
                    StmtKind::Assignment {
                        left,
                        operator,
                        right,
                    } => {
                        left.len() <= 2
                            && right.len() == 1
                            && matches!(&self.parser.source[operator.clone()], "=" | ":=")
                    }
                    _ => false,
                };
                if !valid {
                    self.parser.error(
                        self.statements[statement.0].span.clone(),
                        "select case requires send or receive",
                    );
                }
                Some(statement)
            };
            if !self.eat(TokenKind::Colon) {
                self.parser
                    .error(token.span.clone(), "expected ':' after communication case");
            }
            let (statements, end) = self.case_body(depth + 1, token.span.end);
            let end = self
                .parser
                .tokens
                .peek()
                .map_or(self.parser.source.len(), |token| token.span.start)
                .max(end);
            cases.push(self.add(
                StmtKind::CommunicationCase {
                    communication,
                    statements,
                },
                token.span.start..end,
            ));
        }
        let end = self.parser.close(
            TokenKind::RBrace,
            "expected '}' after select",
            self.parser.source.len(),
        );
        self.add(StmtKind::Select { cases }, keyword.span.start..end)
    }
}
