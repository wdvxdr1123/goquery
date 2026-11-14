//! Import validated type syntax into compact nodes with declaration identities.
use super::{
    BasicKind, DeclRef, Direction, MAX_BYTES, MAX_DEPTH, MAX_NODES, Ty, TypeArena, TypeId,
};
use crate::syntax::{Lexer, Token, TokenKind};
use std::iter::Peekable;
use std::path::Path;

impl<'a> TypeArena<'a> {
    pub(crate) fn parse(&mut self, origin: &Path, source: &str) -> Option<TypeId> {
        self.parse_in(origin, source, &[])
    }

    pub(crate) fn parse_in(
        &mut self,
        origin: &Path,
        source: &str,
        environment: &[(&str, TypeId)],
    ) -> Option<TypeId> {
        if source.len() > MAX_BYTES || !crate::syntax::parse_type(source).diagnostics.is_empty() {
            return None;
        }
        let mut tokens = Lexer::new(source).peekable();
        let ty = self.read(origin, source, &mut tokens, environment, 0)?;
        tokens
            .all(|token| token.kind == TokenKind::Semicolon)
            .then_some(ty)
    }

    pub(super) fn read(
        &mut self,
        origin: &Path,
        source: &str,
        tokens: &mut Peekable<Lexer<'_>>,
        environment: &[(&str, TypeId)],
        depth: usize,
    ) -> Option<TypeId> {
        if depth >= MAX_DEPTH {
            return None;
        }
        let first = tokens.next()?;
        let ty = match first.kind {
            TokenKind::Interface => {
                let suffix = &source[first.span.start..];
                let parsed = crate::syntax::parse_type_prefix(suffix);
                if !parsed.diagnostics.is_empty() {
                    return None;
                }
                let end = first.span.start + parsed.span.end;
                while tokens.peek().is_some_and(|t| t.span.start < end) {
                    tokens.next();
                }
                return self.read_interface(
                    origin,
                    &source[first.span.start..end],
                    environment,
                    depth + 1,
                );
            }
            TokenKind::Func => {
                let suffix = &source[first.span.start..];
                let parsed = crate::syntax::parse_type_prefix(suffix);
                if !parsed.diagnostics.is_empty() {
                    return None;
                }
                let end = first.span.start + parsed.span.end;
                while tokens.peek().is_some_and(|token| token.span.start < end) {
                    tokens.next();
                }
                let header = &source[first.span.start..end];
                let signature = crate::syntax::parse_literal_signature(header);
                return self.signature(origin, header, &signature, environment, depth + 1);
            }
            TokenKind::Struct => {
                take(tokens, TokenKind::LBrace)?;
                let mut nesting = 1usize;
                let end = loop {
                    let token = tokens.next()?;
                    match token.kind {
                        TokenKind::LBrace => nesting += 1,
                        TokenKind::RBrace => {
                            nesting -= 1;
                            if nesting == 0 {
                                break token.span.end;
                            }
                        }
                        _ => {}
                    }
                };
                return self.read_struct(
                    origin,
                    &source[first.span.start..end],
                    environment,
                    depth,
                );
            }
            TokenKind::Mul => {
                Ty::Pointer(self.read(origin, source, tokens, environment, depth + 1)?)
            }
            TokenKind::LParen => {
                let ty = self.read(origin, source, tokens, environment, depth + 1)?;
                take(tokens, TokenKind::RParen)?;
                return Some(ty);
            }
            TokenKind::LBrack => {
                let start = first.span.end;
                let mut end = start;
                let mut nesting = 0usize;
                loop {
                    let token = tokens.next()?;
                    match token.kind {
                        TokenKind::LBrack | TokenKind::LParen | TokenKind::LBrace => nesting += 1,
                        TokenKind::RBrack if nesting == 0 => break,
                        TokenKind::RBrack | TokenKind::RParen | TokenKind::RBrace => {
                            nesting = nesting.checked_sub(1)?
                        }
                        _ => {}
                    }
                    end = token.span.end;
                }
                let element = self.read(origin, source, tokens, environment, depth + 1)?;
                if start == end {
                    Ty::Slice(element)
                } else {
                    // Constant evaluation is not implemented; only decimal lengths
                    // have a known canonical rendering in this first increment.
                    let length = source[start..end].trim();
                    if length.is_empty() || !length.bytes().all(|byte| byte.is_ascii_digit()) {
                        return None;
                    }
                    if length.len() > 1 && length.starts_with('0') {
                        return None;
                    }
                    let length = length.parse().ok()?;
                    let maximum = (1_u64 << (self.workspace.integer_bits()? - 1)) - 1;
                    if length > maximum {
                        return None;
                    }
                    Ty::Array { length, element }
                }
            }
            TokenKind::Map => {
                take(tokens, TokenKind::LBrack)?;
                let key = self.read(origin, source, tokens, environment, depth + 1)?;
                if self.comparable(key, depth + 1) == Some(false) {
                    self.invalid_type = true;
                    return None;
                }
                take(tokens, TokenKind::RBrack)?;
                let value = self.read(origin, source, tokens, environment, depth + 1)?;
                Ty::Map { key, value }
            }
            TokenKind::Chan | TokenKind::Arrow => {
                let direction = if first.kind == TokenKind::Arrow {
                    take(tokens, TokenKind::Chan)?;
                    Direction::Receive
                } else if tokens
                    .peek()
                    .is_some_and(|token| token.kind == TokenKind::Arrow)
                {
                    tokens.next();
                    Direction::Send
                } else {
                    Direction::Both
                };
                Ty::Chan {
                    direction,
                    element: self.read(origin, source, tokens, environment, depth + 1)?,
                }
            }
            TokenKind::Ident => {
                let mut end = first.span.end;
                if tokens
                    .peek()
                    .is_some_and(|token| token.kind == TokenKind::Period)
                {
                    tokens.next();
                    end = take(tokens, TokenKind::Ident)?.span.end;
                }
                let name = &source[first.span.start..end];
                if name == "_" {
                    return None;
                }
                if let Some((_, ty)) = environment.iter().find(|(parameter, _)| *parameter == name)
                {
                    return Some(*ty);
                }
                let declaration = if name.contains('.') {
                    self.workspace.type_declaration(origin, name)
                } else {
                    // Missing file scope cannot prove an unshadowed universe name.
                    self.workspace.files.get(origin)?;
                    if crate::analysis::scope::universe::imported_name(self.workspace, origin, name)
                    {
                        return None;
                    }
                    let declaration = self.workspace.unqualified_symbol(origin, name);
                    if declaration.is_some_and(|(_, symbol)| !symbol.is_type()) {
                        return None;
                    }
                    declaration
                };
                if declaration.is_none() && matches!(name, "any" | "error") {
                    return self.builtin_interface(origin, name == "error");
                }
                if declaration.is_none()
                    && let Some(kind) = BasicKind::named(name)
                {
                    Ty::Basic(kind)
                } else {
                    let (file, symbol) = declaration?;
                    // unsafe.Pointer is a compiler-defined basic type. Its SDK
                    // documentation declaration is not an ordinary pointer.
                    if symbol.name == "Pointer"
                        && self.workspace.import_path(file).as_deref() == Some("unsafe")
                    {
                        return None;
                    }
                    let mut arguments = Vec::new();
                    if tokens
                        .peek()
                        .is_some_and(|token| token.kind == TokenKind::LBrack)
                    {
                        tokens.next();
                        loop {
                            if arguments.len() >= MAX_NODES {
                                return None;
                            }
                            arguments.push(self.read(
                                origin,
                                source,
                                tokens,
                                environment,
                                depth + 1,
                            )?);
                            if tokens.peek()?.kind != TokenKind::Comma {
                                break;
                            }
                            tokens.next();
                            if tokens.peek()?.kind == TokenKind::RBrack {
                                break;
                            }
                        }
                        take(tokens, TokenKind::RBrack)?;
                    }
                    if self.arguments.len() + arguments.len() > MAX_NODES {
                        return None;
                    }
                    let start = self.arguments.len();
                    self.arguments.extend(arguments);
                    Ty::Named {
                        declaration: DeclRef { file, symbol },
                        arguments: start..self.arguments.len(),
                    }
                }
            }
            _ => return None,
        };
        self.push(ty)
    }
}

fn take(tokens: &mut Peekable<Lexer<'_>>, kind: TokenKind) -> Option<Token> {
    tokens.next().filter(|token| token.kind == kind)
}

pub(crate) fn declaration_body(declaration: &str) -> Option<&str> {
    let mut tokens = crate::syntax::Lexer::new(declaration).peekable();
    if let Some(end) = crate::syntax::type_parameter_end(declaration) {
        while tokens.peek().is_some_and(|token| token.span.start < end) {
            tokens.next();
        }
    }
    if tokens.peek()?.kind == crate::syntax::TokenKind::Assign {
        tokens.next();
    }
    let start = tokens.next()?.span.start;
    Some(&declaration[start..])
}
