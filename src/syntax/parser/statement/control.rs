use crate::syntax::ast::expression::{ExprId, ExprKind};
use crate::syntax::ast::statement::{StmtId, StmtKind};
use crate::syntax::parser::statement::block::BodyParser;
use crate::syntax::token::TokenKind;

impl BodyParser<'_> {
    /// Clauses admit only Go SimpleStmt (plus an optional range clause in the
    /// first `for` header), never declarations, labels, blocks or control flow.
    fn header_statement(&mut self, _depth: usize) -> StmtId {
        let old = self.parser.restricted_header;
        self.parser.restricted_header = true;
        let result = if self.range_header
            && self
                .parser
                .tokens
                .peek()
                .is_some_and(|token| token.kind == TokenKind::Range)
        {
            let keyword = self.parser.tokens.next().expect("range keyword");
            let value = self.parser.expression(1, 0);
            let end = self.parser.nodes[value.0].span.end;
            self.add(
                StmtKind::RangeClause {
                    left: Vec::new(),
                    operator: None,
                    value,
                },
                keyword.span.start..end,
            )
        } else {
            self.simple_statement()
        };
        self.parser.restricted_header = old;
        result
    }

    fn condition(&mut self, statement: StmtId) -> ExprId {
        if let StmtKind::Expression { expression } = self.statements[statement.0].kind {
            return expression;
        }
        let span = self.statements[statement.0].span.clone();
        self.parser
            .error(span.clone(), "expected condition expression");
        self.parser.add(ExprKind::Missing, span)
    }

    pub(super) fn control(&mut self, depth: usize) -> StmtId {
        let first = self.control_clause(depth);
        if !matches!(self.statements[first.0].kind, StmtKind::If { .. }) {
            return first;
        }
        // Else-if clauses are siblings for parser depth. Collect only IDs and
        // link the chain backwards; nested blocks retain their own depth limit.
        let mut clauses = vec![first];
        let mut otherwise = None;
        while self.eat(TokenKind::Else) {
            if self
                .parser
                .tokens
                .peek()
                .is_some_and(|token| token.kind == TokenKind::If)
            {
                let next = self.control_clause(depth);
                if !matches!(self.statements[next.0].kind, StmtKind::If { .. }) {
                    otherwise = Some(next);
                    break;
                }
                clauses.push(next);
            } else {
                otherwise = Some(self.block(depth + 1));
                break;
            }
        }
        for clause in clauses.into_iter().rev() {
            let end = otherwise.map(|id| self.statements[id.0].span.end);
            if let StmtKind::If {
                otherwise: branch, ..
            } = &mut self.statements[clause.0].kind
            {
                *branch = otherwise;
            }
            if let Some(end) = end {
                self.statements[clause.0].span.end = end;
            }
            otherwise = Some(clause);
        }
        first
    }

    fn control_clause(&mut self, depth: usize) -> StmtId {
        let keyword = self.parser.tokens.next().expect("control keyword");
        let keyword_kind = keyword.kind;
        if depth >= 128 {
            self.parser
                .error(keyword.span.clone(), "control nesting limit exceeded");
            return self.add(StmtKind::Missing, keyword.span);
        }
        let mut init = None;
        let mut type_guard = None;
        let mut condition = None;
        let mut post = None;
        let mut semicolon = self.eat(TokenKind::Semicolon);
        if !semicolon
            && !self
                .parser
                .tokens
                .peek()
                .is_some_and(|t| t.kind == TokenKind::LBrace)
        {
            let old = self.range_header;
            self.range_header = keyword_kind == TokenKind::For;
            let first = self.header_statement(depth + 1);
            self.range_header = old;
            if matches!(self.statements[first.0].kind, StmtKind::RangeClause { .. }) {
                let body = self.block(depth + 1);
                let end = self.statements[body.0].span.end;
                return self.add(
                    StmtKind::RangeFor {
                        clause: first,
                        body,
                    },
                    keyword.span.start..end,
                );
            }
            semicolon = self.eat(TokenKind::Semicolon);
            if semicolon {
                init = Some(first);
            } else if keyword_kind == TokenKind::Switch && self.is_type_guard(first) {
                type_guard = Some(first);
            } else {
                condition = Some(self.condition(first));
            }
        }
        if semicolon {
            if !self
                .parser
                .tokens
                .peek()
                .is_some_and(|t| matches!(t.kind, TokenKind::LBrace | TokenKind::Semicolon))
            {
                let header = self.header_statement(depth + 1);
                if keyword_kind == TokenKind::Switch && self.is_type_guard(header) {
                    type_guard = Some(header);
                } else {
                    condition = Some(self.condition(header));
                }
            }
            if keyword_kind == TokenKind::For {
                if !self.eat(TokenKind::Semicolon) {
                    self.parser
                        .error(keyword.span.clone(), "expected second for-clause semicolon");
                }
                if !self
                    .parser
                    .tokens
                    .peek()
                    .is_some_and(|t| t.kind == TokenKind::LBrace)
                {
                    post = Some(self.header_statement(depth + 1));
                }
            }
        }
        if keyword_kind == TokenKind::Switch {
            let (cases, end) = self.switch_cases(depth + 1);
            if let Some(guard) = type_guard {
                return self.add(
                    StmtKind::TypeSwitch { init, guard, cases },
                    keyword.span.start..end,
                );
            }
            return self.add(
                StmtKind::Switch {
                    init,
                    tag: condition,
                    cases,
                },
                keyword.span.start..end,
            );
        }
        let body = self.block(depth + 1);
        let end = self.statements[body.0].span.end;
        let kind = if keyword_kind == TokenKind::If {
            let condition = condition.unwrap_or_else(|| {
                self.parser
                    .error(keyword.span.clone(), "if requires condition");
                self.parser.add(ExprKind::Missing, keyword.span.clone())
            });
            StmtKind::If {
                init,
                condition,
                then_block: body,
                otherwise: None,
            }
        } else {
            StmtKind::For {
                init,
                condition,
                post,
                body,
            }
        };
        self.add(kind, keyword.span.start..end)
    }

    fn is_type_guard(&mut self, statement: StmtId) -> bool {
        let (expression, binding) = match &self.statements[statement.0].kind {
            StmtKind::Expression { expression } => (*expression, None),
            StmtKind::Assignment {
                left,
                operator,
                right,
            } if right.len() == 1 => (right[0], Some((left, operator))),
            _ => return false,
        };
        if !matches!(
            self.parser.nodes[expression.0].kind,
            ExprKind::TypeSwitchGuard { .. }
        ) {
            return false;
        }
        if let Some((left, operator)) = binding
            && (left.len() != 1
                || !matches!(self.parser.nodes[left[0].0].kind, ExprKind::Name)
                || &self.parser.source[operator.clone()] != ":=")
        {
            self.parser.error(
                self.statements[statement.0].span.clone(),
                "type switch guard requires one identifier and ':='",
            );
        }
        true
    }

    fn switch_cases(&mut self, depth: usize) -> (Vec<StmtId>, usize) {
        let mut cases = Vec::new();
        if !self.eat(TokenKind::LBrace) {
            let end = self.parser.source.len();
            self.parser.error(end..end, "expected switch body");
            return (cases, end);
        }
        while let Some(token) = self.parser.tokens.peek().cloned() {
            if token.kind == TokenKind::RBrace {
                break;
            }
            if self.eat(TokenKind::Semicolon) {
                continue;
            }
            let spelling = &self.parser.source[token.span.clone()];
            if !matches!(spelling, "case" | "default") {
                self.parser.error(token.span, "expected case or default");
                self.parser.tokens.next();
                continue;
            }
            self.parser.tokens.next();
            let default = spelling == "default";
            let values = if default {
                Vec::new()
            } else {
                self.expression_list()
            };
            if self
                .parser
                .tokens
                .peek()
                .is_some_and(|t| &self.parser.source[t.span.clone()] == ":")
            {
                self.parser.tokens.next();
            } else {
                self.parser
                    .error(token.span.clone(), "expected ':' after case");
            }
            let (statements, end) = self.case_body(depth, token.span.end);
            // The clause span covers the whole clause region up to the next
            // clause keyword or the switch closing brace; the query scope layer
            // uses it to keep clause locals visible in trailing trivia.
            let end = self
                .parser
                .tokens
                .peek()
                .map_or(self.parser.source.len(), |token| token.span.start)
                .max(end);
            cases.push(self.add(
                StmtKind::Case {
                    values,
                    statements,
                    default,
                },
                token.span.start..end,
            ));
        }
        let end = self.parser.close(
            TokenKind::RBrace,
            "expected '}' after switch",
            self.parser.source.len(),
        );
        (cases, end)
    }
    pub(super) fn case_body(&mut self, depth: usize, initial_end: usize) -> (Vec<StmtId>, usize) {
        let mut statements = Vec::new();
        let mut end = initial_end;
        while let Some(next) = self.parser.tokens.peek() {
            if next.kind == TokenKind::RBrace
                || matches!(&self.parser.source[next.span.clone()], "case" | "default")
            {
                break;
            }
            if self.eat(TokenKind::Semicolon) {
                continue;
            }
            let before = self
                .parser
                .tokens
                .peek()
                .expect("case statement")
                .span
                .start;
            let statement = self.statement(depth + 1);
            end = self.statements[statement.0].span.end;
            statements.push(statement);
            if self.eat(TokenKind::Semicolon) {
                continue;
            }
            if self
                .parser
                .tokens
                .peek()
                .is_some_and(|t| t.kind != TokenKind::RBrace)
            {
                self.parser
                    .error(end..end, "expected case statement separator");
            }
            if self
                .parser
                .tokens
                .peek()
                .is_some_and(|t| t.span.start <= before)
            {
                self.parser.tokens.next();
                self.parser.error(before..before, "invalid case statement");
            }
        }
        (statements, end)
    }
}
