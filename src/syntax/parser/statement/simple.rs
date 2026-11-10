use crate::syntax::ast::statement::{StmtId, StmtKind};
use crate::syntax::parser::statement::block::BodyParser;
use crate::syntax::token::TokenKind;

impl BodyParser<'_> {
    pub(super) fn simple_statement(&mut self) -> StmtId {
        let Some(token) = self.parser.tokens.peek().cloned() else {
            let end = self.parser.source.len();
            self.parser.error(end..end, "expected simple statement");
            return self.add(StmtKind::Missing, end..end);
        };
        let left = self.expression_list();
        let mut end = self.parser.nodes[left.last().expect("nonempty expression list").0]
            .span
            .end;
        let next = self.parser.tokens.peek().cloned();
        let operator = next
            .as_ref()
            .map_or("", |t| &self.parser.source[t.span.clone()]);
        let kind = if matches!(
            operator,
            "=" | ":="
                | "+="
                | "-="
                | "*="
                | "/="
                | "%="
                | "<<="
                | ">>="
                | "&="
                | "|="
                | "^="
                | "&^="
        ) {
            let operator = self.parser.tokens.next().expect("assignment operator").span;
            if self.range_header
                && self
                    .parser
                    .tokens
                    .peek()
                    .is_some_and(|t| t.kind == TokenKind::Range)
            {
                self.parser.tokens.next();
                if left.len() > 2 || !matches!(&self.parser.source[operator.clone()], "=" | ":=") {
                    self.parser
                        .error(operator.clone(), "invalid range assignment");
                }
                let value = self.parser.expression(1, 0);
                let end = self.parser.nodes[value.0].span.end;
                return self.add(
                    StmtKind::RangeClause {
                        left,
                        operator: Some(operator),
                        value,
                    },
                    token.span.start..end,
                );
            }
            let right = self.expression_list();
            end = self.parser.nodes[right.last().expect("nonempty assignment values").0]
                .span
                .end;
            StmtKind::Assignment {
                left,
                operator,
                right,
            }
        } else if matches!(operator, "++" | "--") {
            let operator = self.parser.tokens.next().expect("increment operator").span;
            end = operator.end;
            if left.len() != 1 {
                self.parser
                    .error(token.span.clone(), "increment requires one operand");
            }
            StmtKind::Increment {
                expression: left[0],
                operator,
            }
        } else if operator == "<-" {
            self.parser.tokens.next();
            let value = self.parser.expression(1, 0);
            end = self.parser.nodes[value.0].span.end;
            if left.len() != 1 {
                self.parser
                    .error(token.span.clone(), "send requires one channel");
            }
            StmtKind::Send {
                channel: left[0],
                value,
            }
        } else {
            if left.len() != 1 {
                self.parser
                    .error(token.span.clone(), "unexpected expression list");
            }
            StmtKind::Expression {
                expression: left[0],
            }
        };
        self.add(kind, token.span.start..end)
    }
}
