//! Receiver type parameter extraction from handwritten method headers.
use crate::syntax::TokenKind;
use std::ops::Range;

// Receiver type arguments declare implicit type parameters; they must shadow
// package types even though generic method-set resolution is still pending.
pub(crate) fn receiver_parameters(source: &str, annotation: &Range<usize>) -> Vec<Range<usize>> {
    let mut tokens = crate::syntax::Lexer::new(&source[annotation.clone()]).peekable();
    let mut parentheses = 0usize;
    while let Some(token) = tokens.peek() {
        match token.kind {
            TokenKind::LParen => parentheses += 1,
            TokenKind::Mul => {}
            _ => break,
        }
        tokens.next();
    }
    if !tokens
        .next()
        .is_some_and(|token| token.kind == TokenKind::Ident)
        || !tokens
            .next()
            .is_some_and(|token| token.kind == TokenKind::LBrack)
    {
        return Vec::new();
    }
    let mut names = Vec::new();
    loop {
        let Some(name) = tokens.next().filter(|token| token.kind == TokenKind::Ident) else {
            return Vec::new();
        };
        names.push(annotation.start + name.span.start..annotation.start + name.span.end);
        match tokens.next().map(|token| token.kind) {
            Some(TokenKind::RBrack) => break,
            Some(TokenKind::Comma) => {
                if tokens
                    .peek()
                    .is_some_and(|token| token.kind == TokenKind::RBrack)
                {
                    tokens.next();
                    break;
                }
            }
            _ => return Vec::new(),
        }
    }
    for _ in 0..parentheses {
        if tokens.next().map(|token| token.kind) != Some(TokenKind::RParen) {
            return Vec::new();
        }
    }
    if tokens
        .next()
        .is_some_and(|token| token.kind != TokenKind::Semicolon)
        || tokens.next().is_some()
    {
        return Vec::new();
    }
    names
}
