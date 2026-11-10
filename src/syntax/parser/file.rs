//! Full-file acceptance with declaration-local arenas and on-demand body traversal.
use crate::syntax::ast::expression::{Diagnostic, ExprKind};
use crate::syntax::parser::expression::{MAX_DIAGNOSTICS, Parser};
use crate::syntax::parser::signature::ParameterContext;
use crate::syntax::parser::statement;
use crate::syntax::token::TokenKind;
use std::ops::Range;

/// Validate file syntax without returning or retaining a whole-file syntax tree.
/// Bodies are checked one at a time, including nested function literals. The
/// lightweight `stub` indexing path remains separate and does not call this API.
/// Output is limited to 4,096 diagnostics; body worklists allow 4,096 pending
/// bodies and 128 nested literal scopes, reporting excess as a syntax error.
pub fn validate_file(source: &str) -> Vec<Diagnostic> {
    let mut parser = Parser::new(source);
    parser.expect_kind(TokenKind::Package, "expected package clause");
    parser.expect_kind(TokenKind::Ident, "expected package name");
    separator(&mut parser, None);
    let mut diagnostics = Vec::new();
    let mut declarations = false;
    while let Some(token) = parser.tokens.peek().cloned() {
        let mut bodies = Vec::new();
        match token.kind {
            TokenKind::Import => {
                if declarations {
                    parser.error(token.span.clone(), "imports must precede declarations");
                }
                imports(&mut parser);
            }
            TokenKind::Var | TokenKind::Const | TokenKind::Type => {
                declarations = true;
                parser = statement::declaration_parser(parser);
            }
            TokenKind::Func => {
                declarations = true;
                function(&mut parser, &mut bodies);
            }
            _ => {
                parser.tokens.next();
                parser.error(token.span.clone(), "expected top-level declaration");
            }
        }
        for expression in &parser.nodes {
            if let ExprKind::FunctionLiteral { body, .. } = &expression.kind {
                bodies.push((body.clone(), 0));
            }
        }
        append_diagnostics(&mut diagnostics, parser.diagnostics.drain(..), 0);
        // Release the declaration arena before visiting its literal bodies.
        parser.nodes = Vec::new();
        parser.type_references = Vec::new();
        parser.signature_names = Vec::new();
        parser.fields = Vec::new();
        parser.interface_members = Vec::new();
        validate_bodies(source, bodies, &mut diagnostics);
        if diagnostics.len() == MAX_DIAGNOSTICS {
            return diagnostics;
        }
        if parser
            .tokens
            .peek()
            .is_some_and(|next| next.span.start <= token.span.start)
        {
            parser.tokens.next();
            parser.error(token.span, "file parser made no progress");
        }
        separator(&mut parser, None);
    }
    append_diagnostics(&mut diagnostics, parser.diagnostics, 0);
    diagnostics
}

fn separator(parser: &mut Parser<'_>, closing: Option<TokenKind>) {
    if parser
        .tokens
        .peek()
        .is_some_and(|token| token.kind == TokenKind::Semicolon)
    {
        parser.tokens.next();
    } else if let Some(token) = parser
        .tokens
        .peek()
        .filter(|token| Some(token.kind) != closing)
    {
        let span = token.span.clone();
        parser.error(span, "expected declaration separator");
    }
}

fn imports(parser: &mut Parser<'_>) {
    parser.tokens.next();
    let grouped = parser
        .tokens
        .peek()
        .is_some_and(|token| token.kind == TokenKind::LParen);
    if grouped {
        parser.tokens.next();
    }
    loop {
        if grouped
            && parser
                .tokens
                .peek()
                .is_none_or(|token| token.kind == TokenKind::RParen)
        {
            break;
        }
        if parser
            .tokens
            .peek()
            .is_some_and(|token| matches!(token.kind, TokenKind::Ident | TokenKind::Period))
        {
            parser.tokens.next();
        }
        let start = parser
            .tokens
            .peek()
            .map_or(parser.source.len(), |token| token.span.start);
        parser.expect_kind(TokenKind::String, "expected import path string");
        if !grouped {
            break;
        }
        separator(parser, Some(TokenKind::RParen));
        if parser
            .tokens
            .peek()
            .is_some_and(|token| token.span.start == start)
        {
            parser.tokens.next();
        }
    }
    if grouped {
        parser.expect_kind(TokenKind::RParen, "expected ')' after imports");
    }
}

fn function(parser: &mut Parser<'_>, bodies: &mut Vec<(Range<usize>, usize)>) {
    parser.tokens.next();
    if parser
        .tokens
        .peek()
        .is_some_and(|token| token.kind == TokenKind::LParen)
    {
        parser.parameter_list(ParameterContext::Inputs, 0);
    }
    parser.expect_kind(TokenKind::Ident, "expected function name");
    if parser
        .tokens
        .peek()
        .is_some_and(|token| token.kind == TokenKind::LBrack)
    {
        parser.parameter_list(ParameterContext::TypeParameters, 0);
    }
    let (_, end) = parser.parameter_list(ParameterContext::Inputs, 0);
    parser.result_parameters(0, end);
    if let Some(token) = parser
        .tokens
        .peek()
        .filter(|token| token.kind == TokenKind::LBrace)
        .cloned()
    {
        let end = parser.type_group(TokenKind::LBrace, TokenKind::RBrace);
        bodies.push((token.span.start..end, 0));
    }
}

fn validate_bodies(
    source: &str,
    mut bodies: Vec<(Range<usize>, usize)>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    while let Some((span, depth)) = bodies.pop() {
        if diagnostics.len() == MAX_DIAGNOSTICS {
            return;
        }
        if depth >= 128 || bodies.len() >= 4096 {
            append_diagnostics(
                diagnostics,
                [Diagnostic {
                    span,
                    message: "file body validation limit exceeded",
                }],
                0,
            );
            return;
        }
        let parsed = statement::parse_block(&source[span.clone()]);
        append_diagnostics(diagnostics, parsed.diagnostics, span.start);
        for expression in parsed.expressions {
            if let ExprKind::FunctionLiteral { body, .. } = expression.kind {
                if bodies.len() == 4096 {
                    append_diagnostics(
                        diagnostics,
                        [Diagnostic {
                            span,
                            message: "file body validation limit exceeded",
                        }],
                        0,
                    );
                    return;
                }
                bodies.push((span.start + body.start..span.start + body.end, depth + 1));
            }
        }
    }
}

fn append_diagnostics(
    target: &mut Vec<Diagnostic>,
    diagnostics: impl IntoIterator<Item = Diagnostic>,
    base: usize,
) {
    for mut diagnostic in diagnostics.into_iter().take(MAX_DIAGNOSTICS - target.len()) {
        diagnostic.span = base + diagnostic.span.start..base + diagnostic.span.end;
        target.push(diagnostic);
    }
}
