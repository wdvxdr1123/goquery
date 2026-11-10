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

/// Declaration parameters are derived only when a member's type is requested.
pub(crate) fn parameters(declaration: &str) -> Option<Vec<String>> {
    let Some(end) = crate::syntax::type_parameter_end(declaration) else {
        return Some(Vec::new());
    };
    if end > MAX_TYPE_BYTES {
        return None;
    }
    let header = format!("func _{}()", declaration.get(..end)?);
    let signature = crate::syntax::parse_signature(&header);
    if !signature.diagnostics.is_empty() {
        return None;
    }
    signature
        .type_parameters
        .iter()
        .map(|parameter| {
            parameter
                .name
                .as_ref()
                .map(|span| header[span.clone()].to_owned())
        })
        .collect()
}

/// Already validated type spelling; split arguments without recursive trees.
pub(crate) fn arguments(name: &str) -> Option<Vec<&str>> {
    let opening = name.find('[')?;
    let source = name.get(opening + 1..name.len().checked_sub(1)?)?;
    let mut depth = 0usize;
    let mut start = 0;
    let mut arguments = Vec::new();
    for token in Lexer::new(source) {
        match token.kind {
            TokenKind::LBrack | TokenKind::LParen | TokenKind::LBrace => depth += 1,
            TokenKind::RBrack | TokenKind::RParen | TokenKind::RBrace => {
                depth = depth.checked_sub(1)?
            }
            TokenKind::Comma if depth == 0 => {
                arguments.push(source[start..token.span.start].trim());
                start = token.span.end;
            }
            _ => {}
        }
    }
    let last = source[start..].trim();
    if !last.is_empty() {
        arguments.push(last);
    }
    (depth == 0 && !arguments.is_empty() && arguments.len() <= 256).then_some(arguments)
}
