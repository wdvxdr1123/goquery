//! Named function headers and reusable parameter-list parsing.
use crate::syntax::ast::signature::{Parameter, Signature};
use crate::syntax::parser::expression::Parser;
use crate::syntax::parser::signature::ParameterContext;
use crate::syntax::parser::signature::parameters;
use crate::syntax::token::TokenKind;

/// Parses a named function/method header ending before its opening body brace.
/// Type references include nested aggregate member types, without owning source.
pub fn parse_signature(source: &str) -> Signature {
    header(source, HeaderKind::Named)
}

/// Parses a function literal/type signature ending before its optional body.
pub fn parse_literal_signature(source: &str) -> Signature {
    header(source, HeaderKind::Literal)
}

#[derive(PartialEq, Eq)]
enum HeaderKind {
    Named,
    Literal,
}

fn header(source: &str, kind: HeaderKind) -> Signature {
    let mut parser = Parser::new(source);
    parser.expect_kind(TokenKind::Func, "expected func");
    let receiver = if kind == HeaderKind::Named && parser.at(TokenKind::LParen) {
        parser.parameter_list(ParameterContext::Inputs, 0).0
    } else {
        Vec::new()
    };
    if kind == HeaderKind::Named {
        parser.expect_kind(TokenKind::Ident, "expected function name");
    }
    let type_parameters = if kind == HeaderKind::Named && parser.at(TokenKind::LBrack) {
        parser.parameter_list(ParameterContext::TypeParameters, 0).0
    } else {
        Vec::new()
    };
    let (parameters, end) = parser.parameter_list(ParameterContext::Inputs, 0);
    let (results, _) = parser.result_parameters(0, end);
    while parser.at(TokenKind::Semicolon) {
        parser.tokens.next();
    }
    if let Some(token) = parser.tokens.next() {
        parser.error(token.span, "unexpected token after signature");
    }
    Signature {
        receiver,
        type_parameters,
        parameters,
        results,
        diagnostics: parser.diagnostics,
        type_references: parser.type_references,
        expressions: parser.nodes,
    }
}

impl Parser<'_> {
    fn at(&mut self, kind: TokenKind) -> bool {
        self.tokens.peek().is_some_and(|token| token.kind == kind)
    }

    pub(in crate::syntax::parser) fn result_parameters(
        &mut self,
        depth: usize,
        fallback: usize,
    ) -> (Vec<Parameter>, usize) {
        if self.at(TokenKind::LParen) {
            self.parameter_list(ParameterContext::Results, depth)
        } else if self
            .tokens
            .peek()
            .is_some_and(|token| parameters::starts_type(token.kind))
        {
            let annotation = self.type_syntax(depth + 1);
            let end = annotation.end;
            (
                vec![Parameter {
                    name: None,
                    annotation,
                    variadic: false,
                }],
                end,
            )
        } else {
            (Vec::new(), fallback)
        }
    }
}
