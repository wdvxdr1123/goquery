use crate::syntax::ast::expression::{ExprId, ExprKind};
use crate::syntax::parser::expression::Parser;
use crate::syntax::token::TokenKind;

impl Parser<'_> {
    pub(super) fn composite(&mut self, literal_type: Option<ExprId>, depth: usize) -> ExprId {
        let opening = self.tokens.next().expect("peeked composite literal brace");
        if depth >= 128 {
            self.error(opening.span.clone(), "composite nesting limit exceeded");
            return self.add(ExprKind::Missing, opening.span);
        }
        let start = literal_type.map_or(opening.span.start, |id| self.nodes[id.0].span.start);
        let mut end = opening.span.end;
        let mut elements = Vec::new();
        while let Some(token) = self.tokens.peek() {
            if matches!(token.kind, TokenKind::RBrace | TokenKind::Semicolon) {
                break;
            }
            let mut element = self.literal_element(depth + 1);
            if self
                .tokens
                .peek()
                .is_some_and(|t| &self.source[t.span.clone()] == ":")
            {
                self.tokens.next();
                let value = self.literal_element(depth + 1);
                let span = self.nodes[element.0].span.start..self.nodes[value.0].span.end;
                element = self.add(
                    ExprKind::KeyValue {
                        key: element,
                        value,
                    },
                    span,
                );
            }
            end = self.nodes[element.0].span.end;
            elements.push(element);
            if self
                .tokens
                .peek()
                .is_some_and(|t| t.kind == TokenKind::Comma)
            {
                self.tokens.next();
            } else {
                break;
            }
        }
        let end = self.close(TokenKind::RBrace, "expected '}' in composite literal", end);
        self.add(
            ExprKind::Composite {
                literal_type,
                elements,
            },
            start..end,
        )
    }

    fn literal_element(&mut self, depth: usize) -> ExprId {
        if self
            .tokens
            .peek()
            .is_some_and(|t| t.kind == TokenKind::LBrace)
        {
            self.composite(None, depth)
        } else {
            self.unrestricted_expression(depth)
        }
    }
}
