//! Shared import-spec grammar for the stub and full-file parsers.
use crate::syntax::{Lexer, Token, TokenKind};
use std::iter::Peekable;
use std::ops::Range;

pub(crate) struct ImportSpec {
    pub(crate) alias: Option<Range<usize>>,
    pub(crate) path: Range<usize>,
}

pub(super) fn import_spec(tokens: &mut Peekable<Lexer<'_>>) -> Option<ImportSpec> {
    let alias = if matches!(tokens.peek()?.kind, TokenKind::Ident | TokenKind::Period) {
        tokens.next().map(|token| token.span)
    } else {
        None
    };
    if tokens.peek()?.kind != TokenKind::String {
        return None;
    }
    let path = tokens.next()?.span;
    Some(ImportSpec { alias, path })
}

/// Identifiers immediately followed by a period. This is a necessary spelling
/// for a qualified import use, and may also refer to a shadowing local.
pub(crate) fn qualified_prefixes(source: &str) -> impl Iterator<Item = &str> {
    let mut previous: Option<Token> = None;
    Lexer::new(source).filter_map(move |token| {
        let period = token.kind == TokenKind::Period;
        let previous = previous.replace(token)?;
        (period && previous.kind == TokenKind::Ident).then(|| &source[previous.span])
    })
}
