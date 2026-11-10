//! Streaming Go tokens with byte spans and automatic semicolon insertion.
mod identifier;
mod number;
mod unicode;

pub use identifier::{is_exported_name, is_identifier_continue, is_identifier_start};

use crate::syntax::literal;
use crate::syntax::token::{Token, TokenKind};

#[derive(Clone)]
pub struct Lexer<'a> {
    source: &'a str,
    cursor: usize,
    insert_semicolon: bool,
    comments: bool,
    pending_semicolon: Option<usize>,
}

impl<'a> Lexer<'a> {
    pub fn new(source: &'a str) -> Self {
        Self {
            source,
            cursor: 0,
            insert_semicolon: false,
            comments: false,
            pending_semicolon: None,
        }
    }

    /// Include comments while preserving the normal token and semicolon stream.
    pub fn with_comments(source: &'a str) -> Self {
        Self {
            comments: true,
            ..Self::new(source)
        }
    }

    fn semicolon(&mut self, offset: usize) -> Token {
        self.insert_semicolon = false;
        Token {
            kind: TokenKind::Semicolon,
            span: offset..offset,
            inserted: true,
        }
    }

    fn advance(&mut self) -> Option<char> {
        let ch = self.source[self.cursor..].chars().next()?;
        self.cursor += ch.len_utf8();
        Some(ch)
    }
    // Validate without allocating decoded text: a rune counts one source
    // character or escape, while strings allow any number of them.
    fn valid_literal(text: &str, quote: char) -> bool {
        let mut characters = text[1..text.len() - 1].chars();
        let mut count = 0;
        while let Some(character) = characters.next() {
            if matches!(character, '\0' | '\u{feff}') || (quote != '`' && character == '\n') {
                return false;
            }
            if quote != '`'
                && character == '\\'
                && literal::escape(&mut characters, quote).is_none()
            {
                return false;
            }
            count += 1;
        }
        quote != '\'' || count == 1
    }
}

impl Iterator for Lexer<'_> {
    type Item = Token;

    fn next(&mut self) -> Option<Token> {
        if let Some(offset) = self.pending_semicolon.take() {
            return Some(self.semicolon(offset));
        }
        loop {
            let start = self.cursor;
            let rest = &self.source[start..];
            let Some(ch) = rest.chars().next() else {
                return self.insert_semicolon.then(|| self.semicolon(start));
            };
            if matches!(ch, ' ' | '\t' | '\r' | '\n') || (start == 0 && ch == '\u{feff}') {
                self.advance();
                if ch == '\n' && self.insert_semicolon {
                    return Some(self.semicolon(start));
                }
                continue;
            }
            if rest.starts_with("//") {
                self.cursor += rest.find('\n').unwrap_or(rest.len());
                if self.comments {
                    return Some(Token {
                        kind: TokenKind::Comment,
                        span: start..self.cursor,
                        inserted: false,
                    });
                }
                continue;
            }
            if let Some(comment) = rest.strip_prefix("/*") {
                if let Some(end) = comment.find("*/") {
                    let length = end + 4;
                    self.cursor += length;
                    if self.comments {
                        if self.insert_semicolon {
                            self.pending_semicolon =
                                rest[..length].find('\n').map(|offset| start + offset);
                        }
                        return Some(Token {
                            kind: TokenKind::Comment,
                            span: start..self.cursor,
                            inserted: false,
                        });
                    }
                    if self.insert_semicolon
                        && let Some(newline) = rest[..length].find('\n')
                    {
                        return Some(self.semicolon(start + newline));
                    }
                    continue;
                }
                self.cursor = self.source.len();
                self.insert_semicolon = false;
                return Some(Token {
                    kind: TokenKind::Illegal,
                    span: start..self.cursor,
                    inserted: false,
                });
            }
            break;
        }
        let start = self.cursor;
        let ch = self.advance()?;
        let mut kind = TokenKind::Illegal;
        if is_identifier_start(ch) {
            while let Some(next) = self.source[self.cursor..].chars().next() {
                if !is_identifier_continue(next) {
                    break;
                }
                self.advance();
            }
            kind = TokenKind::from_keyword(&self.source[start..self.cursor])
                .unwrap_or(TokenKind::Ident);
        } else if ch.is_ascii_digit()
            || (ch == '.' && self.source[self.cursor..].starts_with(|c: char| c.is_ascii_digit()))
        {
            self.cursor = start;
            kind = self.number();
        } else if matches!(ch, '"' | '\'' | '`') {
            kind = if ch == '\'' {
                TokenKind::Char
            } else {
                TokenKind::String
            };
            let mut terminated = false;
            while let Some(next) = self.advance() {
                if next == ch {
                    terminated = true;
                    break;
                }
                if ch != '`' && next == '\\' {
                    self.advance();
                } else if ch != '`' && next == '\n' {
                    break;
                }
            }
            if !terminated || !Self::valid_literal(&self.source[start..self.cursor], ch) {
                kind = TokenKind::Illegal;
            }
        } else {
            if let Some((symbol, length)) = TokenKind::from_symbol_prefix(&self.source[start..]) {
                self.cursor = start + length;
                kind = symbol;
            }
        }
        self.insert_semicolon = kind.can_end_statement();
        Some(Token {
            kind,
            span: start..self.cursor,
            inserted: false,
        })
    }
}
