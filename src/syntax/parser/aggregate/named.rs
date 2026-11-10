//! Streaming named alias/underlying targets; no recursive type tree.
use crate::syntax::ast::TypeTarget;
use crate::syntax::lexer::Lexer;
use crate::syntax::token::TokenKind;

pub(super) fn target(source: &str) -> Option<TypeTarget> {
    let mut tokens = Lexer::new(source).take(8192).peekable();
    let generic = tokens.peek()?.kind == TokenKind::LBrack;
    if generic {
        let end = parameter_end(source)?;
        while tokens.peek().is_some_and(|token| token.span.start < end) {
            tokens.next();
        }
    }
    let alias = tokens.peek()?.kind == TokenKind::Assign;
    if alias {
        tokens.next();
    }
    let mut name = String::new();
    if tokens.peek()?.kind == TokenKind::Mul {
        tokens.next();
        name.push('*');
    }
    let first = tokens
        .next()
        .filter(|token| token.kind == TokenKind::Ident)?;
    if first.span.len() > 16 * 1024 {
        return None;
    }
    name.push_str(&source[first.span]);
    if tokens
        .peek()
        .is_some_and(|token| token.kind == TokenKind::Period)
    {
        tokens.next();
        let field = tokens
            .next()
            .filter(|token| token.kind == TokenKind::Ident)?;
        if name.len().saturating_add(field.span.len()) >= 16 * 1024 {
            return None;
        }
        name.push('.');
        name.push_str(&source[field.span]);
    }
    if name.len() > 16 * 1024 {
        return None;
    }
    if tokens
        .peek()
        .is_some_and(|token| token.kind == TokenKind::LBrack)
    {
        tokens.next();
        name.push('[');
        let mut depth = 1usize;
        for token in tokens.by_ref() {
            match token.kind {
                TokenKind::LBrack => depth += 1,
                TokenKind::RBrack => depth -= 1,
                _ => {}
            }
            name.push(' ');
            if token.kind == TokenKind::Semicolon {
                name.push(';');
            } else {
                name.push_str(&source[token.span]);
            }
            if name.len() > 16 * 1024 {
                return None;
            }
            if depth == 0 {
                break;
            }
        }
        if depth != 0 {
            return None;
        }
    }
    if tokens.any(|token| token.kind != TokenKind::Semicolon) {
        return None;
    }
    Some(TypeTarget {
        name,
        alias,
        generic,
    })
}

/// Proves a parameter prefix instead of mistaking array length syntax for one.
pub(super) fn parameter_end(source: &str) -> Option<usize> {
    let mut tokens = Lexer::new(source).take(8192);
    if tokens.next()?.kind != TokenKind::LBrack {
        return None;
    }
    let first = tokens.next()?;
    if first.kind != TokenKind::Ident {
        return None;
    }
    let mut stack = vec![TokenKind::RBrack];
    let mut previous = first.kind;
    let mut proven = false;
    for token in tokens {
        let kind = token.kind;
        if stack.len() == 1 {
            proven |= (kind == TokenKind::Ident
                && matches!(previous, TokenKind::Ident | TokenKind::RBrack))
                || matches!(
                    kind,
                    TokenKind::Tilde
                        | TokenKind::Interface
                        | TokenKind::Struct
                        | TokenKind::Map
                        | TokenKind::Chan
                        | TokenKind::Func
                );
            if kind == TokenKind::RBrack {
                return (proven || previous == TokenKind::Comma).then_some(token.span.end);
            }
        }
        match kind {
            TokenKind::LParen => stack.push(TokenKind::RParen),
            TokenKind::LBrack => stack.push(TokenKind::RBrack),
            TokenKind::LBrace => stack.push(TokenKind::RBrace),
            TokenKind::RParen | TokenKind::RBrack | TokenKind::RBrace
                if stack.pop() != Some(kind) =>
            {
                return None;
            }
            _ => {}
        }
        if stack.len() > 128 {
            return None;
        }
        if stack.len() == 1 {
            previous = kind;
        }
    }
    None
}
