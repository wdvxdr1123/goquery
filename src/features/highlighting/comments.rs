//! gopls-style doc-link tokens inside comments.
//!
//! A `[Symbol]`, `[pkg.Symbol]` or `[Type.Method]` link that resolves to a
//! package-scope object, an imported object, or a method of a named type is
//! emitted as object tokens separated by comment tokens. Everything else stays
//! a plain comment token, matching `tokenVisitor.comment`.
use crate::analysis::resolve;
use crate::syntax::TypeConstructor;
use crate::workspace::Workspace;
use std::ops::Range;
use std::path::Path;

mod lookup;
use lookup::lookup;

use super::config::SemanticType;

const COMMENT: SemanticType = SemanticType::Comment;

pub(super) fn split(
    workspace: &Workspace,
    file: &Path,
    source: &str,
    span: &Range<usize>,
) -> Vec<(Range<usize>, SemanticType, u64)> {
    let text = &source[span.clone()];
    // A known `//go:directive` highlights the directive name as a namespace.
    if let Some(tokens) = godirective(text, span.start) {
        return tokens;
    }
    // Plain comments without bracket references keep a single token.
    if !text.contains('[') {
        return vec![(span.clone(), COMMENT, 0)];
    }
    let mut tokens = Vec::new();
    let mut offset = span.start;
    for line in text.split('\n') {
        let line_end = offset + line.len();
        split_line(workspace, file, line, offset, line_end, &mut tokens);
        offset = line_end + 1;
    }
    if tokens.is_empty() {
        vec![(span.clone(), COMMENT, 0)]
    } else {
        tokens
    }
}

/// Known compiler/build directives from gopls' `godirectives` list.
const GODIRECTIVES: [&str; 9] = [
    "noescape",
    "uintptrescapes",
    "noinline",
    "norace",
    "nosplit",
    "linkname",
    "build",
    "binary-only-package",
    "embed",
];

fn godirective(text: &str, start: usize) -> Option<Vec<(Range<usize>, SemanticType, u64)>> {
    let rest = text.strip_prefix("//go:")?;
    let directive = rest.split(' ').next().unwrap_or(rest);
    if !GODIRECTIVES.contains(&directive) {
        return None;
    }
    // `//go:directive`: the leading `//` stays comment, `go:directive` is a
    // namespace token, and the remaining arguments are comment text.
    let namespace_start = start + 2;
    let namespace_end = namespace_start + 3 + directive.len();
    let mut tokens = vec![(start..namespace_start, COMMENT, 0)];
    tokens.push((namespace_start..namespace_end, SemanticType::Namespace, 0));
    if text.as_bytes().get(namespace_end - start) == Some(&b' ')
        && namespace_end < start + text.len()
    {
        tokens.push((namespace_end + 1..start + text.len(), COMMENT, 0));
    }
    Some(tokens)
}

fn split_line(
    workspace: &Workspace,
    file: &Path,
    line: &str,
    base: usize,
    line_end: usize,
    tokens: &mut Vec<(Range<usize>, SemanticType, u64)>,
) {
    let bytes = line.as_bytes();
    let mut last = 0usize;
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] != b'[' {
            index += 1;
            continue;
        }
        let Some((name_start, name_end)) = link_name(bytes, index) else {
            index += 1;
            continue;
        };
        let name = &line[name_start..name_end];
        let objects = lookup(workspace, file, name);
        if objects.is_empty() {
            index += 1;
            continue;
        }
        if last < name_start {
            tokens.push((base + last..base + name_start, COMMENT, 0));
        }
        let mut cursor = name_start;
        for (position, object) in objects.iter().enumerate() {
            if position > 0 {
                tokens.push((base + cursor..base + cursor + 1, COMMENT, 0));
                cursor += 1;
            }
            let length = object.name.len();
            tokens.push((
                base + cursor..base + cursor + length,
                object.kind,
                object.modifiers,
            ));
            cursor += length;
        }
        // gopls resumes the trailing comment at the end of the link name, so
        // the closing `]` stays inside the comment token.
        last = name_end;
        index = name_end;
    }
    if last < line.len() {
        tokens.push((base + last..line_end, COMMENT, 0));
    }
}

/// Parses `[*?name(.name){0,2}]` starting at `bracket`; returns the name range.
fn link_name(bytes: &[u8], bracket: usize) -> Option<(usize, usize)> {
    let mut index = bracket + 1;
    if bytes.get(index) == Some(&b'*') {
        index += 1;
    }
    let start = index;
    let mut components = 0usize;
    loop {
        let first = *bytes.get(index)?;
        if !(first.is_ascii_alphabetic() || first == b'_' || first >= 0x80) {
            return None;
        }
        index += 1;
        while matches!(
            bytes.get(index),
            Some(byte) if byte.is_ascii_alphanumeric() || *byte == b'_' || *byte >= 0x80
        ) {
            index += 1;
        }
        components += 1;
        if bytes.get(index) == Some(&b'.') && components < 3 {
            index += 1;
            continue;
        }
        break;
    }
    (bytes.get(index) == Some(&b']')).then_some((start, index))
}

/// Maps a Go declaration symbol to its semantic token type and modifiers.
pub(super) fn style(symbol: &crate::index::IndexedSymbol) -> (super::config::SemanticType, u64) {
    use super::config::{READONLY, SIGNATURE, STATIC, SemanticType};
    let mut modifiers = 0u64;
    let kind = if symbol.is_type() {
        modifiers |= constructor_modifiers(&symbol.constructor);
        SemanticType::Type
    } else if symbol.is_const() {
        modifiers |= READONLY;
        SemanticType::Variable
    } else if symbol.is_var() {
        modifiers |= STATIC;
        SemanticType::Variable
    } else if symbol.is_function() {
        modifiers |= SIGNATURE;
        SemanticType::Function
    } else if symbol.is_method() {
        modifiers |= SIGNATURE;
        SemanticType::Method
    } else {
        SemanticType::Variable
    };
    (kind, modifiers)
}

fn constructor_modifiers(constructor: &TypeConstructor) -> u64 {
    use super::config::{ARRAY, CHAN, INTERFACE, MAP, POINTER, SIGNATURE, SLICE, STRUCT};
    match constructor {
        TypeConstructor::Struct => STRUCT,
        TypeConstructor::Interface => INTERFACE,
        TypeConstructor::Map => MAP,
        TypeConstructor::Chan => CHAN,
        TypeConstructor::Func => SIGNATURE,
        TypeConstructor::Pointer => POINTER,
        TypeConstructor::Array => ARRAY,
        TypeConstructor::Slice => SLICE,
        TypeConstructor::Named | TypeConstructor::Unknown => 0,
    }
}

/// Exported-name guard matching `go/types.Object.Exported` for doc links.
pub(super) fn exported(name: &str) -> bool {
    resolve::exported(name)
}
