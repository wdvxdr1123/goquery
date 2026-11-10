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
