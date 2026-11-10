use crate::syntax::ast::expression::{ExprId, ExprKind};
use crate::syntax::parser::expression::Parser;
use crate::syntax::token::TokenKind;

impl Parser<'_> {
    fn colon(&mut self) -> bool {
        if self
            .tokens
            .peek()
            .is_some_and(|token| &self.source[token.span.clone()] == ":")
        {
            self.tokens.next();
            true
        } else {
            false
        }
    }

    fn bound(&mut self, depth: usize) -> Option<ExprId> {
        let token = self.tokens.peek()?;
        if matches!(token.kind, TokenKind::RBrack | TokenKind::Semicolon)
            || &self.source[token.span.clone()] == ":"
        {
            return None;
        }
        Some(self.unrestricted_expression(depth + 1))
    }

    pub(super) fn brackets(&mut self, operand: ExprId, depth: usize) -> ExprId {
        let opening = self.tokens.next().expect("peeked opening bracket");
        let first = self.bound(depth);
        let kind = if self.colon() {
            let high = self.bound(depth);
            let full = self.colon();
            let max = if full { self.bound(depth) } else { None };
            if full && (high.is_none() || max.is_none()) {
                self.error(
                    opening.span.clone(),
                    "full slice requires high and maximum bounds",
                );
            }
            ExprKind::Slice {
                operand,
                low: first,
                high,
                max,
                full,
            }
        } else {
            let mut arguments: Vec<_> = first.into_iter().collect();
            while self
                .tokens
                .peek()
                .is_some_and(|t| t.kind == TokenKind::Comma)
            {
                self.tokens.next();
                if self
                    .tokens
                    .peek()
                    .is_some_and(|t| t.kind == TokenKind::RBrack)
                {
                    break;
                }
                if let Some(argument) = self.bound(depth) {
                    arguments.push(argument);
                } else {
                    break;
                }
            }
            if arguments.is_empty() {
                self.error(opening.span.clone(), "expected index or type argument");
            }
            if self
                .tokens
                .peek()
                .is_some_and(|t| &self.source[t.span.clone()] == "...")
            {
                let ellipsis = self.tokens.next().expect("peeked ellipsis");
                self.error(ellipsis.span, "ellipsis is only valid in call arguments");
            }
            ExprKind::Index { operand, arguments }
        };
        let fallback = self
            .nodes
            .last()
            .map_or(opening.span.end, |n| n.span.end)
            .max(opening.span.end);
        let end = self.close(TokenKind::RBrack, "expected ']'", fallback);
        self.add(kind, self.nodes[operand.0].span.start..end)
    }

    pub(super) fn type_assertion(&mut self, operand: ExprId, depth: usize) -> ExprId {
        self.tokens.next().expect("peeked assertion parenthesis");
        if self
            .tokens
            .peek()
            .is_some_and(|t| &self.source[t.span.clone()] == "type")
        {
            let guard = self.tokens.next().expect("peeked type switch guard");
            let end = self.close(TokenKind::RParen, "expected ')'", guard.span.end);
            return self.add(
                ExprKind::TypeSwitchGuard { operand },
                self.nodes[operand.0].span.start..end,
            );
        }
        let asserted_type = self.type_syntax(depth + 1);
        let closing = self.close(TokenKind::RParen, "expected ')'", asserted_type.end);
        self.add(
            ExprKind::TypeAssertion {
                operand,
                asserted_type,
            },
            self.nodes[operand.0].span.start..closing,
        )
    }
}
