//! Compact named type spellings retain instantiation arguments across queries.
use crate::syntax::{Lexer, TokenKind};

const MAX_TYPE_BYTES: usize = 16 * 1024;

pub(crate) fn named_root(name: &str) -> &str {
    name.split_once('[').map_or(name, |(root, _)| root)
}

pub(crate) fn named_type(source: &str) -> Option<String> {
    if source.len() > MAX_TYPE_BYTES {
        return None;
    }
    let mut tokens = Lexer::new(source)
        .filter(|token| token.kind != TokenKind::Semicolon)
        .peekable();
    if tokens.peek()?.kind == TokenKind::Mul {
        tokens.next();
    }
    let first = tokens
        .next()
        .filter(|token| token.kind == TokenKind::Ident)?;
    let mut name = source[first.span].to_owned();
    if tokens
        .peek()
        .is_some_and(|token| token.kind == TokenKind::Period)
    {
        tokens.next();
        let member = tokens
            .next()
            .filter(|token| token.kind == TokenKind::Ident)?;
        name.push('.');
        name.push_str(&source[member.span]);
    }
    if let Some(token) = tokens.next() {
        if token.kind != TokenKind::LBrack {
            return None;
        }
        let parsed = crate::syntax::parse_type(source);
        if !parsed.diagnostics.is_empty() {
            return None;
        }
        name.push_str(source.get(token.span.start..parsed.span.end)?);
    }
    Some(name)
}
