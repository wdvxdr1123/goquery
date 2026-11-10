use crate::syntax::ast::expression::{ExprId, ExprKind};
use crate::syntax::parser::expression::Parser;
use crate::syntax::token::TokenKind;

impl Parser<'_> {
    pub(super) fn postfix(&mut self, mut operand: ExprId, depth: usize) -> ExprId {
        while let Some(token) = self.tokens.peek() {
            let start = self.nodes[operand.0].span.start;
            match token.kind {
                TokenKind::LBrace
                    if matches!(
                        self.nodes[operand.0].kind,
                        ExprKind::Name
                            | ExprKind::Selector { .. }
                            | ExprKind::Index { .. }
                            | ExprKind::Type
                    ) =>
                {
                    if self.restricted_header
                        && !matches!(self.nodes[operand.0].kind, ExprKind::Type)
                    {
                        break;
                    }
                    if matches!(self.nodes[operand.0].kind, ExprKind::Type) {
                        let spelling = &self.source[self.nodes[operand.0].span.clone()];
                        if !spelling.starts_with('[')
                            && !spelling.starts_with("map")
                            && !spelling.starts_with("struct")
                        {
                            break;
                        }
                    }
                    operand = self.composite(Some(operand), depth + 1);
                }
                TokenKind::Period => {
                    let dot = self.tokens.next().expect("peeked dot");
                    if self
                        .tokens
                        .peek()
                        .is_some_and(|t| t.kind == TokenKind::LParen)
                    {
                        operand = self.type_assertion(operand, depth);
                        continue;
                    }
                    let field = if self
                        .tokens
                        .peek()
                        .is_some_and(|t| t.kind == TokenKind::Ident)
                    {
                        self.tokens.next().expect("peeked selector field").span
                    } else {
                        self.error(dot.span.clone(), "expected selector name");
                        dot.span.end..dot.span.end
                    };
                    let end = field.end;
                    operand = self.add(ExprKind::Selector { operand, field }, start..end);
                }
                TokenKind::LBrack => {
                    operand = self.brackets(operand, depth);
                }
                TokenKind::LParen => {
                    let opening = self.tokens.next().expect("peeked opening delimiter");
                    let closing = TokenKind::RParen;
                    let mut arguments = Vec::new();
                    let mut variadic = false;
                    while let Some(next) = self.tokens.peek() {
                        if next.kind == closing || next.kind == TokenKind::Semicolon {
                            break;
                        }
                        let before = next.span.start;
                        if variadic {
                            let span = next.span.clone();
                            self.error(span, "variadic argument must be last");
                        }
                        arguments.push(self.unrestricted_expression(depth + 1));
                        if self
                            .tokens
                            .peek()
                            .is_some_and(|t| &self.source[t.span.clone()] == "...")
                        {
                            self.tokens.next();
                            variadic = true;
                        }
                        if !self
                            .tokens
                            .peek()
                            .is_some_and(|t| t.kind == TokenKind::Comma)
                        {
                            break;
                        }
                        self.tokens.next();
                        if self.tokens.peek().is_some_and(|t| t.span.start <= before) {
                            break;
                        }
                    }
                    let end = arguments
                        .last()
                        .map_or(opening.span.end, |id| self.nodes[id.0].span.end);
                    let end = self.close(closing, "expected ')'", end);
                    let kind = ExprKind::Call {
                        function: operand,
                        arguments,
                        variadic,
                    };
                    operand = self.add(kind, start..end);
                }
                _ => break,
            }
        }
        operand
    }
}
