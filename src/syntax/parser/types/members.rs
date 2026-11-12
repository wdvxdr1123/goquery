//! Struct fields, interface methods and type terms retain compact name spans.
use crate::syntax::ast::types::{InterfaceMember, TypeField, TypeReference};
use crate::syntax::parser::expression::Parser;
use crate::syntax::token::TokenKind;

impl Parser<'_> {
    pub(in crate::syntax::parser) fn aggregate_type(
        &mut self,
        interface: bool,
        depth: usize,
    ) -> usize {
        let start = self
            .tokens
            .peek()
            .map_or(self.source.len(), |token| token.span.start);
        self.expect_kind(TokenKind::LBrace, "expected aggregate body");
        while let Some(token) = self.tokens.peek().cloned() {
            if token.kind == TokenKind::RBrace {
                break;
            }
            if token.kind == TokenKind::Semicolon {
                self.tokens.next();
                continue;
            }
            if interface {
                self.interface_member(depth, start);
            } else {
                self.struct_field(depth, start);
            }
            if self
                .tokens
                .peek()
                .is_some_and(|token| token.kind == TokenKind::Semicolon)
            {
                self.tokens.next();
            } else if !self
                .tokens
                .peek()
                .is_some_and(|token| token.kind == TokenKind::RBrace)
            {
                self.error(token.span.clone(), "expected aggregate member separator");
                while self.tokens.peek().is_some_and(|token| {
                    !matches!(token.kind, TokenKind::Semicolon | TokenKind::RBrace)
                }) {
                    self.tokens.next();
                }
            }
            if self
                .tokens
                .peek()
                .is_some_and(|next| next.span.start <= token.span.start)
            {
                self.tokens.next();
                self.error(token.span, "invalid aggregate member");
            }
        }
        self.close(TokenKind::RBrace, "expected '}' after aggregate", start)
    }

    fn struct_field(&mut self, depth: usize, container: usize) {
        let first = self.tokens.peek().expect("struct member").clone();
        if !matches!(first.kind, TokenKind::Ident | TokenKind::Mul)
            || first.kind == TokenKind::Mul
                && !self
                    .tokens
                    .clone()
                    .nth(1)
                    .is_some_and(|token| token.kind == TokenKind::Ident)
        {
            self.error(
                first.span.clone(),
                "expected field name or embedded named type",
            );
        }
        let mut field = self.parameter_field(false, depth);
        let mut names: Vec<_> = field.name.iter().cloned().collect();
        let mut grouped = false;
        while field.annotation.is_none()
            && self
                .tokens
                .peek()
                .is_some_and(|token| token.kind == TokenKind::Comma)
        {
            grouped = true;
            self.tokens.next();
            field = self.parameter_field(false, depth);
            names.extend(field.name.iter().cloned());
        }
        if grouped && (field.annotation.is_none() || field.name.is_none()) {
            self.error(first.span.clone(), "expected type after field names");
        } else if field.annotation.is_none()
            && let Some(name) = field.name.clone()
        {
            self.type_references.push(TypeReference {
                qualifier: None,
                name: name.clone(),
            });
            // A bare embedded type name does not pass through `type_syntax`.
            self.record_type_node(crate::syntax::ast::types::TypeNodeKind::Name, name);
        }
        if field.variadic {
            self.error(first.span, "struct field cannot be variadic");
        }
        let tag = if self
            .tokens
            .peek()
            .is_some_and(|token| token.kind == TokenKind::String)
        {
            self.tokens.next().map(|token| token.span)
        } else {
            None
        };
        let embedded = names.is_empty() || field.annotation.is_none();
        let annotation = field.annotation.or(field.name);
        if let Some(annotation) = annotation {
            if embedded {
                // The implicit name is the written type identifier, before any
                // generic arguments; qualifiers and a leading '*' are excluded.
                names = crate::syntax::lexer::Lexer::new(&self.source[annotation.clone()])
                    .take_while(|token| token.kind != TokenKind::LBrack)
                    .filter(|token| token.kind == TokenKind::Ident)
                    .last()
                    .map(|token| {
                        annotation.start + token.span.start..annotation.start + token.span.end
                    })
                    .into_iter()
                    .collect();
            }
            self.fields.extend(names.into_iter().map(|name| TypeField {
                name,
                annotation: annotation.clone(),
                container,
                embedded,
                tag: tag.clone(),
            }));
        }
    }

    fn interface_member(&mut self, depth: usize, container: usize) {
        if self
            .tokens
            .peek()
            .is_some_and(|token| token.kind == TokenKind::Ident)
            && self
                .tokens
                .clone()
                .nth(1)
                .is_some_and(|token| token.kind == TokenKind::LParen)
        {
            let name = self.tokens.next().expect("peeked method name").span;
            let start = self
                .tokens
                .peek()
                .expect("peeked method parentheses")
                .span
                .start;
            let end = self.function_type_signature(depth + 1);
            self.interface_members.push(InterfaceMember {
                name: Some(name),
                annotation: start..end,
                container,
            });
        } else {
            let start = self.tokens.peek().expect("interface member").span.start;
            let mut end;
            loop {
                if self
                    .tokens
                    .peek()
                    .is_some_and(|token| token.kind == TokenKind::Tilde)
                {
                    self.tokens.next();
                }
                end = self.type_syntax(depth + 1).end;
                if !self
                    .tokens
                    .peek()
                    .is_some_and(|token| token.kind == TokenKind::Or)
                {
                    break;
                }
                self.tokens.next();
            }
            self.interface_members.push(InterfaceMember {
                name: None,
                annotation: start..end,
                container,
            });
        }
    }
}
