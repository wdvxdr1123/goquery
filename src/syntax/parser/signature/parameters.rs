//! Parameter names/types are disambiguated before right-to-left distribution.
use crate::syntax::ast::signature::Parameter;
use crate::syntax::parser::expression::Parser;
use crate::syntax::token::TokenKind;
use std::ops::Range;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::syntax::parser) enum ParameterContext {
    Inputs,
    Results,
    TypeParameters,
}

pub(in crate::syntax::parser) struct ParameterField {
    pub(in crate::syntax::parser) name: Option<Range<usize>>,
    pub(in crate::syntax::parser) annotation: Option<Range<usize>>,
    pub(in crate::syntax::parser) variadic: bool,
}

impl Parser<'_> {
    pub(in crate::syntax::parser) fn parameter_list(
        &mut self,
        context: ParameterContext,
        depth: usize,
    ) -> (Vec<Parameter>, usize) {
        let types = context == ParameterContext::TypeParameters;
        let (opening, closing) = if types {
            (TokenKind::LBrack, TokenKind::RBrack)
        } else {
            (TokenKind::LParen, TokenKind::RParen)
        };
        let start = self
            .tokens
            .peek()
            .map_or(self.source.len(), |token| token.span.start);
        self.expect_kind(opening, "expected parameter list");
        let mut fields = Vec::new();
        while let Some(token) = self.tokens.peek().cloned() {
            if token.kind == closing {
                break;
            }
            if matches!(token.kind, TokenKind::Semicolon | TokenKind::RBrace) {
                self.error(token.span, "expected parameter or closing delimiter");
                break;
            }
            fields.push(self.parameter_field(types, depth));
            if self
                .tokens
                .peek()
                .is_some_and(|token| token.kind == TokenKind::Comma)
            {
                self.tokens.next();
            } else {
                break;
            }
            if self
                .tokens
                .peek()
                .is_some_and(|next| next.span.start <= token.span.start)
            {
                self.tokens.next();
                break;
            }
        }
        let end = self.close(closing, "expected parameter list closing delimiter", start);
        if fields.is_empty() && types {
            self.error(start..end, "empty type parameter list");
        }
        let named = fields
            .iter()
            .any(|field| field.name.is_some() && field.annotation.is_some());
        if named {
            let mut inherited = None;
            for field in fields.iter_mut().rev() {
                if field.annotation.is_some() {
                    inherited = field.annotation.clone().map(|span| (span, field.variadic));
                } else if let Some((span, variadic)) = &inherited {
                    field.annotation = Some(span.clone());
                    field.variadic = *variadic;
                } else {
                    self.error(start..end, "missing parameter type");
                }
                if field.name.is_none() {
                    self.error(start..end, "mixed named and unnamed parameters");
                }
            }
        } else {
            if types && !fields.is_empty() {
                self.error(start..end, "missing type parameter name or constraint");
            }
            for field in &mut fields {
                if field.annotation.is_none() {
                    field.annotation = field.name.take();
                    if let Some(name) = &field.annotation {
                        self.type_references
                            .push(crate::syntax::ast::types::TypeReference {
                                qualifier: None,
                                name: name.clone(),
                            });
                        // `type_syntax` is bypassed for a bare unnamed type name.
                        self.record_type_node(
                            crate::syntax::ast::types::TypeNodeKind::Name,
                            name.clone(),
                        );
                    }
                }
            }
        }
        let count = fields.len();
        let parameters = fields
            .into_iter()
            .enumerate()
            .map(|(index, field)| {
                if field.variadic && (context != ParameterContext::Inputs || index + 1 != count) {
                    self.error(start..end, "variadic type requires final input parameter");
                }
                Parameter {
                    name: field.name,
                    annotation: field.annotation.unwrap_or(end..end),
                    variadic: field.variadic,
                }
            })
            .collect();
        (parameters, end)
    }

    pub(in crate::syntax::parser) fn parameter_field(
        &mut self,
        constraints: bool,
        depth: usize,
    ) -> ParameterField {
        let saved = self.tokens.clone();
        let mut name = None;
        let mut annotation = None;
        if self
            .tokens
            .peek()
            .is_some_and(|token| token.kind == TokenKind::Ident)
        {
            let ident = self.tokens.next().expect("peeked parameter identifier");
            let next = self.tokens.peek().map(|token| token.kind);
            let unnamed = next == Some(TokenKind::Period)
                || next == Some(TokenKind::LBrack) && !self.bracket_followed_by_type()
                || constraints && next == Some(TokenKind::Or);
            if unnamed {
                self.tokens = saved;
            } else {
                name = Some(ident.span);
                if !self.tokens.peek().is_some_and(|token| {
                    starts_type(token.kind)
                        || token.kind == TokenKind::Ellipsis
                        || constraints && token.kind == TokenKind::Tilde
                }) {
                    return ParameterField {
                        name,
                        annotation,
                        variadic: false,
                    };
                }
            }
        }
        let variadic = self
            .tokens
            .peek()
            .is_some_and(|token| token.kind == TokenKind::Ellipsis);
        if variadic {
            self.tokens.next();
        }
        if constraints
            && self
                .tokens
                .peek()
                .is_some_and(|token| token.kind == TokenKind::Tilde)
        {
            let start = self.tokens.next().expect("constraint tilde").span.start;
            annotation = Some(start..self.type_syntax(depth + 1).end);
        } else {
            annotation = Some(
                if name.is_some()
                    && self
                        .tokens
                        .peek()
                        .is_some_and(|token| token.kind == TokenKind::LBrack)
                {
                    self.array_field_type(depth + 1)
                } else {
                    self.type_syntax(depth + 1)
                },
            );
        }
        if constraints {
            while self
                .tokens
                .peek()
                .is_some_and(|token| token.kind == TokenKind::Or)
            {
                self.tokens.next();
                if self
                    .tokens
                    .peek()
                    .is_some_and(|token| token.kind == TokenKind::Tilde)
                {
                    self.tokens.next();
                }
                let end = self.type_syntax(depth + 1).end;
                annotation.as_mut().expect("constraint type").end = end;
            }
        }
        ParameterField {
            name,
            annotation,
            variadic,
        }
    }

    // T[P] is an unnamed instantiated type unless a following type makes the
    // brackets an array/slice belonging to the parameter named T.
    fn bracket_followed_by_type(&self) -> bool {
        let mut tokens = self.tokens.clone();
        let mut depth = 0;
        for token in tokens.by_ref() {
            match token.kind {
                TokenKind::LBrack => depth += 1,
                TokenKind::RBrack => {
                    depth -= 1;
                    if depth == 0 {
                        return tokens.next().is_some_and(|token| starts_type(token.kind));
                    }
                }
                _ => {}
            }
        }
        false
    }
}

pub(super) fn starts_type(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Ident
            | TokenKind::Mul
            | TokenKind::Arrow
            | TokenKind::Func
            | TokenKind::Chan
            | TokenKind::Map
            | TokenKind::Struct
            | TokenKind::Interface
            | TokenKind::LParen
            | TokenKind::LBrack
    )
}
