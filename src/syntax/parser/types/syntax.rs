//! Type syntax consumed on demand without constructing a recursive type tree.
use crate::syntax::ast::types::{
    SignatureName, SignatureNameKind, TypeNodeKind, TypeReference, TypeSyntax,
};
use crate::syntax::parser::expression::Parser;
use crate::syntax::parser::signature::ParameterContext;
use crate::syntax::token::TokenKind;
use std::ops::Range;

/// Parses one type, collecting name spans and array-length expressions.
pub fn parse_type(source: &str) -> TypeSyntax {
    parse(source, End::Complete)
}

/// Parses the first complete type in a larger token stream; span ends before
/// the following delimiter. Diagnostics still describe the complete prefix.
pub fn parse_type_prefix(source: &str) -> TypeSyntax {
    parse(source, End::Prefix)
}

#[derive(PartialEq, Eq)]
enum End {
    Complete,
    Prefix,
}

fn parse(source: &str, end: End) -> TypeSyntax {
    let mut parser = Parser::new(source);
    let span = parser.type_syntax(0);
    while parser
        .tokens
        .peek()
        .is_some_and(|token| token.kind == TokenKind::Semicolon)
    {
        parser.tokens.next();
    }
    if end == End::Complete
        && let Some(token) = parser.tokens.next()
    {
        parser.error(token.span, "unexpected token after type");
    }
    TypeSyntax {
        span,
        references: parser.type_references,
        signature_names: parser.signature_names,
        fields: parser.fields,
        interface_members: parser.interface_members,
        expressions: parser.nodes,
        diagnostics: parser.diagnostics,
        type_nodes: parser.type_nodes,
    }
}

impl Parser<'_> {
    pub(in crate::syntax::parser) fn function_type_signature(&mut self, depth: usize) -> usize {
        // Function types cannot have type parameters, but go/parser still consumes
        // and reports them before parsing the parameter list. Consuming the list
        // keeps trailing conversions/calls and function-literal bodies recoverable
        // (e.g. `(func [P any]())(nil)` and `func [P any]() {}`).
        if self
            .tokens
            .peek()
            .is_some_and(|token| token.kind == TokenKind::LBrack)
        {
            let start = self
                .tokens
                .peek()
                .expect("peeked type parameter list")
                .span
                .start;
            let (_, end) = self.parameter_list(ParameterContext::TypeParameters, depth);
            self.error(start..end, "function type must have no type parameters");
        }
        let (parameters, end) = self.parameter_list(ParameterContext::Inputs, depth);
        let (results, end) = self.result_parameters(depth, end);
        for (parameters, kind) in [
            (parameters, SignatureNameKind::Parameter),
            (results, SignatureNameKind::Result),
        ] {
            self.signature_names
                .extend(parameters.into_iter().filter_map(|parameter| {
                    parameter.name.map(|name| SignatureName {
                        name,
                        kind,
                        annotation: parameter.annotation,
                        variadic: parameter.variadic,
                    })
                }));
        }
        end
    }

    pub(in crate::syntax::parser) fn type_syntax(&mut self, depth: usize) -> Range<usize> {
        self.type_with_inference(depth, true)
    }

    pub(in crate::syntax::parser) fn array_field_type(&mut self, depth: usize) -> Range<usize> {
        self.type_with_inference(depth, false)
    }

    fn type_with_inference(&mut self, depth: usize, inferred_array: bool) -> Range<usize> {
        let Some(token) = self.tokens.peek().cloned() else {
            let end = self.source.len();
            self.error(end..end, "expected type");
            return end..end;
        };
        if depth >= 128 {
            self.tokens.next();
            self.error(token.span.clone(), "type nesting limit exceeded");
            return token.span;
        }
        let start = token.span.start;
        let spelling = &self.source[token.span.clone()];
        let end = match (token.kind, spelling) {
            (TokenKind::Mul, _) => {
                self.tokens.next();
                let node = self.begin_type_node(TypeNodeKind::Pointer, start);
                let end = self.type_syntax(depth + 1).end;
                self.end_type_node(node, end);
                end
            }
            (TokenKind::LParen, _) => {
                self.tokens.next();
                let node = self.begin_type_node(TypeNodeKind::Parenthesized, start);
                let inner = self.type_syntax(depth + 1);
                let end = self.close(TokenKind::RParen, "expected ')' in type", inner.end);
                self.end_type_node(node, end);
                end
            }
            (TokenKind::LBrack, _) => {
                self.tokens.next();
                let inferred_length = self.eat_spelling("...");
                if inferred_length && !inferred_array {
                    self.error(
                        token.span.clone(),
                        "inferred array length is not allowed in a named field",
                    );
                }
                let slice = !inferred_length
                    && self
                        .tokens
                        .peek()
                        .is_some_and(|t| t.kind == TokenKind::RBrack);
                let node = self.begin_type_node(
                    if slice {
                        TypeNodeKind::Slice
                    } else {
                        TypeNodeKind::Array
                    },
                    start,
                );
                if !inferred_length && !slice {
                    self.expression_without_type_nodes(1, depth + 1);
                }
                self.close(
                    TokenKind::RBrack,
                    "expected ']' in array type",
                    token.span.end,
                );
                let end = self.type_syntax(depth + 1).end;
                self.end_type_node(node, end);
                end
            }
            (TokenKind::Map, _) => {
                self.tokens.next();
                let node = self.begin_type_node(TypeNodeKind::Map, start);
                self.expect_kind(TokenKind::LBrack, "expected '[' after map");
                let key = self.type_syntax(depth + 1);
                self.close(TokenKind::RBrack, "expected ']' after map key", key.end);
                let end = self.type_syntax(depth + 1).end;
                self.end_type_node(node, end);
                end
            }
            (TokenKind::Chan, _) | (TokenKind::Arrow, _) => {
                self.tokens.next();
                let kind = if spelling == "<-" {
                    if !self.eat_spelling("chan") {
                        self.error(token.span.clone(), "expected chan after '<-' in type");
                    }
                    TypeNodeKind::RecvChan
                } else if self.eat_spelling("<-") {
                    TypeNodeKind::SendChan
                } else {
                    TypeNodeKind::Chan
                };
                let node = self.begin_type_node(kind, start);
                let end = self.type_syntax(depth + 1).end;
                self.end_type_node(node, end);
                end
            }
            (TokenKind::Struct | TokenKind::Interface, _) => {
                self.tokens.next();
                let interface = token.kind == TokenKind::Interface;
                let node = self.begin_type_node(
                    if interface {
                        TypeNodeKind::Interface
                    } else {
                        TypeNodeKind::Struct
                    },
                    start,
                );
                let end = self.aggregate_type(interface, depth + 1);
                self.end_type_node(node, end);
                end
            }
            (TokenKind::Func, _) => {
                self.tokens.next();
                let node = self.begin_type_node(TypeNodeKind::Function, start);
                let end = self.function_type_signature(depth + 1);
                self.end_type_node(node, end);
                end
            }
            (TokenKind::Ident, _) => {
                self.tokens.next();
                let mut end = token.span.end;
                let mut qualifier = None;
                let name = token.span.clone();
                if self
                    .tokens
                    .peek()
                    .is_some_and(|t| t.kind == TokenKind::Period)
                {
                    self.tokens.next();
                    qualifier = Some(token.span.clone());
                    if let Some(name) = self
                        .tokens
                        .peek()
                        .filter(|t| t.kind == TokenKind::Ident)
                        .cloned()
                    {
                        self.tokens.next();
                        end = name.span.end;
                        // Keep the final component distinct from the package alias.
                        self.type_references.push(TypeReference {
                            qualifier: qualifier.clone(),
                            name: name.span.clone(),
                        });
                        self.record_type_node(TypeNodeKind::Qualified, start..end);
                    } else {
                        self.error(end..end, "expected qualified type name");
                    }
                }
                if qualifier.is_none() {
                    self.type_references.push(TypeReference {
                        qualifier: None,
                        name,
                    });
                    self.record_type_node(TypeNodeKind::Name, token.span.clone());
                }
                if self
                    .tokens
                    .peek()
                    .is_some_and(|t| t.kind == TokenKind::LBrack)
                {
                    self.tokens.next();
                    let node = self.begin_type_node(TypeNodeKind::Instantiation, start);
                    let mut count = 0;
                    while self.tokens.peek().is_some_and(|t| {
                        !matches!(
                            t.kind,
                            TokenKind::RBrack | TokenKind::Semicolon | TokenKind::RParen
                        )
                    }) {
                        let argument = self.type_syntax(depth + 1);
                        end = argument.end;
                        count += 1;
                        if !self.eat_spelling(",") {
                            break;
                        }
                    }
                    if count == 0 {
                        self.error(end..end, "expected type argument");
                    }
                    end = self.close(TokenKind::RBrack, "expected ']' after type arguments", end);
                    self.end_type_node(node, end);
                }
                end
            }
            _ => {
                self.error(token.span.clone(), "expected type");
                if !matches!(
                    token.kind,
                    TokenKind::RParen | TokenKind::RBrack | TokenKind::Comma | TokenKind::Semicolon
                ) {
                    self.tokens.next();
                }
                start
            }
        };
        start..end
    }

    fn eat_spelling(&mut self, spelling: &str) -> bool {
        if self
            .tokens
            .peek()
            .is_some_and(|t| &self.source[t.span.clone()] == spelling)
        {
            self.tokens.next();
            true
        } else {
            false
        }
    }

    pub(in crate::syntax::parser) fn expect_kind(
        &mut self,
        kind: TokenKind,
        message: &'static str,
    ) {
        if self.tokens.peek().is_some_and(|t| t.kind == kind) {
            self.tokens.next();
        } else {
            let span = self
                .tokens
                .peek()
                .map_or(self.source.len()..self.source.len(), |t| t.span.clone());
            self.error(span, message);
        }
    }

    // Opaque function-literal bodies are scanned iteratively.
    pub(in crate::syntax::parser) fn type_group(
        &mut self,
        opening: TokenKind,
        closing: TokenKind,
    ) -> usize {
        let start = self
            .tokens
            .peek()
            .map_or(self.source.len(), |t| t.span.start);
        if !self.tokens.peek().is_some_and(|t| t.kind == opening) {
            self.error(start..start, "expected type member group");
            return start;
        }
        self.tokens.next();
        let mut closings = vec![closing];
        while let Some(token) = self.tokens.next() {
            match token.kind {
                TokenKind::LParen => closings.push(TokenKind::RParen),
                TokenKind::LBrack => closings.push(TokenKind::RBrack),
                TokenKind::LBrace => closings.push(TokenKind::RBrace),
                TokenKind::RParen | TokenKind::RBrack | TokenKind::RBrace => {
                    if closings.pop() != Some(token.kind) {
                        self.error(token.span.clone(), "mismatched type delimiter");
                    }
                    if closings.is_empty() {
                        return token.span.end;
                    }
                }
                TokenKind::Illegal => self.error(token.span, "invalid token in type"),
                _ => {}
            }
        }
        self.error(
            self.source.len()..self.source.len(),
            "unclosed type member group",
        );
        self.source.len()
    }
}
