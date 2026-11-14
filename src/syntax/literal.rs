//! Shared escape validation and on-demand string decoding; tokens own no values.

pub(crate) enum Escape {
    Byte(u8),
    Character(char),
}

pub(crate) fn escape(characters: &mut std::str::Chars<'_>, quote: char) -> Option<Escape> {
    let first = characters.next()?;
    let byte = match first {
        'a' => Some(7),
        'b' => Some(8),
        'f' => Some(12),
        'n' => Some(b'\n'),
        'r' => Some(b'\r'),
        't' => Some(b'\t'),
        'v' => Some(11),
        '\\' => Some(b'\\'),
        ch if ch == quote => Some(quote as u8),
        _ => None,
    };
    if let Some(byte) = byte {
        return Some(Escape::Byte(byte));
    }
    let (digits, radix, mut value) = match first {
        '0'..='7' => (2, 8, first as u32 - '0' as u32),
        'x' => (2, 16, 0),
        'u' => (4, 16, 0),
        'U' => (8, 16, 0),
        _ => return None,
    };
    for _ in 0..digits {
        value = value * radix + characters.next()?.to_digit(radix)?;
    }
    if matches!(first, 'u' | 'U') {
        char::from_u32(value).map(Escape::Character)
    } else {
        u8::try_from(value).ok().map(Escape::Byte)
    }
}

// Called only for string tokens already checked by the lexer. Preserve byte
// escapes until UTF-8 validation: lossily replacing invalid bytes would invent
// a different package identity. Semantic import-path restrictions are separate.
pub(crate) fn import_path(literal: &str) -> Option<String> {
    String::from_utf8(string_value(literal)?).ok()
}

fn string_value(literal: &str) -> Option<Vec<u8>> {
    if let Some(raw) = literal
        .strip_prefix('`')
        .and_then(|text| text.strip_suffix('`'))
    {
        return Some(raw.replace('\r', "").into_bytes());
    }
    let text = literal.strip_prefix('"')?.strip_suffix('"')?;
    if !text.contains('\\') {
        return Some(text.as_bytes().to_vec());
    }
    let mut bytes = Vec::with_capacity(text.len());
    let mut characters = text.chars();
    let mut encoded = [0u8; 4];
    while let Some(character) = characters.next() {
        match character {
            '\\' => match escape(&mut characters, '"')? {
                Escape::Byte(byte) => bytes.push(byte),
                Escape::Character(character) => {
                    bytes.extend_from_slice(character.encode_utf8(&mut encoded).as_bytes())
                }
            },
            character => bytes.extend_from_slice(character.encode_utf8(&mut encoded).as_bytes()),
        }
    }
    Some(bytes)
}

/// Decode one complete Go string literal, preserving non-UTF-8 byte escapes.
/// Raw-string carriage returns are removed. Only the returned value is allocated.
pub fn decode_string_literal(source: &str) -> Option<Vec<u8>> {
    let token = crate::syntax::lexer::Lexer::new(source).next()?;
    if token.kind != crate::syntax::token::TokenKind::String || token.span != (0..source.len()) {
        return None;
    }
    string_value(source)
}

/// Decode a validated rune literal on demand, without allocating a value buffer.
pub fn decode_rune_literal(source: &str) -> Option<u32> {
    let token = crate::syntax::lexer::Lexer::new(source).next()?;
    if token.kind != crate::syntax::token::TokenKind::Char || token.span != (0..source.len()) {
        return None;
    }
    let mut characters = source.strip_prefix('\'')?.strip_suffix('\'')?.chars();
    let value = match characters.next()? {
        '\\' => match escape(&mut characters, '\'')? {
            Escape::Byte(byte) => u32::from(byte),
            Escape::Character(character) => u32::from(character),
        },
        character => u32::from(character),
    };
    characters.next().is_none().then_some(value)
}

/// Decode one complete Go string literal as a UTF-8 import identity.
/// Returns None for malformed literals or decoded non-UTF-8 byte sequences.
/// Raw-string carriage returns are removed; semantic path restrictions are
/// separate. Only the returned path is allocated, without retaining the input.
pub fn decode_import_path(source: &str) -> Option<String> {
    let token = crate::syntax::lexer::Lexer::new(source).next()?;
    if token.kind != crate::syntax::token::TokenKind::String || token.span != (0..source.len()) {
        return None;
    }
    import_path(source)
}
/// Encode decoded UTF-8 as a Go interpreted string literal.
pub fn quote_string(value: &str) -> String {
    quote_bytes(value.as_bytes())
}

/// Encode Go string bytes without replacing invalid UTF-8 sequences.
pub fn quote_bytes(mut value: &[u8]) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    while !value.is_empty() {
        let (valid, invalid) = match std::str::from_utf8(value) {
            Ok(text) => (text, 0),
            Err(error) => {
                let valid = std::str::from_utf8(&value[..error.valid_up_to()])
                    .expect("UTF-8 error prefix is validated");
                (
                    valid,
                    error
                        .error_len()
                        .unwrap_or(value.len() - error.valid_up_to()),
                )
            }
        };
        for ch in valid.chars() {
            match ch {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                '\u{7}' => out.push_str("\\a"),
                '\u{8}' => out.push_str("\\b"),
                '\u{c}' => out.push_str("\\f"),
                '\u{b}' => out.push_str("\\v"),
                ch if ch.is_control() && ch <= '\u{7f}' => {
                    out.push_str(&format!("\\x{:02x}", ch as u32))
                }
                ch if ch.is_control() || matches!(ch, '\u{2028}' | '\u{2029}' | '\u{feff}') => {
                    let value = ch as u32;
                    if value <= 0xffff {
                        out.push_str(&format!("\\u{value:04x}"));
                    } else {
                        out.push_str(&format!("\\U{value:08x}"));
                    }
                }
                ch => out.push(ch),
            }
        }
        let start = valid.len();
        for byte in &value[start..start + invalid] {
            out.push_str(&format!("\\x{byte:02x}"));
        }
        value = &value[start + invalid..];
    }
    out.push('"');
    out
}
