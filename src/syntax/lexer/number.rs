//! Numeric token boundaries and validity, without decoding or allocating values.
use super::Lexer;
use crate::syntax::token::TokenKind;

impl Lexer<'_> {
    // Keep consuming the same numeric token after an error so a malformed
    // exponent or radix prefix cannot become a valid number plus a name.
    pub(super) fn number(&mut self) -> TokenKind {
        let start = self.cursor;
        let mut kind = TokenKind::Int;
        let mut prefix = 0;
        let mut base = 10;
        let mut digits = false;
        let mut invalid_digit = false;
        if self.source.as_bytes()[self.cursor] != b'.' {
            if self.source.as_bytes()[self.cursor] == b'0' {
                self.cursor += 1;
                (base, prefix) = match self.source.as_bytes().get(self.cursor) {
                    Some(b'x' | b'X') => (16, b'x'),
                    Some(b'o' | b'O') => (8, b'o'),
                    Some(b'b' | b'B') => (2, b'b'),
                    _ => (8, b'0'),
                };
                if prefix == b'0' {
                    digits = true;
                } else {
                    self.cursor += 1;
                }
            }
            let scanned = self.number_digits(base);
            digits |= scanned.0;
            invalid_digit |= scanned.1;
        }
        let mut invalid = false;
        if self.source.as_bytes().get(self.cursor) == Some(&b'.') {
            self.cursor += 1;
            kind = TokenKind::Float;
            invalid = matches!(prefix, b'o' | b'b');
            let scanned = self.number_digits(base);
            digits |= scanned.0;
            invalid_digit |= scanned.1;
        }
        invalid |= !digits;
        if let Some(exponent @ (b'e' | b'E' | b'p' | b'P')) =
            self.source.as_bytes().get(self.cursor).copied()
        {
            invalid |= match exponent {
                b'e' | b'E' => !matches!(prefix, 0 | b'0'),
                _ => prefix != b'x',
            };
            self.cursor += 1;
            kind = TokenKind::Float;
            if matches!(self.source.as_bytes().get(self.cursor), Some(b'+' | b'-')) {
                self.cursor += 1;
            }
            invalid |= !self.number_digits(10).0;
        } else if prefix == b'x' && kind == TokenKind::Float {
            invalid = true;
        }
        if self.source.as_bytes().get(self.cursor) == Some(&b'i') {
            self.cursor += 1;
            kind = TokenKind::Imag;
        }
        // Like go/scanner, legacy leading-zero floats/imaginaries use decimal
        // digits. Numeric value/type checking is separate from parser acceptance.
        invalid |= kind == TokenKind::Int && invalid_digit;
        let text = &self.source.as_bytes()[start..self.cursor];
        invalid |= text.contains(&b'_') && !valid_separators(text, prefix);
        if invalid { TokenKind::Illegal } else { kind }
    }

    // Return (any digits, any out-of-base digits). Binary/octal tokens consume
    // decimal digits too, matching Go's recovery boundaries on malformed input.
    fn number_digits(&mut self, base: u8) -> (bool, bool) {
        let mut present = false;
        let mut invalid = false;
        while let Some(byte) = self.source.as_bytes().get(self.cursor).copied() {
            if byte == b'_' {
                self.cursor += 1;
                continue;
            }
            if !(byte.is_ascii_digit() || (base == 16 && byte.is_ascii_hexdigit())) {
                break;
            }
            present = true;
            invalid |= base < 10 && byte >= b'0' + base;
            self.cursor += 1;
        }
        (present, invalid)
    }
}

fn valid_separators(text: &[u8], prefix: u8) -> bool {
    let explicit_prefix = matches!(prefix, b'x' | b'o' | b'b');
    let mut previous_digit = explicit_prefix;
    let mut separator = false;
    for &byte in &text[if explicit_prefix { 2 } else { 0 }..] {
        if byte == b'_' {
            if !previous_digit {
                return false;
            }
            previous_digit = false;
            separator = true;
        } else {
            let digit = byte.is_ascii_digit() || (prefix == b'x' && byte.is_ascii_hexdigit());
            if separator && !digit {
                return false;
            }
            previous_digit = digit;
            separator = false;
        }
    }
    !separator
}
