//! Import validated type syntax into compact nodes with declaration identities.
use super::{BasicKind, Direction, MAX_BYTES, MAX_DEPTH, Ty, TypeArena, TypeId};
use crate::analysis::obj::DeclId;
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
        self.parse_in_internal(origin, source, None, environment)
    }

    pub(crate) fn parse_in_at(
        &mut self,
        origin: &Path,
        source: &str,
        file_offset: usize,
        environment: &[(&str, TypeId)],
    ) -> Option<TypeId> {
        self.parse_in_internal(origin, source, Some(file_offset), environment)
    }

    pub(crate) fn builtin_type(&mut self, origin: &Path, name: &str) -> Option<TypeId> {
        match name {
            "any" => self.builtin_interface(origin, false),
            "error" => self.builtin_interface(origin, true),
            _ => BasicKind::named(name).and_then(|kind| self.basic(kind)),
        }
    }

    pub(super) fn parse_in_internal(
        &mut self,
        origin: &Path,
        source: &str,
        file_offset: Option<usize>,
        environment: &[(&str, TypeId)],
    ) -> Option<TypeId> {
        if source.len() > MAX_BYTES || !crate::syntax::parse_type(source).diagnostics.is_empty() {
            return None;
        }
        let mut tokens = Lexer::new(source).peekable();
        let ty = self.read_at(origin, source, file_offset, &mut tokens, environment, 0)?;
        tokens
            .all(|token| token.kind == TokenKind::Semicolon)
            .then_some(ty)
    }

    pub(super) fn read_at(
        &mut self,
        origin: &Path,
        source: &str,
        file_offset: Option<usize>,
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
                return self.read_interface_at(
                    origin,
                    &source[first.span.start..end],
                    file_offset.map(|base| base + first.span.start),
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
                return self.signature(
                    origin,
                    header,
                    file_offset.map(|base| base + first.span.start),
                    &signature,
                    environment,
                    depth + 1,
                );
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
                return self.read_struct_at(
                    origin,
                    &source[first.span.start..end],
                    file_offset.map(|base| base + first.span.start),
                    environment,
                    depth,
                );
            }
            TokenKind::Mul => Ty::Pointer(self.read_at(
                origin,
                source,
                file_offset,
                tokens,
                environment,
                depth + 1,
            )?),
            TokenKind::LParen => {
                let ty =
                    self.read_at(origin, source, file_offset, tokens, environment, depth + 1)?;
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
                let element =
                    self.read_at(origin, source, file_offset, tokens, environment, depth + 1)?;
                if start == end {
                    Ty::Slice(element)
                } else {
                    let length = source[start..end].trim();
                    let length = parse_symbolic_integer(length)
                        .or_else(|| self.eval_const_array_length(origin, length))?;
                    let maximum = (1_u64 << (self.integer_bits()? - 1)) - 1;
                    if length > maximum {
                        return None;
                    }
                    Ty::Array { length, element }
                }
            }
            TokenKind::Map => {
                take(tokens, TokenKind::LBrack)?;
                let key =
                    self.read_at(origin, source, file_offset, tokens, environment, depth + 1)?;
                if !self.symbolic && self.comparable(key, depth + 1) == Some(false) {
                    self.invalid_type = true;
                    return None;
                }
                take(tokens, TokenKind::RBrack)?;
                let value =
                    self.read_at(origin, source, file_offset, tokens, environment, depth + 1)?;
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
                    element: self.read_at(
                        origin,
                        source,
                        file_offset,
                        tokens,
                        environment,
                        depth + 1,
                    )?,
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
                    self.resolver().resolve_type_declaration(origin, name)
                } else {
                    // Missing file scope cannot prove an unshadowed universe name.
                    self.workspace.files.get(origin)?;
                    if self.resolver().imported_name(origin, name) {
                        return None;
                    }
                    let declaration = self.resolver().unqualified_symbol(origin, name);
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
                        if tokens
                            .peek()
                            .is_some_and(|token| token.kind == TokenKind::LBrack)
                        {
                            return None;
                        }
                        return self.basic(BasicKind::UnsafePointer);
                    }
                    if !self.symbolic
                        && matches!(symbol.name.as_str(), "ArbitraryType" | "IntegerType")
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
                            if arguments.len() >= self.node_limit {
                                return None;
                            }
                            arguments.push(self.read_at(
                                origin,
                                source,
                                file_offset,
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
                    return self.push_named(DeclId { file, symbol }, &arguments, symbol.alias);
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

fn parse_symbolic_integer(text: &str) -> Option<u64> {
    if Lexer::new(text)
        .filter(|token| token.kind != TokenKind::Semicolon)
        .count()
        != 1
    {
        return None;
    }
    let digits = text.replace('_', "");
    let (radix, body) = if digits.starts_with("0x") || digits.starts_with("0X") {
        (16, &digits[2..])
    } else if digits.starts_with("0b") || digits.starts_with("0B") {
        (2, &digits[2..])
    } else if digits.starts_with("0o") || digits.starts_with("0O") {
        (8, &digits[2..])
    } else if digits.starts_with('0') && digits.len() > 1 {
        (8, &digits[1..])
    } else {
        (10, digits.as_str())
    };
    u64::from_str_radix(body, radix).ok()
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

/// Return the body span of one outer parenthesized type, or `None` inside the
/// outer option when the type has no outer parentheses.
pub(crate) fn type_parentheses(source: &str) -> Option<Option<std::ops::Range<usize>>> {
    let first = Lexer::new(source).next()?;
    if first.kind != TokenKind::LParen {
        return Some(None);
    }
    let last = Lexer::new(source)
        .filter(|token| token.kind != TokenKind::Semicolon)
        .last()?;
    if last.kind != TokenKind::RParen {
        return None;
    }
    Some(Some(first.span.end..last.span.start))
}

/// Remove at most `MAX_DEPTH` outer pairs; a deeper remainder stays visible to
/// the caller instead of being silently accepted or rejected by this helper.
pub(crate) fn unparenthesized_type(mut source: &str) -> Option<&str> {
    for _ in 0..MAX_DEPTH {
        let Some(span) = type_parentheses(source)? else {
            return Some(source);
        };
        source = source.get(span)?;
    }
    Some(source)
}

impl<'a> TypeArena<'a> {
    fn eval_const_array_length(&mut self, origin: &Path, expr: &str) -> Option<u64> {
        if expr.is_empty() || expr.len() > 1024 {
            return None;
        }
        let int_bits = self.integer_bits()?;
        let op = self.constant_expression(origin, expr)?;
        let kind = self.basic_kind(op.ty)?;
        if !kind.untyped() && !kind.integer() {
            return None;
        }
        super::operand::representable(op.value?, BasicKind::Int, int_bits).then_some(())?;
        let value = match op.value? {
            super::Constant::Int(value)
            | super::Constant::Float {
                integral: Some(value),
                ..
            } => value,
            _ => return None,
        };
        u64::try_from(value).ok()
    }
}
