//! Lexical highlighting shares the frontend scanner and excludes import strings.
use super::config::SemanticType;
use crate::syntax::{Lexer, TokenKind};
use std::ops::Range;

pub(super) fn tokens(source: &str) -> impl Iterator<Item = (Range<usize>, SemanticType)> + '_ {
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
            TokenKind::Comment => SemanticType::Comment,
            TokenKind::String => SemanticType::String,
            TokenKind::Int | TokenKind::Float | TokenKind::Imag | TokenKind::Char => {
                SemanticType::Number
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
            | TokenKind::Tilde => SemanticType::Operator,
            kind if kind
                .fixed_text()
                .and_then(TokenKind::from_keyword)
                .is_some() =>
            {
                SemanticType::Keyword
            }
            _ => return None,
        };
        Some((token.span, kind))
    })
}
