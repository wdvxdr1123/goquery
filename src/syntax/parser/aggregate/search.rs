//! Search-only aggregate walk; never builds expression or function-body trees.
use crate::syntax::ast::{AggregateMember, MemberKind, MemberName};
use crate::syntax::lexer::Lexer;
use crate::syntax::token::{Token, TokenKind};
use std::iter::Peekable;
use std::ops::Range;

pub(super) fn root(source: &str) -> Option<(usize, bool)> {
    let mut tokens = Lexer::new(source).peekable();
    if tokens.peek()?.kind == TokenKind::LBrack {
        let end = super::named::parameter_end(source)?;
        while tokens.peek().is_some_and(|token| token.span.start < end) {
            tokens.next();
        }
    }
    if tokens.peek()?.kind == TokenKind::Assign {
        tokens.next();
    }
    let token = tokens.next()?;
    matches!(token.kind, TokenKind::Struct | TokenKind::Interface)
        .then_some((token.span.start, token.kind == TokenKind::Interface))
}

pub(super) fn walk(
    source: &str,
    offset: usize,
    path: &str,
    depth: usize,
    visit: &mut impl FnMut(AggregateMember, bool),
) {
    let mut sink = Sink {
        remaining: 4096,
        name_bytes: 256 * 1024,
        visit,
    };
    walk_inner(source, offset, path, depth, &mut sink);
}

struct Sink<'a, F> {
    remaining: usize,
    name_bytes: usize,
    visit: &'a mut F,
}

impl<F: FnMut(AggregateMember, bool)> Sink<'_, F> {
    fn emit(
        &mut self,
        name: String,
        kind: MemberKind,
        selection: Range<usize>,
        annotation: Range<usize>,
        container: usize,
        direct: bool,
    ) {
        if self.remaining == 0 || name.len() > self.name_bytes {
            self.remaining = 0;
            return;
        }
        self.remaining -= 1;
        self.name_bytes -= name.len();
        (self.visit)(
            AggregateMember {
                symbol: MemberName { name, kind },
                selection,
                annotation,
                container,
            },
            direct,
        );
    }
}

fn walk_inner(
    source: &str,
    offset: usize,
    path: &str,
    depth: usize,
    visit: &mut Sink<'_, impl FnMut(AggregateMember, bool)>,
) {
    if visit.remaining == 0 || depth >= 64 || path.len() > 16 * 1024 {
        return;
    }
    let mut tokens = Lexer::new(&source[offset..]).peekable();
    let Some(kind) = tokens.next().map(|token| token.kind) else {
        return;
    };
    if !matches!(kind, TokenKind::Struct | TokenKind::Interface)
        || tokens
            .next()
            .is_none_or(|token| token.kind != TokenKind::LBrace)
    {
        return;
    }
    let interface = kind == TokenKind::Interface;
    loop {
        if visit.remaining == 0 {
            return;
        }
        let mut header = Vec::new();
        let mut end = 0;
        while let Some(token) = tokens.peek() {
            if matches!(token.kind, TokenKind::Semicolon | TokenKind::RBrace) {
                break;
            }
            let token = tokens.next().expect("peeked token exists");
            end = if opening(token.kind) {
                group(&mut tokens, &token)
            } else {
                token.span.end
            };
            if header.len() == 8192 {
                return;
            }
            header.push(token);
        }
        if !header.is_empty() {
            field(
                source,
                offset,
                path,
                depth,
                interface,
                (&header, end),
                visit,
            );
        }
        if tokens
            .next()
            .is_none_or(|token| token.kind == TokenKind::RBrace)
        {
            break;
        }
    }
}

fn field(
    source: &str,
    offset: usize,
    path: &str,
    depth: usize,
    interface: bool,
    (header, end): (&[Token], usize),
    visit: &mut Sink<'_, impl FnMut(AggregateMember, bool)>,
) {
    let first = &header[0];
    let named = first.kind == TokenKind::Ident
        && if interface {
            header
                .get(1)
                .is_some_and(|token| token.kind == TokenKind::LParen)
        } else {
            header.get(1).is_some_and(|token| {
                !matches!(token.kind, TokenKind::Period | TokenKind::String)
                    && (token.kind != TokenKind::LBrack || header.len() > 2)
            })
        };
    if named {
        let mut names = 1;
        while header
            .get(names)
            .is_some_and(|token| token.kind == TokenKind::Comma)
            && header
                .get(names + 1)
                .is_some_and(|token| token.kind == TokenKind::Ident)
        {
            names += 2;
        }
        let Some(annotation) = header.get(names) else {
            return;
        };
        for token in header[..names].iter().step_by(2) {
            if visit.remaining == 0 {
                break;
            }
            let name = qualify(
                path,
                &source[offset + token.span.start..offset + token.span.end],
            );
            if name.len() > 16 * 1024 {
                continue;
            }
            visit.emit(
                name.clone(),
                if interface {
                    MemberKind::Method
                } else {
                    MemberKind::Field
                },
                offset + token.span.start..offset + token.span.end,
                if interface {
                    offset + token.span.end..offset + end
                } else {
                    offset + annotation.span.start
                        ..offset
                            + header
                                .last()
                                .filter(|token| token.kind == TokenKind::String)
                                .map_or(end, |token| token.span.start)
                },
                offset,
                depth == 0,
            );
            if !interface
                && let Some(ty) = header.get(names)
                && matches!(ty.kind, TokenKind::Struct | TokenKind::Interface)
            {
                walk_inner(source, offset + ty.span.start, &name, depth + 1, visit);
            }
        }
    } else {
        let type_end = header
            .last()
            .filter(|token| token.kind == TokenKind::String)
            .map_or(end, |token| token.span.start);
        let text = &source[offset + first.span.start..offset + type_end];
        let name = if first.kind == TokenKind::Ident
            && header
                .get(1)
                .is_some_and(|token| token.kind == TokenKind::Period)
            && header
                .get(2)
                .is_some_and(|token| token.kind == TokenKind::Ident)
            && (header.len() == 3 || header.len() == 4 && header[3].kind == TokenKind::String)
        {
            let token = &header[2];
            source[offset + token.span.start..offset + token.span.end].to_owned()
        } else {
            let Some(name) = type_name(text) else {
                return;
            };
            name
        };
        let name = qualify(path, &name);
        if name.len() <= 16 * 1024 {
            visit.emit(
                name,
                if interface {
                    MemberKind::Interface
                } else {
                    MemberKind::Field
                },
                offset + first.span.start..offset + end,
                offset + first.span.start..offset + type_end,
                offset,
                false,
            );
        }
    }
}

fn qualify(path: &str, name: &str) -> String {
    if path.is_empty() {
        name.to_owned()
    } else {
        format!("{path}.{name}")
    }
}

fn type_name(source: &str) -> Option<String> {
    let mut text = String::new();
    for token in Lexer::new(source).filter(|token| token.kind != TokenKind::Semicolon) {
        if text.len() + token.span.len() + 3 > 16 * 1024 {
            return None;
        }
        if token.kind == TokenKind::Or {
            text.push_str(" | ");
        } else {
            text.push_str(&source[token.span]);
            if token.kind == TokenKind::Comma {
                text.push(' ');
            }
        }
    }
    Some(text)
}

fn opening(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::LParen | TokenKind::LBrack | TokenKind::LBrace
    )
}

fn group(tokens: &mut Peekable<Lexer<'_>>, first: &Token) -> usize {
    let mut depth = 1;
    let mut end = first.span.end;
    for token in tokens.by_ref() {
        end = token.span.end;
        if opening(token.kind) {
            depth += 1;
        } else if matches!(
            token.kind,
            TokenKind::RParen | TokenKind::RBrack | TokenKind::RBrace
        ) {
            depth -= 1;
            if depth == 0 {
                break;
            }
        }
    }
    end
}
