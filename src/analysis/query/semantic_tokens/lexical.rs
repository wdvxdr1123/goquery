//! Lexical highlighting shares the frontend scanner and excludes import strings.
use super::SemanticTokenKind;
use crate::syntax::{Lexer, TokenKind};
use std::ops::Range;

/// An unavailable imported literal may still expose a written container. Only
/// project that syntax: do not follow named declarations or invent field types.
pub(super) fn container_element(text: &str, key: bool) -> Option<&str> {
    use crate::syntax::TypeNodeKind;
    if text.len() > crate::analysis::budget::MAX_BYTES {
        return None;
    }
    let parsed = crate::syntax::parse_type(text);
    if !parsed.diagnostics().is_empty() {
        return None;
    }
    let mut span = parsed.span.clone();
    for _ in 0..crate::analysis::budget::MAX_DEPTH {
        let node = parsed.type_nodes().iter().find(|node| node.span == span)?;
        let child = parsed.type_nodes().iter().find(|child| {
            child.span != span && span.start <= child.span.start && child.span.end <= span.end
        })?;
        match node.kind {
            TypeNodeKind::Parenthesized => span = child.span.clone(),
            TypeNodeKind::Array | TypeNodeKind::Slice if !key => {
                return text.get(child.span.clone());
            }
            TypeNodeKind::Map if key => return text.get(child.span.clone()),
            TypeNodeKind::Map => {
                let value = parsed.type_nodes().iter().find(|value| {
                    child.span.end <= value.span.start && value.span.end <= span.end
                })?;
                return text.get(value.span.clone());
            }
            _ => return None,
        }
    }
    None
}

pub(super) fn tokens(source: &str) -> impl Iterator<Item = (Range<usize>, SemanticTokenKind)> + '_ {
    let mut importing = false;
    let mut grouped = false;
    Lexer::with_comments(source).filter_map(move |token| {
        if token.kind == TokenKind::Import {
            importing = true;
        } else if importing {
            match token.kind {
                TokenKind::LParen => grouped = true,
                TokenKind::RParen => {
                    importing = false;
                    grouped = false;
                }
                TokenKind::Semicolon if !grouped => importing = false,
                // A virtual namespace declaration inside the path is emitted
                // by the resolver. Import paths are not string expressions.
                TokenKind::String => return None,
                _ => {}
            }
        }
        let kind = match token.kind {
            TokenKind::Comment => SemanticTokenKind::Comment,
            TokenKind::String => SemanticTokenKind::String,
            TokenKind::Int | TokenKind::Float | TokenKind::Imag | TokenKind::Char => {
                SemanticTokenKind::Number
            }
            TokenKind::Add
            | TokenKind::Sub
            | TokenKind::Mul
            | TokenKind::Quo
            | TokenKind::Rem
            | TokenKind::And
            | TokenKind::Or
            | TokenKind::Xor
            | TokenKind::Shl
            | TokenKind::Shr
            | TokenKind::AndNot
            | TokenKind::AddAssign
            | TokenKind::SubAssign
            | TokenKind::MulAssign
            | TokenKind::QuoAssign
            | TokenKind::RemAssign
            | TokenKind::AndAssign
            | TokenKind::OrAssign
            | TokenKind::XorAssign
            | TokenKind::ShlAssign
            | TokenKind::ShrAssign
            | TokenKind::AndNotAssign
            | TokenKind::Land
            | TokenKind::Lor
            | TokenKind::Arrow
            | TokenKind::Inc
            | TokenKind::Dec
            | TokenKind::Eql
            | TokenKind::Lss
            | TokenKind::Gtr
            | TokenKind::Assign
            | TokenKind::Not
            | TokenKind::Neq
            | TokenKind::Leq
            | TokenKind::Geq
            | TokenKind::Define
            | TokenKind::Ellipsis
            | TokenKind::Tilde => SemanticTokenKind::Operator,
            kind if kind
                .fixed_text()
                .and_then(TokenKind::from_keyword)
                .is_some() =>
            {
                SemanticTokenKind::Keyword
            }
            _ => return None,
        };
        Some((token.span, kind))
    })
}
