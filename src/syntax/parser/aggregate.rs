//! Extracts shallow named embeddings with bounded per-member token storage.
mod named;
mod search;
use crate::syntax::ast::declarations::{TypeConstructor, TypeInfo};
use crate::syntax::lexer::Lexer;
use crate::syntax::token::{Token, TokenKind};

pub(super) fn type_info(
    source: &str,
    constructor: TypeConstructor,
    offset: usize,
) -> Option<Box<TypeInfo>> {
    if matches!(
        constructor,
        TypeConstructor::Named | TypeConstructor::Pointer | TypeConstructor::Unknown
    ) && let Some(target) = named::target(source)
    {
        return Some(Box::new(TypeInfo {
            constructor,
            embeddings_complete: true,
            target: Some(target),
            embedded: Vec::new(),
            members: Vec::new(),
            search_members: Vec::new(),
        }));
    }
    let (offset, constructor) = match constructor {
        TypeConstructor::Struct | TypeConstructor::Interface if offset == usize::MAX => {
            return None;
        }
        TypeConstructor::Struct | TypeConstructor::Interface => (offset, constructor),
        TypeConstructor::Unknown => {
            let (offset, interface) = search::root(source)?;
            let constructor = if interface {
                TypeConstructor::Interface
            } else {
                TypeConstructor::Struct
            };
            (offset, constructor)
        }
        _ => return None,
    };
    // Type arguments affect member types, not their declaration identities.
    let scanned = scan(&source[offset..]);
    let semantic = scanned.is_some();
    let mut info = scanned.unwrap_or_else(|| {
        Box::new(TypeInfo {
            constructor,
            embeddings_complete: false,
            target: None,
            embedded: Vec::new(),
            members: Vec::new(),
            search_members: Vec::new(),
        })
    });
    search::walk(source, offset, "", 0, &mut |member, direct| {
        if semantic
            && member.container == offset
            && member.selection.start == member.annotation.start
            && let Some((field, embedding)) = generic_embedding(&source[member.annotation.clone()])
        {
            // The shallow scanner treats brackets as opaque. Recover the
            // embedded identity without retaining an argument syntax tree.
            info.members.retain(|name| *name != field);
            info.embedded.push(embedding);
        }
        if !direct || info.members.is_empty() {
            info.search_members.push(member.symbol);
        }
    });
    Some(info)
}

fn generic_embedding(source: &str) -> Option<(String, String)> {
    if source.len() > 16 * 1024 {
        return None;
    }
    let mut tokens = Lexer::new(source).peekable();
    if tokens.peek()?.kind == TokenKind::Mul {
        tokens.next();
    }
    let token = tokens
        .next()
        .filter(|token| token.kind == TokenKind::Ident)?;
    let mut field = source[token.span].to_owned();
    let mut name = field.clone();
    if tokens
        .peek()
        .is_some_and(|token| token.kind == TokenKind::Period)
    {
        tokens.next();
        let token = tokens
            .next()
            .filter(|token| token.kind == TokenKind::Ident)?;
        field = source[token.span].to_owned();
        name.push('.');
        name.push_str(&field);
    }
    let opening = tokens
        .next()
        .filter(|token| token.kind == TokenKind::LBrack)?;
    // Store tokens, not arbitrary whitespace/comments from the closed source.
    // Separators preserve adjacent identifiers in anonymous argument types.
    name.push('[');
    let arguments = source[opening.span.end..].trim_end();
    for token in Lexer::new(arguments) {
        if token.inserted && token.span.start == arguments.len() {
            continue;
        }
        name.push(' ');
        if token.kind == TokenKind::Semicolon {
            name.push(';');
        } else {
            name.push_str(&arguments[token.span]);
        }
        if name.len() > 16 * 1024 {
            return None;
        }
    }
    Some((field, name))
}

/// Returns aggregate search names and spans, including nested/embedded members.
/// Generic parameter lists and alias markers may precede the type expression.
/// Walking is bounded to 64 nested aggregates, 4,096 names, 256 KiB of name
/// bytes, 16 KiB per name and 8,192 header tokens per member. Results are partial
/// when a bound is reached.
pub fn aggregate_members(source: &str) -> Vec<crate::syntax::ast::AggregateMember> {
    let mut names = Vec::new();
    if let Some((offset, _)) = search::root(source) {
        search::walk(source, offset, "", 0, &mut |member, _| names.push(member));
    }
    names
}

/// Byte offset of the root aggregate after optional type parameters or `=`.
pub fn aggregate_offset(source: &str) -> Option<usize> {
    search::root(source).map(|(offset, _)| offset)
}

/// End of a proven declaration parameter list, excluding array/slice syntax.
pub fn type_parameter_end(source: &str) -> Option<usize> {
    named::parameter_end(source)
}

fn scan(source: &str) -> Option<Box<TypeInfo>> {
    let mut tokens = Lexer::new(source).peekable();
    // Generic parameter identities require substitution-aware type resolution.
    if tokens.peek()?.kind == TokenKind::LBrack {
        return None;
    }
    let kind = tokens.next()?.kind;
    if !matches!(kind, TokenKind::Struct | TokenKind::Interface)
        || tokens.next()?.kind != TokenKind::LBrace
    {
        return None;
    }
    let mut embedded = Vec::new();
    let mut members = Vec::new();
    let mut names = Vec::new();
    let mut leading = true;
    let mut expect_name = true;
    let mut member = Vec::new();
    let mut depth = 0;
    let mut opaque = false;
    let mut complete = true;
    let mut closed = false;
    let mut method = false;
    let mut explicit_field = false;
    let mut bracket_field = false;
    let mut after_brackets = false;
    for token in tokens {
        if depth == 0 && matches!(token.kind, TokenKind::Semicolon | TokenKind::RBrace) {
            complete &= !opaque || method || explicit_field;
            if !opaque && let Some(name) = embedded_name(source, &member) {
                embedded.push(name);
            } else {
                members.extend(
                    names
                        .drain(..)
                        .map(|token: Token| source[token.span].to_owned()),
                );
            }
            names.clear();
            leading = true;
            expect_name = true;
            member.clear();
            opaque = false;
            method = false;
            explicit_field = false;
            bracket_field = false;
            after_brackets = false;
            if token.kind == TokenKind::RBrace {
                closed = true;
                break;
            }
            continue;
        }
        if depth == 0 && leading {
            if expect_name && token.kind == TokenKind::Ident {
                names.push(token.clone());
                expect_name = false;
            } else if !expect_name && token.kind == TokenKind::Comma {
                expect_name = true;
            } else {
                leading = false;
                if kind == TokenKind::Struct && !names.is_empty() {
                    explicit_field = field_type_start(token.kind);
                    bracket_field = token.kind == TokenKind::LBrack;
                }
            }
        }
        // X[N] may be an embedded instantiation; X[N]T is an explicit field.
        // Complex named field annotations cannot hide a root embedding.
        if depth == 0 && bracket_field && after_brackets {
            explicit_field |= field_type_start(token.kind) || token.kind == TokenKind::LBrack;
        }
        method |= kind == TokenKind::Interface
            && depth == 0
            && token.kind == TokenKind::LParen
            && member.len() == 1
            && names.len() == 1;
        match token.kind {
            TokenKind::LParen | TokenKind::LBrack | TokenKind::LBrace => {
                depth += 1;
                opaque = true;
            }
            TokenKind::RParen | TokenKind::RBrack | TokenKind::RBrace => depth -= 1,
            _ => {}
        }
        if bracket_field && token.kind == TokenKind::RBrack && depth == 0 {
            after_brackets = true;
        }
        if depth == 0 && member.len() < 5 {
            member.push(token);
        } else {
            opaque = true;
        }
    }
    Some(Box::new(TypeInfo {
        constructor: if kind == TokenKind::Interface {
            TypeConstructor::Interface
        } else {
            TypeConstructor::Struct
        },
        embeddings_complete: complete && closed,
        target: None,
        embedded,
        members,
        search_members: Vec::new(),
    }))
}

fn field_type_start(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Ident
            | TokenKind::Mul
            | TokenKind::Struct
            | TokenKind::Interface
            | TokenKind::Func
            | TokenKind::Map
            | TokenKind::Chan
            | TokenKind::Arrow
    )
}

fn embedded_name(source: &str, tokens: &[Token]) -> Option<String> {
    let mut tokens = tokens.iter().peekable();
    if tokens.peek()?.kind == TokenKind::Mul {
        tokens.next();
    }
    let first = tokens.next().filter(|t| t.kind == TokenKind::Ident)?;
    let mut name = source[first.span.clone()].to_owned();
    if tokens.peek().is_some_and(|t| t.kind == TokenKind::Period) {
        tokens.next();
        let next = tokens.next().filter(|t| t.kind == TokenKind::Ident)?;
        name.push('.');
        name.push_str(&source[next.span.clone()]);
    }
    if tokens.peek().is_some_and(|t| t.kind == TokenKind::String) {
        tokens.next();
    }
    tokens.next().is_none().then_some(name)
}
