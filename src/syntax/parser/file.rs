//! Full-file acceptance with declaration-local arenas and on-demand body traversal.
use crate::syntax::ast::expression::{Diagnostic, Expr, ExprKind};
use crate::syntax::parser::expression::{MAX_DIAGNOSTICS, Parser};
use crate::syntax::parser::imports::{self, ImportSpec};
use crate::syntax::parser::signature::HeaderKind;
use crate::syntax::parser::statement;
use crate::syntax::token::TokenKind;
use std::ops::Range;

const MAX_PENDING_BODIES: usize = 4096;

/// Validate file syntax without returning or retaining a whole-file syntax tree.
/// Bodies are checked one at a time, including nested function literals. The
/// lightweight `stub` indexing path remains separate and does not call this API.
/// Output is limited to 4,096 diagnostics; body worklists allow 4,096 pending
/// bodies and 128 nested literal scopes, reporting excess as a syntax error.
pub fn validate_file(source: &str) -> Vec<Diagnostic> {
    validate_file_with_imports(source, |_, _, _, _| {})
}

/// Visit import specs as part of file validation. Spans belong to `source`;
/// the visitor controls how many specs it retains for its request.
pub(crate) fn validate_file_with_imports(
    source: &str,
    mut visit_import: impl FnMut(ImportSpec, usize, Option<usize>, Option<usize>),
) -> Vec<Diagnostic> {
    let mut parser = Parser::new(source);
    // Validation consumes syntax and diagnostics without projecting type shapes.
    parser.record_type_nodes = false;
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
                import_declaration(&mut parser, &mut visit_import);
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
        append_diagnostics(&mut diagnostics, parser.diagnostics.drain(..), 0);
        if let Some(span) = enqueue_literal_bodies(&mut bodies, &parser.nodes, 0, 0) {
            body_limit(&mut diagnostics, span);
            return diagnostics;
        }
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
    } else if parser
        .tokens
        .peek()
        .is_some_and(|token| Some(token.kind) != closing)
    {
        parser.error_here("expected declaration separator", Some("expected ';'"));
    }
}

fn import_declaration(
    parser: &mut Parser<'_>,
    visit: &mut impl FnMut(ImportSpec, usize, Option<usize>, Option<usize>),
) {
    let start = parser.tokens.next().expect("import keyword").span.start;
    let group_start = if parser
        .tokens
        .peek()
        .is_some_and(|token| token.kind == TokenKind::LParen)
    {
        parser.tokens.next().map(|token| token.span.end)
    } else {
        None
    };
    let mut previous_end = None;
    loop {
        if group_start.is_some()
            && parser
                .tokens
                .peek()
                .is_none_or(|token| token.kind == TokenKind::RParen)
        {
            break;
        }
        let before = parser
            .tokens
            .peek()
            .map_or(parser.source.len(), |token| token.span.start);
        if let Some(spec) = imports::import_spec(&mut parser.tokens) {
            let end = spec.path.end;
            visit(spec, start, group_start, previous_end);
            previous_end = Some(end);
        } else if parser.tokens.peek().is_some_and(|token| {
            matches!(
                token.kind,
                TokenKind::Int | TokenKind::Float | TokenKind::Imag | TokenKind::Char
            )
        }) {
            parser.error_here("import path must be a string", None);
            parser.tokens.next();
        } else {
            parser.error_here("missing import path", None);
        }
        if group_start.is_none() {
            break;
        }
        separator(parser, Some(TokenKind::RParen));
        if parser
            .tokens
            .peek()
            .is_some_and(|token| token.span.start == before)
        {
            parser.tokens.next();
        }
    }
    if group_start.is_some() {
        parser.expect_kind(TokenKind::RParen, "expected ')' after imports");
    }
}

fn function(parser: &mut Parser<'_>, bodies: &mut Vec<(Range<usize>, usize)>) {
    parser.signature_header(HeaderKind::Named);
    if parser
        .tokens
        .peek()
        .is_some_and(|t| t.kind == TokenKind::Semicolon)
        && parser
            .tokens
            .clone()
            .nth(1)
            .is_some_and(|t| t.kind == TokenKind::LBrace)
    {
        parser.tokens.next();
        parser.error_here("unexpected semicolon or newline before {", None);
    }
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
        if depth >= 128 {
            body_limit(diagnostics, span);
            return;
        }
        let parsed = statement::parse_block(&source[span.clone()]);
        append_diagnostics(diagnostics, parsed.diagnostics, span.start);
        if let Some(span) =
            enqueue_literal_bodies(&mut bodies, &parsed.expressions, span.start, depth + 1)
        {
            body_limit(diagnostics, span);
            return;
        }
    }
}

fn enqueue_literal_bodies(
    bodies: &mut Vec<(Range<usize>, usize)>,
    expressions: &[Expr],
    base: usize,
    depth: usize,
) -> Option<Range<usize>> {
    for expression in expressions {
        if let ExprKind::FunctionLiteral { body, .. } = &expression.kind {
            let span = base + body.start..base + body.end;
            if bodies.len() == MAX_PENDING_BODIES {
                return Some(span);
            }
            bodies.push((span, depth));
        }
    }
    None
}

fn body_limit(diagnostics: &mut Vec<Diagnostic>, span: Range<usize>) {
    if diagnostics.len() < MAX_DIAGNOSTICS {
        diagnostics.push(Diagnostic {
            span,
            message: "file body validation limit exceeded",
            expected: None,
        });
    }
}

fn append_diagnostics(
    target: &mut Vec<Diagnostic>,
    diagnostics: impl IntoIterator<Item = Diagnostic>,
    base: usize,
) {
    for mut diagnostic in diagnostics.into_iter().take(MAX_DIAGNOSTICS - target.len()) {
        diagnostic.rebase(base);
        target.push(diagnostic);
    }
}
