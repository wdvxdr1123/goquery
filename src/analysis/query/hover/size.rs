//! gopls-compatible size/offset annotations for type and field declarations.
//!
//! Layout follows `go/types` gcSizes for the workspace word size: word-aligned
//! fields, gc's zero-size final field rule and padded struct sizes. Only
//! syntax-derived layouts are computed; unresolved or parameterized types stay
//! unknown instead of guessing.
use super::render::declaration_source;
use crate::analysis::query::Context;
use crate::index::IndexedSymbol as Symbol;
use crate::syntax::{Lexer, Token, TokenKind};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const MAX_NODES: usize = 512;
const MAX_DEPTH: usize = 24;
const MAX_SOURCE_BYTES: usize = 512 * 1024;

#[derive(Clone, Copy)]
pub(super) struct Layout {
    pub(super) size: i64,
    pub(super) align: i64,
}

pub(super) struct FieldLayout {
    pub(super) name: String,
    pub(super) size: i64,
    pub(super) offset: i64,
    pub(super) align: i64,
    pub(super) annotation: String,
    pub(super) embedded: bool,
}

pub(super) struct StructLayout {
    pub(super) layout: Layout,
    pub(super) fields: Vec<FieldLayout>,
}

pub(super) struct Sizes<'context, 'workspace> {
    context: &'context Context<'workspace>,
    named: HashMap<(PathBuf, usize), Option<Layout>>,
    sources: HashMap<PathBuf, String>,
    source_bytes: usize,
    nodes: usize,
}

impl<'context, 'workspace> Sizes<'context, 'workspace> {
    pub(super) fn new(context: &'context Context<'workspace>) -> Self {
        Self {
            context,
            named: HashMap::new(),
            sources: HashMap::new(),
            source_bytes: 0,
            nodes: 0,
        }
    }

    fn word(&self) -> i64 {
        8
    }

    fn layout_at(&mut self, origin: &Path, text: &str, depth: usize) -> Option<Layout> {
        if depth >= MAX_DEPTH || text.len() > 16 * 1024 {
            return None;
        }
        let tokens = tokens(text);
        let mut index = 0usize;
        let layout = self.parse(origin, text, &tokens, &mut index, depth);
        (index == tokens.len()).then_some(layout?)
    }

    fn parse(
        &mut self,
        origin: &Path,
        source: &str,
        tokens: &[Token],
        index: &mut usize,
        depth: usize,
    ) -> Option<Layout> {
        self.nodes += 1;
        if self.nodes > MAX_NODES || depth >= MAX_DEPTH {
            return None;
        }
        let token = tokens.get(*index)?;
        match token.kind {
            TokenKind::Mul => {
                *index += 1;
                skip_type(tokens, index)?;
                Some(self.word_layout())
            }
            TokenKind::LBrack => {
                *index += 1;
                if tokens.get(*index)?.kind == TokenKind::RBrack {
                    *index += 1;
                    skip_type(tokens, index)?;
                    return Some(Layout {
                        size: self.word() * 3,
                        align: self.word(),
                    });
                }
                let start = *index;
                let mut nested = 0usize;
                while let Some(token) = tokens.get(*index) {
                    match token.kind {
                        TokenKind::LBrack => nested += 1,
                        TokenKind::RBrack if nested == 0 => break,
                        TokenKind::RBrack => nested -= 1,
                        _ => {}
                    }
                    *index += 1;
                }
                let length = self.const_int(origin, source, &tokens[start..*index])?;
                *index += 1;
                let element = self.parse(origin, source, tokens, index, depth + 1)?;
                Some(Layout {
                    size: element.size.checked_mul(length)?,
                    align: element.align,
                })
            }
            TokenKind::Map => {
                *index += 1;
                if tokens
                    .get(*index)
                    .is_some_and(|token| token.kind == TokenKind::LBrack)
                {
                    skip_balanced(tokens, index)?;
                }
                skip_type(tokens, index)?;
                Some(self.word_layout())
            }
            TokenKind::Chan => {
                *index += 1;
                if tokens
                    .get(*index)
                    .is_some_and(|token| token.kind == TokenKind::Arrow)
                {
                    *index += 1;
                }
                skip_type(tokens, index)?;
                Some(self.word_layout())
            }
            TokenKind::Arrow => {
                *index += 1;
                skip_type(tokens, index)?;
                Some(self.word_layout())
            }
            TokenKind::Func => {
                *index += 1;
                self.skip_signature(source, tokens, index)?;
                Some(self.word_layout())
            }
            TokenKind::Struct => {
                *index += 1;
                Some(
                    self.struct_tail(origin, source, tokens, index, depth + 1)?
                        .layout,
                )
            }
            TokenKind::Interface => {
                *index += 1;
                skip_balanced(tokens, index)?;
                Some(Layout {
                    size: self.word() * 2,
                    align: self.word(),
                })
            }
            TokenKind::LParen => {
                *index += 1;
                let layout = self.parse(origin, source, tokens, index, depth + 1)?;
                if tokens.get(*index)?.kind != TokenKind::RParen {
                    return None;
                }
                *index += 1;
                Some(layout)
            }
            TokenKind::Ident => {
                let start = *index;
                *index += 1;
                let mut text = &source[token.span.clone()];
                if tokens
                    .get(*index)
                    .is_some_and(|token| token.kind == TokenKind::Period)
                {
                    *index += 1;
                    let part = tokens.get(*index)?;
                    if part.kind != TokenKind::Ident {
                        return None;
                    }
                    let end = part.span.end;
                    text = source.get(token.span.start..end)?;
                    *index += 1;
                }
                let _ = start;
                self.named(origin, text, depth + 1)
            }
            _ => None,
        }
    }

    fn word_layout(&self) -> Layout {
        Layout {
            size: self.word(),
            align: self.word(),
        }
    }

    fn named(&mut self, origin: &Path, text: &str, depth: usize) -> Option<Layout> {
        if let Some(layout) = builtin_layout(text) {
            return Some(layout);
        }
        let workspace = self.context.workspace();
        let Some((file, symbol)) = workspace.type_declaration(origin, text) else {
            // go/types treats a type from an unresolvable imported package as the
            // invalid type, which still occupies a word in layouts.
            if let Some((prefix, _)) = text.split_once('.')
                && workspace
                    .imported_symbol_iter(origin, prefix)
                    .next()
                    .is_none()
            {
                return Some(Layout { size: 8, align: 8 });
            }
            return None;
        };
        if symbol.generic {
            // A generic instantiation cannot be expanded from the declaration
            // alone; its common representations (atomic.Pointer, atomic.Value)
            // are word-sized handles.
            return Some(self.word_layout());
        }
        let key = (file.clone(), symbol.start);
        if let Some(cached) = self.named.get(&key) {
            return *cached;
        }
        self.named.insert(key.clone(), None);
        let layout = self.compute(file, symbol, depth + 1);
        self.named.insert(key, layout);
        layout
    }

    fn compute(&mut self, file: &Path, symbol: &Symbol, depth: usize) -> Option<Layout> {
        if depth >= MAX_DEPTH || self.nodes > MAX_NODES {
            return None;
        }
        let source = self.source_text(file)?;
        let declaration = symbol.declaration()?;
        let rest = source.get(symbol.end..declaration.end)?;
        let tokens = tokens(rest);
        let mut index = 0usize;
        if tokens
            .first()
            .is_some_and(|token| token.kind == TokenKind::Assign)
        {
            index = 1;
        }
        let layout = self.parse(file, rest, &tokens, &mut index, depth + 1)?;
        (index == tokens.len()).then_some(layout)
    }

    fn source_text(&mut self, file: &Path) -> Option<String> {
        if file == self.context.file() {
            return Some(self.context.source().to_owned());
        }
        if let Some(source) = self.sources.get(file) {
            return Some(source.clone());
        }
        let source = self.context.workspace().source(file)?;
        if self.source_bytes.saturating_add(source.len()) > MAX_SOURCE_BYTES {
            return None;
        }
        self.source_bytes += source.len();
        self.sources.insert(file.to_path_buf(), source.clone());
        Some(source)
    }

    fn const_int(&mut self, origin: &Path, source: &str, tokens: &[Token]) -> Option<i64> {
        self.constant_expression(origin, source, tokens, &mut 0, 0)
    }

    fn constant_expression(
        &mut self,
        origin: &Path,
        source: &str,
        tokens: &[Token],
        index: &mut usize,
        depth: usize,
    ) -> Option<i64> {
        if depth >= 16 {
            return None;
        }
        let mut value = self.constant_term(origin, source, tokens, index, depth)?;
        while let Some(token) = tokens.get(*index) {
            let operator = token.kind;
            if !matches!(
                operator,
                TokenKind::Add
                    | TokenKind::Sub
                    | TokenKind::Or
                    | TokenKind::Xor
                    | TokenKind::Shl
                    | TokenKind::Shr
                    | TokenKind::And
                    | TokenKind::AndNot
            ) {
                break;
            }
            *index += 1;
            let right = self.constant_term(origin, source, tokens, index, depth)?;
            value = match operator {
                TokenKind::Add => value.checked_add(right)?,
                TokenKind::Sub => value.checked_sub(right)?,
                TokenKind::Or => value | right,
                TokenKind::Xor => value ^ right,
                TokenKind::Shl => value.checked_shl(u32::try_from(right).ok()?)?,
                TokenKind::Shr => value.checked_shr(u32::try_from(right).ok()?)?,
                TokenKind::And => value & right,
                TokenKind::AndNot => value & !right,
                _ => return None,
            };
        }
        Some(value)
    }

    fn constant_term(
        &mut self,
        origin: &Path,
        source: &str,
        tokens: &[Token],
        index: &mut usize,
        depth: usize,
    ) -> Option<i64> {
        let mut value = self.constant_primary(origin, source, tokens, index, depth)?;
        while let Some(token) = tokens.get(*index) {
            let operator = token.kind;
            if !matches!(operator, TokenKind::Mul | TokenKind::Quo | TokenKind::Rem) {
                break;
            }
            *index += 1;
            let right = self.constant_primary(origin, source, tokens, index, depth)?;
            value = match operator {
                TokenKind::Mul => value.checked_mul(right)?,
                TokenKind::Quo => {
                    if right == 0 {
                        return None;
                    }
                    value / right
                }
                TokenKind::Rem => {
                    if right == 0 {
                        return None;
                    }
                    value % right
                }
                _ => return None,
            };
        }
        Some(value)
    }

    fn constant_primary(
        &mut self,
        origin: &Path,
        source: &str,
        tokens: &[Token],
        index: &mut usize,
        depth: usize,
    ) -> Option<i64> {
        let token = tokens.get(*index)?;
        match token.kind {
            TokenKind::Int => {
                *index += 1;
                parse_int(source.get(token.span.clone())?)
            }
            TokenKind::LParen => {
                *index += 1;
                let value = self.constant_expression(origin, source, tokens, index, depth + 1)?;
                if tokens.get(*index)?.kind != TokenKind::RParen {
                    return None;
                }
                *index += 1;
                Some(value)
            }
            TokenKind::Ident => {
                let start = *index;
                *index += 1;
                let mut text = source.get(token.span.clone())?.to_owned();
                if tokens
                    .get(*index)
                    .is_some_and(|next| next.kind == TokenKind::Period)
                {
                    *index += 1;
                    let part = tokens.get(*index)?;
                    if part.kind != TokenKind::Ident {
                        return None;
                    }
                    text.push('.');
                    text.push_str(source.get(part.span.clone())?);
                    *index += 1;
                }
                if *index == start {
                    return None;
                }
                if tokens
                    .get(*index)
                    .is_some_and(|next| next.kind == TokenKind::LParen)
                    && self.is_type_conversion(origin, &text)
                {
                    *index += 1;
                    let value =
                        self.constant_expression(origin, source, tokens, index, depth + 1)?;
                    if tokens.get(*index)?.kind != TokenKind::RParen {
                        return None;
                    }
                    *index += 1;
                    return Some(value);
                }
                self.constant_value(origin, &text, depth + 1)
            }
            _ => None,
        }
    }

    /// Whether the identifier names a predeclared or package-declared type.
    fn is_type_conversion(&self, origin: &Path, name: &str) -> bool {
        if matches!(
            name,
            "int"
                | "int8"
                | "int16"
                | "int32"
                | "int64"
                | "uint"
                | "uint8"
                | "uint16"
                | "uint32"
                | "uint64"
                | "uintptr"
                | "float32"
                | "float64"
                | "complex64"
                | "complex128"
                | "rune"
                | "byte"
        ) {
            return true;
        }
        let workspace = self.context.workspace();
        if let Some((_, symbol)) = workspace.unqualified_symbol(origin, name) {
            return symbol.is_type();
        }
        if let Some((alias, part)) = name.split_once('.') {
            return workspace
                .imported_symbol_iter(origin, alias)
                .any(|(_, symbol)| symbol.is_type() && symbol.name == part);
        }
        false
    }

    /// Value of a named (possibly imported) integer constant.
    fn constant_value(&mut self, origin: &Path, name: &str, depth: usize) -> Option<i64> {
        if depth >= 16 {
            return None;
        }
        let workspace = self.context.workspace();
        let (file, symbol) = if let Some((alias, constant)) = name.split_once('.') {
            let (path, symbol) = workspace
                .imported_symbol_iter(origin, alias)
                .find(|(_, symbol)| symbol.is_const() && symbol.name == constant)?;
            (path.clone(), symbol)
        } else {
            let (path, symbol) = workspace
                .unqualified_symbol(origin, name)
                .filter(|(_, symbol)| symbol.is_const())?;
            (path.clone(), symbol)
        };
        let source = self.source_text(&file)?;
        let declaration = symbol.declaration()?;
        let spec = source.get(declaration)?;
        let value =
            if let Some(equals) = Lexer::new(spec).find(|token| token.kind == TokenKind::Assign) {
                spec.get(equals.span.end..)?.trim().to_owned()
            } else {
                // Empty expression lists inherit the previous spec's expression.
                let group = super::symbol::const_group_context(&source, symbol.start)?;
                let expression = group.last_expression?;
                super::value::substitute_iota(&expression, group.iota_index)
            };
        let value = value.as_str();
        let tokens = tokens(value);
        let mut index = 0usize;
        let result = self.constant_expression(&file, value, &tokens, &mut index, depth + 1)?;
        (index == tokens.len()).then_some(result)
    }

    fn skip_signature(&mut self, source: &str, tokens: &[Token], index: &mut usize) -> Option<()> {
        let _ = (source, &self.nodes);
        if tokens
            .get(*index)
            .is_some_and(|token| token.kind == TokenKind::LParen)
        {
            skip_balanced(tokens, index)?;
        }
        match tokens.get(*index)?.kind {
            TokenKind::LParen => skip_balanced(tokens, index)?,
            TokenKind::LBrack
            | TokenKind::Map
            | TokenKind::Chan
            | TokenKind::Func
            | TokenKind::Struct
            | TokenKind::Interface
            | TokenKind::Mul
            | TokenKind::Arrow
            | TokenKind::Ident => skip_type(tokens, index)?,
            _ => {}
        }
        Some(())
    }

    /// Parse the brace body after a leading `struct` keyword.
    fn struct_tail(
        &mut self,
        origin: &Path,
        source: &str,
        tokens: &[Token],
        index: &mut usize,
        depth: usize,
    ) -> Option<StructLayout> {
        if tokens.get(*index)?.kind != TokenKind::LBrace {
            return None;
        }
        *index += 1;
        let mut fields: Vec<FieldLayout> = Vec::new();
        let mut offset = 0i64;
        let mut align = 1i64;
        loop {
            let token = tokens.get(*index)?;
            match token.kind {
                TokenKind::RBrace => {
                    *index += 1;
                    break;
                }
                TokenKind::Semicolon | TokenKind::String => {
                    *index += 1;
                    continue;
                }
                _ => {}
            }
            let field_start = *index;
            let mut names: Vec<String> = Vec::new();
            if tokens[*index].kind == TokenKind::Ident {
                let mut look = *index;
                loop {
                    if tokens.get(look)?.kind != TokenKind::Ident {
                        break;
                    }
                    if tokens
                        .get(look + 1)
                        .is_some_and(|token| token.kind == TokenKind::Period)
                    {
                        break;
                    }
                    let name = source.get(tokens[look].span.clone())?.to_owned();
                    look += 1;
                    if tokens
                        .get(look)
                        .is_some_and(|token| token.kind == TokenKind::Comma)
                    {
                        names.push(name);
                        look += 1;
                        continue;
                    }
                    names.push(name);
                    break;
                }
                if tokens.get(look).is_some_and(|token| {
                    token.kind == TokenKind::Ident
                        && tokens.get(look - 1).is_some_and(|name| {
                            new_line_between(source, name.span.end, token.span.start)
                        })
                        && names.len() == 1
                }) {
                    // Embedded field: its written type is the name itself.
                    names.clear();
                } else if tokens.get(look).is_some_and(|token| type_start(token.kind)) {
                    *index = look;
                } else {
                    names.clear();
                }
            }
            let type_start = *index;
            if names.is_empty() {
                // Embedded field: the field name is the written type name.
                let layout = self.parse(origin, source, tokens, index, depth + 1)?;
                let written = source
                    .get(tokens.get(type_start)?.span.start..tokens.get(*index - 1)?.span.end)?;
                let name = embedded_name(written)?;
                offset = align_to(offset, layout.align);
                fields.push(FieldLayout {
                    name,
                    size: layout.size,
                    offset,
                    align: layout.align,
                    annotation: written.trim().to_owned(),
                    embedded: true,
                });
                offset = offset.checked_add(layout.size)?;
                align = align.max(layout.align);
            } else {
                let layout = self.parse(origin, source, tokens, index, depth + 1)?;
                let annotation = source
                    .get(tokens.get(type_start)?.span.start..tokens.get(*index - 1)?.span.end)?;
                for name in names {
                    offset = align_to(offset, layout.align);
                    fields.push(FieldLayout {
                        name,
                        size: layout.size,
                        offset,
                        align: layout.align,
                        annotation: annotation.trim().to_owned(),
                        embedded: false,
                    });
                    offset = offset.checked_add(layout.size)?;
                    align = align.max(layout.align);
                }
            }
            if tokens
                .get(*index)
                .is_some_and(|token| token.kind == TokenKind::String)
            {
                *index += 1;
            }
            if *index == field_start {
                return None;
            }
        }
        let mut size = offset;
        if let Some(last) = fields.last()
            && last.offset > 0
            && last.size == 0
        {
            size = last.offset + 1;
        }
        Some(StructLayout {
            layout: Layout {
                size: align_to(size, align),
                align,
            },
            fields,
        })
    }
}

pub(super) struct FieldDecl {
    pub(super) name: String,
    pub(super) annotation: String,
    pub(super) embedded: bool,
}

/// Evaluate a constant expression, resolving named/imported constants.
pub(super) fn const_integer(context: &Context<'_>, origin: &Path, expression: &str) -> Option<i64> {
    let mut sizes = Sizes::new(context);
    let tokens = tokens(expression);
    let mut index = 0usize;
    let value = sizes.constant_expression(origin, expression, &tokens, &mut index, 0)?;
    (index == tokens.len()).then_some(value)
}

/// Layout of an anonymous `struct { ... }` type expression.
pub(super) fn struct_text_layout(
    context: &Context<'_>,
    origin: &Path,
    text: &str,
) -> Option<StructLayout> {
    let mut sizes = Sizes::new(context);
    let tokens = tokens(text);
    let mut index = 0usize;
    if tokens
        .first()
        .is_some_and(|token| token.kind == TokenKind::Struct)
    {
        index = 1;
    }
    if tokens.get(index)?.kind != TokenKind::LBrace {
        return None;
    }
    let layout = sizes.struct_tail(origin, text, &tokens, &mut index, 0)?;
    (index == tokens.len()).then_some(layout)
}

/// Struct field declarations without resolving any field types.
pub(super) fn struct_fields(
    context: &Context<'_>,
    file: &Path,
    symbol: &Symbol,
) -> Option<Vec<FieldDecl>> {
    let source_text = declaration_source(context, file)?;
    let source = source_text.text();
    let declaration = symbol.declaration()?;
    let rest = source.get(symbol.end..declaration.end)?;
    let text = rest.trim_start().trim_start_matches('=').trim_start();
    let tokens = tokens(text);
    let mut index = 0usize;
    if tokens
        .first()
        .is_some_and(|token| token.kind == TokenKind::LBrack)
    {
        skip_balanced(&tokens, &mut index)?;
    }
    if tokens
        .get(index)
        .is_some_and(|token| token.kind != TokenKind::Struct)
    {
        return None;
    }
    index += 1;
    if tokens
        .get(index)
        .is_some_and(|token| token.kind != TokenKind::LBrace)
    {
        return None;
    }
    index += 1;
    let mut fields = Vec::new();
    loop {
        match tokens.get(index)?.kind {
            TokenKind::RBrace => break,
            TokenKind::Semicolon | TokenKind::String => {
                index += 1;
                continue;
            }
            _ => {}
        }
        let field_start = index;
        let mut names: Vec<String> = Vec::new();
        if tokens[index].kind == TokenKind::Ident {
            let mut look = index;
            loop {
                if tokens.get(look)?.kind != TokenKind::Ident {
                    break;
                }
                if tokens
                    .get(look + 1)
                    .is_some_and(|token| token.kind == TokenKind::Period)
                {
                    break;
                }
                let name = text.get(tokens[look].span.clone())?.to_owned();
                look += 1;
                if tokens
                    .get(look)
                    .is_some_and(|token| token.kind == TokenKind::Comma)
                {
                    names.push(name);
                    look += 1;
                    continue;
                }
                names.push(name);
                break;
            }
            if tokens.get(look).is_some_and(|token| {
                token.kind == TokenKind::Ident
                    && tokens
                        .get(look - 1)
                        .is_some_and(|name| new_line_between(text, name.span.end, token.span.start))
                    && names.len() == 1
            }) {
                // Embedded field: its written type is the name itself.
                names.clear();
            } else if tokens.get(look).is_some_and(|token| type_start(token.kind)) {
                index = look;
            } else {
                names.clear();
            }
        }
        if names.is_empty() {
            skip_type(&tokens, &mut index)?;
            let annotation =
                text.get(tokens[field_start].span.start..tokens[index - 1].span.end)?;
            let name = embedded_name(annotation)?;
            fields.push(FieldDecl {
                name,
                annotation: annotation.trim().to_owned(),
                embedded: true,
            });
        } else {
            let type_start = index;
            skip_type(&tokens, &mut index)?;
            let annotation = text.get(tokens[type_start].span.start..tokens[index - 1].span.end)?;
            for name in names {
                fields.push(FieldDecl {
                    name,
                    annotation: annotation.trim().to_owned(),
                    embedded: false,
                });
            }
        }
        if tokens
            .get(index)
            .is_some_and(|token| token.kind == TokenKind::String)
        {
            index += 1;
        }
        if index == field_start {
            return None;
        }
    }
    Some(fields)
}

fn tokens(source: &str) -> Vec<Token> {
    Lexer::new(source).filter(|token| !token.inserted).collect()
}

fn type_start(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Mul
            | TokenKind::LBrack
            | TokenKind::Map
            | TokenKind::Chan
            | TokenKind::Func
            | TokenKind::Struct
            | TokenKind::Interface
            | TokenKind::Arrow
            | TokenKind::LParen
            | TokenKind::Ident
    )
}

fn skip_balanced(tokens: &[Token], index: &mut usize) -> Option<()> {
    let opening = tokens.get(*index)?.kind;
    let closing = match opening {
        TokenKind::LParen => TokenKind::RParen,
        TokenKind::LBrack => TokenKind::RBrack,
        _ => TokenKind::RBrace,
    };
    let mut depth = 0usize;
    while let Some(token) = tokens.get(*index) {
        if token.kind == opening {
            depth += 1;
        } else if token.kind == closing {
            depth = depth.checked_sub(1)?;
            if depth == 0 {
                *index += 1;
                return Some(());
            }
        }
        *index += 1;
    }
    None
}

/// Advance over one type expression without resolving any names.
/// Whether a newline separates two byte offsets.
fn new_line_between(text: &str, end: usize, start: usize) -> bool {
    text.get(end..start).is_some_and(|gap| gap.contains('\n'))
}

fn skip_type(tokens: &[Token], index: &mut usize) -> Option<()> {
    let start = *index;
    let token = tokens.get(*index)?;
    match token.kind {
        TokenKind::Mul => {
            *index += 1;
            skip_type(tokens, index)?;
        }
        TokenKind::LBrack => {
            skip_balanced(tokens, index)?;
            if tokens
                .get(*index)
                .is_some_and(|token| token.kind != TokenKind::Semicolon)
                && matches!(
                    tokens.get(*index)?.kind,
                    TokenKind::Mul
                        | TokenKind::LBrack
                        | TokenKind::Map
                        | TokenKind::Chan
                        | TokenKind::Func
                        | TokenKind::Struct
                        | TokenKind::Interface
                        | TokenKind::Arrow
                        | TokenKind::Ident
                        | TokenKind::LParen
                )
            {
                skip_type(tokens, index)?;
            }
        }
        TokenKind::Map => {
            *index += 1;
            if tokens
                .get(*index)
                .is_some_and(|token| token.kind == TokenKind::LBrack)
            {
                skip_balanced(tokens, index)?;
            }
            skip_type(tokens, index)?;
        }
        TokenKind::Chan => {
            *index += 1;
            if tokens
                .get(*index)
                .is_some_and(|token| token.kind == TokenKind::Arrow)
            {
                *index += 1;
            }
            skip_type(tokens, index)?;
        }
        TokenKind::Arrow => {
            *index += 1;
            skip_type(tokens, index)?;
        }
        TokenKind::Func => {
            *index += 1;
            if tokens
                .get(*index)
                .is_some_and(|token| token.kind == TokenKind::LParen)
            {
                skip_balanced(tokens, index)?;
            }
            match tokens.get(*index)?.kind {
                TokenKind::LParen => skip_balanced(tokens, index)?,
                TokenKind::Ident
                | TokenKind::Mul
                | TokenKind::LBrack
                | TokenKind::Map
                | TokenKind::Chan
                | TokenKind::Func
                | TokenKind::Struct
                | TokenKind::Interface
                | TokenKind::Arrow => skip_type(tokens, index)?,
                _ => {}
            }
        }
        TokenKind::Struct | TokenKind::Interface => {
            *index += 1;
            skip_balanced(tokens, index)?;
        }
        TokenKind::LParen => {
            *index += 1;
            skip_type(tokens, index)?;
            if tokens.get(*index)?.kind != TokenKind::RParen {
                return None;
            }
            *index += 1;
        }
        TokenKind::Ident => {
            *index += 1;
            if tokens
                .get(*index)
                .is_some_and(|token| token.kind == TokenKind::Period)
            {
                *index += 2;
            }
            if tokens
                .get(*index)
                .is_some_and(|token| token.kind == TokenKind::LBrack)
            {
                skip_balanced(tokens, index)?;
            }
        }
        _ => return None,
    }
    (*index > start).then_some(())
}

fn embedded_name(written: &str) -> Option<String> {
    let text = written.trim().trim_start_matches('*');
    let text = text.split('[').next()?;
    named_part(text).map(str::to_owned)
}

fn named_part(text: &str) -> Option<&str> {
    text.rsplit('.').next()
}

/// Size annotation of a type declaration's underlying type.
pub(super) fn type_layout(context: &Context<'_>, origin: &Path, symbol: &Symbol) -> Option<Layout> {
    if symbol.generic {
        return None;
    }
    let source_text = declaration_source(context, origin)?;
    let source = source_text.text();
    let declaration = symbol.declaration()?;
    let rest = source.get(symbol.end..declaration.end)?;
    let mut sizes = Sizes::new(context);
    sizes.layout_at(
        origin,
        rest.trim_start().trim_start_matches('=').trim_start(),
        0,
    )
}

/// Struct layout of a type declaration, for waste and field offsets.
pub(super) fn struct_layout(
    context: &Context<'_>,
    origin: &Path,
    symbol: &Symbol,
) -> Option<StructLayout> {
    let source_text = declaration_source(context, origin)?;
    let source = source_text.text();
    let declaration = symbol.declaration()?;
    let rest = source.get(symbol.end..declaration.end)?;
    let text = rest.trim_start().trim_start_matches('=').trim_start();
    let tokens = tokens(text);
    let mut index = 0usize;
    if tokens
        .first()
        .is_some_and(|token| token.kind == TokenKind::LBrack)
    {
        skip_balanced(&tokens, &mut index)?;
    }
    if tokens
        .get(index)
        .is_some_and(|token| token.kind != TokenKind::Struct)
    {
        return None;
    }
    index += 1;
    let mut sizes = Sizes::new(context);
    sizes.struct_tail(origin, text, &tokens, &mut index, 0)
}

/// Size and offset of one declared struct field of `owner`.
pub(super) fn field_layout(
    context: &Context<'_>,
    origin: &Path,
    owner: &Symbol,
    name: &str,
) -> Option<FieldLayout> {
    if owner.generic {
        return None;
    }
    let layout = struct_layout(context, origin, owner)?;
    let mut position = None;
    for (index, field) in layout.fields.iter().enumerate() {
        if field.name == name {
            position = Some(index);
            break;
        }
    }
    let index = position?;
    let field = layout.fields.get(index)?;
    Some(FieldLayout {
        name: field.name.clone(),
        size: field.size,
        offset: field.offset,
        align: field.align,
        annotation: field.annotation.clone(),
        embedded: field.embedded,
    })
}

/// gopls' wasted-space percentage of a struct type.
pub(super) fn wasted(layout: StructLayout) -> Option<i64> {
    if layout.layout.size <= 0 || layout.fields.is_empty() {
        return None;
    }
    let mut fields: Vec<&FieldLayout> = layout.fields.iter().collect();
    fields.sort_by_key(|field| std::cmp::Reverse(field.size));
    let mut offset = 0i64;
    let mut last = (0i64, 0i64);
    for field in &fields {
        offset = align_to(offset, field.align.max(1));
        last = (offset, field.size);
        offset = offset.checked_add(field.size)?;
    }
    let compact = last.0 + last.1;
    let wasted = 100 * (layout.layout.size - compact) / layout.layout.size;
    (wasted >= 0).then_some(wasted)
}

/// `size=..[ (N% wasted)][, class=..]` annotation.
pub(super) fn size_summary(layout: Layout, wasted: Option<i64>) -> String {
    let mut text = format!("size={}", number(layout.size));
    if let Some(wasted) = wasted.filter(|wasted| *wasted >= 20) {
        text.push_str(&format!(" ({wasted}% wasted)"));
    }
    if let Some(class) = size_class(layout.size).filter(|class| *class > layout.size) {
        text.push_str(&format!(", class={}", number(class)));
    }
    text
}

/// gopls `sizeClass`: the runtime allocator size class of an allocation.
pub(super) fn size_class(size: i64) -> Option<i64> {
    const CLASSES: &[i64] = &[
        0, 8, 16, 24, 32, 48, 64, 80, 96, 112, 128, 144, 160, 176, 192, 208, 224, 240, 256, 288,
        320, 352, 384, 416, 448, 480, 512, 576, 640, 704, 768, 896, 1024, 1152, 1280, 1408, 1536,
        1792, 2048, 2304, 2688, 3072, 3200, 3456, 4096, 4864, 5376, 6144, 6528, 6784, 6912, 8192,
        9472, 9728, 10240, 10880, 12288, 13568, 14336, 16384, 18432, 19072, 20480, 21760, 24576,
        27264, 28672, 32768,
    ];
    const PAGE: i64 = 8192;
    if size > 1 << 16 {
        return None;
    }
    if size <= 0 {
        return Some(0);
    }
    if size > 32768 {
        return Some((size + PAGE - 1) / PAGE * PAGE);
    }
    CLASSES.iter().copied().find(|class| *class >= size)
}

/// Decimal rendering used by `size=`/`offset=` annotations.
pub(super) fn number(value: i64) -> String {
    if value < 10 {
        value.to_string()
    } else {
        format!("{value} (0x{value:x})")
    }
}

fn builtin_layout(name: &str) -> Option<Layout> {
    Some(match name {
        "bool" | "int8" | "uint8" | "byte" => Layout { size: 1, align: 1 },
        "int16" | "uint16" => Layout { size: 2, align: 2 },
        "int32" | "uint32" | "rune" => Layout { size: 4, align: 4 },
        "int64" | "uint64" | "int" | "uint" | "uintptr" => Layout { size: 8, align: 8 },
        "float32" => Layout { size: 4, align: 4 },
        "float64" => Layout { size: 8, align: 8 },
        "complex64" => Layout { size: 8, align: 4 },
        "complex128" => Layout { size: 16, align: 8 },
        "string" => Layout { size: 16, align: 8 },
        "any" | "error" | "comparable" => Layout { size: 16, align: 8 },
        _ => return None,
    })
}

fn align_to(value: i64, align: i64) -> i64 {
    if align <= 1 {
        value
    } else {
        (value + align - 1) / align * align
    }
}

fn parse_int(text: &str) -> Option<i64> {
    let text = text.trim().replace('_', "");
    let (digits, radix) =
        if let Some(rest) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
            (rest, 16)
        } else if let Some(rest) = text.strip_prefix("0b").or_else(|| text.strip_prefix("0B")) {
            (rest, 2)
        } else if let Some(rest) = text.strip_prefix("0o").or_else(|| text.strip_prefix("0O")) {
            (rest, 8)
        } else if text.len() > 1 && text.starts_with('0') {
            (&text[1..], 8)
        } else {
            (text.as_str(), 10)
        };
    i64::from_str_radix(digits, radix).ok()
}
