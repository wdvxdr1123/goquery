//! gopls-style doc-link tokens inside comments.
//!
//! A `[Symbol]`, `[pkg.Symbol]` or `[Type.Member]` link that resolves to a
//! package-scope object, imported object, field or method of a named type is
//! emitted as object tokens separated by comment tokens. Everything else stays
//! a plain comment token, matching `tokenVisitor.comment`.
//!
//! Lookup of doc-link names in package scope and imports.
//!
//! Mirrors `tokenVisitor.comment`'s `lookupObjects`: an optional import
//! qualifier, then either a package-scope name or `Type.Member`. Unresolved links stay comments. Bare non-exported names also stay comments;
//! members of a named package type follow native package visibility.
use crate::analysis::obj::{DeclId, MethodOrigin, ObjectRef};
use crate::analysis::resolve;
use crate::analysis::types::TypeArena;
use crate::syntax::TypeConstructor;
use std::ops::Range;
use std::path::Path;

use super::SemanticTokenKind;

const COMMENT: SemanticTokenKind = SemanticTokenKind::Comment;

pub(super) fn split(
    arena: &mut TypeArena<'_>,
    file: &Path,
    source: &str,
    span: &Range<usize>,
) -> Vec<(Range<usize>, SemanticTokenKind, u64)> {
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
        split_line(arena, file, line, offset, line_end, &mut tokens);
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

fn godirective(text: &str, start: usize) -> Option<Vec<(Range<usize>, SemanticTokenKind, u64)>> {
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
    tokens.push((
        namespace_start..namespace_end,
        SemanticTokenKind::Namespace,
        0,
    ));
    if text.as_bytes().get(namespace_end - start) == Some(&b' ')
        && namespace_end < start + text.len()
    {
        tokens.push((namespace_end + 1..start + text.len(), COMMENT, 0));
    }
    Some(tokens)
}

fn split_line(
    arena: &mut TypeArena<'_>,
    file: &Path,
    line: &str,
    base: usize,
    line_end: usize,
    tokens: &mut Vec<(Range<usize>, SemanticTokenKind, u64)>,
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
        let objects = lookup(arena, file, name);
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

struct Found {
    name: String,
    kind: SemanticTokenKind,
    modifiers: u64,
}

fn lookup(arena: &mut TypeArena<'_>, file: &Path, name: &str) -> Vec<Found> {
    let parts: Vec<&str> = name.split('.').collect();
    if parts.is_empty() || parts.len() > 3 {
        return Vec::new();
    }
    if parts.len() > 1 {
        if import_path(arena, file, parts[0]).is_some() {
            return imported(arena, file, parts[0], &parts[1..]);
        }
        // `T.M` resolves in the current package.
        return current_member(arena, file, parts[0], parts[1], parts.len() == 3);
    }
    let Some((_, symbol)) = arena.resolver().package_symbol(file, parts[0]) else {
        return Vec::new();
    };
    if !exported(&symbol.name) {
        return Vec::new();
    }
    let (kind, modifiers) = style(symbol);
    vec![Found {
        name: parts[0].to_owned(),
        kind,
        modifiers,
    }]
}

/// Resolves an import alias to its canonical import path.
fn import_path<'a>(arena: &TypeArena<'a>, file: &Path, alias: &str) -> Option<&'a str> {
    let resolver = arena.resolver();
    arena
        .workspace()
        .files
        .get(file)?
        .imports
        .iter()
        .find(|import| {
            !import.is_dot()
                && !import.is_blank()
                && resolver.import_name(file, import) == Some(alias)
        })
        .map(|import| import.path.as_str())
}

fn imported(arena: &mut TypeArena<'_>, file: &Path, alias: &str, rest: &[&str]) -> Vec<Found> {
    let mut objects = vec![Found {
        name: alias.to_owned(),
        kind: SemanticTokenKind::Namespace,
        modifiers: 0,
    }];
    if rest.len() == 1 {
        let name = rest[0];
        let Some((_, symbol)) = arena
            .resolver()
            .imported_symbol_candidates(
                file,
                alias,
                Some(crate::bloom::Fingerprint::new(name.as_bytes())),
            )
            .find(|(_, symbol)| symbol.name == name && symbol.receiver.is_none())
        else {
            return Vec::new();
        };
        if !exported(name) {
            return Vec::new();
        }
        let (kind, modifiers) = style(symbol);
        objects.push(Found {
            name: name.to_owned(),
            kind,
            modifiers,
        });
        return objects;
    }
    if rest.len() != 2 {
        return Vec::new();
    }
    let (receiver_name, member_name) = (rest[0], rest[1]);
    let Some((origin, receiver)) = arena
        .resolver()
        .resolve_type_declaration(file, &format!("{alias}.{receiver_name}"))
        .filter(|(_, receiver)| !receiver.alias)
    else {
        return Vec::new();
    };
    let Some(member) = arena.declaration_member(
        DeclId {
            file: origin,
            symbol: receiver,
        },
        file,
        member_name,
    ) else {
        return Vec::new();
    };
    let (kind, modifiers) = style(receiver);
    objects.push(Found {
        name: receiver_name.to_owned(),
        kind,
        modifiers,
    });
    let Some((kind, modifiers)) = member_style(member) else {
        return Vec::new();
    };
    objects.push(Found {
        name: member_name.to_owned(),
        kind,
        modifiers,
    });
    objects
}

fn current_member(
    arena: &mut TypeArena<'_>,
    file: &Path,
    receiver_name: &str,
    member_name: &str,
    has_extra: bool,
) -> Vec<Found> {
    if has_extra {
        return Vec::new();
    }
    let Some((origin, receiver)) = arena
        .resolver()
        .package_symbol(file, receiver_name)
        .filter(|(_, symbol)| symbol.is_type() && !symbol.alias)
    else {
        return Vec::new();
    };
    let Some(member) = arena.declaration_member(
        DeclId {
            file: origin,
            symbol: receiver,
        },
        file,
        member_name,
    ) else {
        return Vec::new();
    };
    let (kind, modifiers) = style(receiver);
    let mut objects = vec![Found {
        name: receiver_name.to_owned(),
        kind,
        modifiers,
    }];
    let Some((kind, modifiers)) = member_style(member) else {
        return Vec::new();
    };
    objects.push(Found {
        name: member_name.to_owned(),
        kind,
        modifiers,
    });
    objects
}

fn member_style(member: ObjectRef<'_>) -> Option<(SemanticTokenKind, u64)> {
    use super::modifiers::SIGNATURE;
    match member {
        ObjectRef::Method(MethodOrigin::Concrete(decl)) => Some(style(decl.symbol)),
        ObjectRef::Method(MethodOrigin::Interface(_)) => {
            Some((SemanticTokenKind::Method, SIGNATURE))
        }
        ObjectRef::Field(_) => Some((SemanticTokenKind::Property, 0)),
        _ => None,
    }
}

/// Maps a Go declaration symbol to its semantic token type and modifiers.
pub(super) fn style(symbol: &crate::index::IndexedSymbol) -> (SemanticTokenKind, u64) {
    use super::modifiers::{READONLY, SIGNATURE, STATIC};
    let mut modifiers = 0u64;
    let kind = if symbol.is_type() {
        modifiers |= constructor_modifiers(&symbol.constructor);
        SemanticTokenKind::Type
    } else if symbol.is_const() {
        modifiers |= READONLY;
        SemanticTokenKind::Variable
    } else if symbol.is_var() {
        modifiers |= STATIC;
        SemanticTokenKind::Variable
    } else if symbol.is_function() {
        modifiers |= SIGNATURE;
        SemanticTokenKind::Function
    } else if symbol.is_method() {
        modifiers |= SIGNATURE;
        SemanticTokenKind::Method
    } else {
        SemanticTokenKind::Variable
    };
    (kind, modifiers)
}

fn constructor_modifiers(constructor: &TypeConstructor) -> u64 {
    use super::modifiers::{ARRAY, CHAN, INTERFACE, MAP, POINTER, SIGNATURE, SLICE, STRUCT};
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
