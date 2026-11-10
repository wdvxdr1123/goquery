//! File-level document symbols with gopls internal/golang/symbols.go semantics.
//!
//! Declarations come from the compact stub; type expressions, signatures and
//! aggregate members are read from the current source on demand and formatted
//! with the go/types `ExprString` normalization. No syntax tree is retained.
use super::super::{Context, SourceRange};
use crate::syntax::{Lexer, SymbolKind, TokenKind};
use crate::text::SourceMap;
use std::ops::Range;

/// Bounds for one document-symbol request.
const MAX_SYMBOLS: usize = 4096;
const MAX_DEPTH: usize = 32;
const MAX_TOKENS: usize = 8192;
const MAX_DETAIL_BYTES: usize = 256 * 1024;
const MAX_TEXT_SPAN: usize = 16 * 1024;

/// Semantic symbol categories; protocol numbers live in the feature layer.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum DocumentSymbolKind {
    Function,
    Method,
    Struct,
    Interface,
    Class,
    Field,
    Variable,
    Constant,
}

#[derive(Clone)]
pub(crate) struct DocumentSymbol {
    pub(crate) name: String,
    pub(crate) kind: DocumentSymbolKind,
    pub(crate) detail: String,
    pub(crate) range: SourceRange,
    pub(crate) selection: SourceRange,
    pub(crate) children: Vec<DocumentSymbol>,
}

struct Details {
    kind: DocumentSymbolKind,
    detail: String,
    children: Vec<DocumentSymbol>,
}

struct Budget {
    tokens: usize,
    bytes: usize,
}

impl Budget {
    fn take(&mut self, tokens: usize) -> Option<()> {
        self.tokens = self.tokens.checked_sub(tokens)?;
        Some(())
    }

    fn write(&mut self, out: &mut String, text: &str) -> Option<()> {
        self.bytes = self.bytes.checked_sub(text.len())?;
        out.push_str(text);
        Some(())
    }
}

pub(crate) fn document_symbols(
    context: &Context<'_>,
) -> Result<Vec<DocumentSymbol>, (i32, String)> {
    let source = context.source();
    // gopls derives document symbols from syntax alone; files excluded from the
    // build still answer from their current buffer. Parse one bounded
    // query-local stub instead of requiring admitted package metadata.
    let stub = crate::syntax::stub(source);
    let map = SourceMap::new(source);
    let mut budget = Budget {
        tokens: MAX_TOKENS,
        bytes: MAX_DETAIL_BYTES,
    };
    let mut symbols = Vec::new();
    for symbol in &stub.symbols {
        if symbol.name == "_" || symbols.len() >= MAX_SYMBOLS {
            continue;
        }
        let row = match symbol.kind {
            SymbolKind::Function => function_symbol(source, &map, symbol, false, &mut budget),
            SymbolKind::Method => function_symbol(source, &map, symbol, true, &mut budget),
            SymbolKind::Type => type_symbol(source, &map, symbol, &mut budget),
            SymbolKind::Variable | SymbolKind::Constant => value_symbol(
                source,
                &map,
                symbol,
                symbol.kind == SymbolKind::Constant,
                &mut budget,
            ),
        };
        if let Some(row) = row {
            symbols.push(row);
        }
    }
    Ok(symbols)
}

fn function_symbol(
    source: &str,
    map: &SourceMap<'_>,
    symbol: &crate::syntax::Symbol,
    method: bool,
    budget: &mut Budget,
) -> Option<DocumentSymbol> {
    let declaration = symbol.range.clone();
    let header_end = symbol
        .body
        .as_ref()
        .map_or(declaration.end, |body| body.start);
    let header = source
        .get(declaration.start..header_end)
        .filter(|header| header.len() <= MAX_TEXT_SPAN)?;
    let signature = crate::syntax::parse_signature(header);
    if !signature.diagnostics.is_empty() {
        return None;
    }
    let mut name = symbol.name.clone();
    if method {
        // gopls prepends the receiver type expression: "(RecvExprString).Name".
        let receiver = signature.receiver.first()?;
        let span = declaration.start + receiver.annotation.start
            ..declaration.start + receiver.annotation.end;
        let receiver = type_text(source, &span, budget, 0)?;
        name = format!("({receiver}).{name}");
        if name.len() > MAX_TEXT_SPAN {
            return None;
        }
    }
    let mut detail = String::new();
    budget.write(&mut detail, "func")?;
    let text = signature_text(header, &signature, budget, 0)?;
    budget.write(&mut detail, &text)?;
    Some(DocumentSymbol {
        name,
        kind: if method {
            DocumentSymbolKind::Method
        } else {
            DocumentSymbolKind::Function
        },
        detail,
        range: source_range(map, &declaration)?,
        selection: source_range(map, &symbol.selection)?,
        children: Vec::new(),
    })
}

fn type_symbol(
    source: &str,
    map: &SourceMap<'_>,
    symbol: &crate::syntax::Symbol,
    budget: &mut Budget,
) -> Option<DocumentSymbol> {
    let declaration = symbol.range.clone();
    let spec = source
        .get(declaration.clone())
        .filter(|spec| spec.len() <= MAX_TEXT_SPAN)?;
    let name_end = symbol.selection.end.checked_sub(declaration.start)?;
    let type_start = declaration.start + type_expression_start(spec, name_end)?;
    let prefix = crate::syntax::parse_type_prefix(source.get(type_start..)?);
    if !prefix.diagnostics.is_empty() {
        return None;
    }
    let span = type_start..type_start + prefix.span.end;
    let details = type_details(source, map, &span, budget, 0)?;
    Some(DocumentSymbol {
        name: symbol.name.clone(),
        kind: details.kind,
        detail: details.detail,
        range: source_range(map, &declaration)?,
        selection: source_range(map, &symbol.selection)?,
        children: details.children,
    })
}

fn value_symbol(
    source: &str,
    map: &SourceMap<'_>,
    symbol: &crate::syntax::Symbol,
    constant: bool,
    budget: &mut Budget,
) -> Option<DocumentSymbol> {
    let declaration = symbol.range.clone();
    // Only the written type before the initializer matters; cap the scan so a
    // huge literal value never forces the whole spec into a bounded slice.
    let scan_end = declaration.end.min(declaration.start + MAX_TEXT_SPAN);
    let spec = source.get(declaration.start..scan_end)?;
    let (detail, children) = match value_type_span(spec) {
        Some(span) => {
            let span = declaration.start + span.start..declaration.start + span.end;
            let details = type_details(source, map, &span, budget, 0)?;
            (details.detail, details.children)
        }
        None => (String::new(), Vec::new()),
    };
    Some(DocumentSymbol {
        name: symbol.name.clone(),
        kind: if constant {
            DocumentSymbolKind::Constant
        } else {
            DocumentSymbolKind::Variable
        },
        detail,
        range: source_range(map, &declaration)?,
        selection: source_range(map, &symbol.selection)?,
        children,
    })
}

/// Start of the written type expression inside a type spec: skip the name,
/// optional type parameters and an alias assignment marker.
fn type_expression_start(spec: &str, name_end: usize) -> Option<usize> {
    let rest = spec.get(name_end..)?;
    let mut tokens = Lexer::new(rest).peekable();
    if tokens
        .peek()
        .is_some_and(|token| token.kind == TokenKind::LBrack)
        && let Some(end) = crate::syntax::type_parameter_end(rest)
    {
        while tokens.peek().is_some_and(|token| token.span.start < end) {
            tokens.next();
        }
    }
    if tokens
        .peek()
        .is_some_and(|token| token.kind == TokenKind::Assign)
    {
        tokens.next();
    }
    Some(name_end + tokens.peek()?.span.start)
}

/// Written type span of a value spec, relative to the spec, when one exists.
/// `var x` and `var x = value` have no written type.
fn value_type_span(spec: &str) -> Option<Range<usize>> {
    let mut tokens = Lexer::new(spec).peekable();
    tokens
        .next()
        .filter(|token| token.kind == TokenKind::Ident)?;
    while tokens
        .peek()
        .is_some_and(|token| token.kind == TokenKind::Comma)
    {
        tokens.next();
        tokens
            .next()
            .filter(|token| token.kind == TokenKind::Ident)?;
    }
    let start = match tokens.peek() {
        Some(token) if token.kind != TokenKind::Assign => token.span.start,
        _ => return None,
    };
    let prefix = crate::syntax::parse_type_prefix(&spec[start..]);
    if !prefix.diagnostics.is_empty() {
        return None;
    }
    Some(start..start + prefix.span.end)
}

fn type_details(
    source: &str,
    map: &SourceMap<'_>,
    span: &Range<usize>,
    budget: &mut Budget,
    depth: usize,
) -> Option<Details> {
    if depth >= MAX_DEPTH {
        return None;
    }
    let text = source.get(span.clone())?;
    let mut tokens = Lexer::new(text).peekable();
    let aggregate = tokens.peek()?.kind;
    if !matches!(aggregate, TokenKind::Struct | TokenKind::Interface) {
        return Some(Details {
            kind: DocumentSymbolKind::Class,
            detail: type_text(source, span, budget, depth + 1)?,
            children: Vec::new(),
        });
    }
    let interface = aggregate == TokenKind::Interface;
    let prefix = crate::syntax::parse_type_prefix(text);
    if !prefix.diagnostics.is_empty() {
        return None;
    }
    let mut tokens = Lexer::new(text).peekable();
    tokens.next();
    let container = tokens.next()?.span.start;
    let children = if interface {
        interface_children(source, map, span, container, &prefix, budget, depth)?
    } else {
        struct_children(source, map, span, container, &prefix, budget, depth)?
    };
    let detail = if children.is_empty() {
        if interface { "interface{}" } else { "struct{}" }.to_owned()
    } else if interface {
        "interface{...}".to_owned()
    } else {
        "struct{...}".to_owned()
    };
    Some(Details {
        kind: if interface {
            DocumentSymbolKind::Interface
        } else {
            DocumentSymbolKind::Struct
        },
        detail,
        children,
    })
}

fn struct_children(
    source: &str,
    map: &SourceMap<'_>,
    span: &Range<usize>,
    container: usize,
    prefix: &crate::syntax::TypeSyntax,
    budget: &mut Budget,
    depth: usize,
) -> Option<Vec<DocumentSymbol>> {
    let fields: Vec<_> = prefix
        .fields
        .iter()
        .filter(|field| field.container == container)
        .collect();
    let mut children = Vec::new();
    let mut index = 0;
    while index < fields.len() {
        let first = fields[index];
        let mut end = index + 1;
        while end < fields.len()
            && fields[end].embedded == first.embedded
            && fields[end].annotation == first.annotation
        {
            end += 1;
        }
        let annotation = span.start + first.annotation.start..span.start + first.annotation.end;
        let details = type_details(source, map, &annotation, budget, depth + 1)?;
        if first.embedded {
            let written = span.start + first.name.start..span.start + first.name.end;
            match embedded_ident(source, &annotation) {
                Some((name, selection)) => children.push(DocumentSymbol {
                    name,
                    kind: DocumentSymbolKind::Field,
                    detail: details.detail,
                    range: source_range(map, &annotation)?,
                    selection: source_range(map, &selection)?,
                    children: details.children,
                }),
                None => children.push(DocumentSymbol {
                    name: details.detail,
                    kind: DocumentSymbolKind::Field,
                    detail: String::new(),
                    range: source_range(map, &annotation)?,
                    selection: source_range(map, &written)?,
                    children: details.children,
                }),
            }
        } else {
            let type_end = first
                .tag
                .as_ref()
                .map_or(annotation.end, |tag| span.start + tag.end);
            let range = source_range(map, &(span.start + first.name.start..type_end))?;
            for field in &fields[index..end] {
                let name = span.start + field.name.start..span.start + field.name.end;
                children.push(DocumentSymbol {
                    name: source.get(name.clone())?.to_owned(),
                    kind: DocumentSymbolKind::Field,
                    detail: details.detail.clone(),
                    range,
                    selection: source_range(map, &name)?,
                    children: details.children.clone(),
                });
            }
        }
        index = end;
    }
    Some(children)
}

fn interface_children(
    source: &str,
    map: &SourceMap<'_>,
    span: &Range<usize>,
    container: usize,
    prefix: &crate::syntax::TypeSyntax,
    budget: &mut Budget,
    depth: usize,
) -> Option<Vec<DocumentSymbol>> {
    let mut children = Vec::new();
    for member in prefix
        .interface_members
        .iter()
        .filter(|member| member.container == container)
    {
        let annotation = span.start + member.annotation.start..span.start + member.annotation.end;
        if let Some(name) = member.name.as_ref() {
            let name = span.start + name.start..span.start + name.end;
            let mut detail = String::new();
            budget.write(&mut detail, "func")?;
            let signature = interface_signature_text(source, &annotation, budget, depth + 1)?;
            budget.write(&mut detail, &signature)?;
            children.push(DocumentSymbol {
                name: source.get(name.clone())?.to_owned(),
                kind: DocumentSymbolKind::Method,
                detail,
                range: source_range(map, &(name.start..annotation.end))?,
                selection: source_range(map, &name)?,
                children: Vec::new(),
            });
        } else {
            let details = type_details(source, map, &annotation, budget, depth + 1)?;
            match embedded_ident(source, &annotation) {
                Some((name, selection)) => children.push(DocumentSymbol {
                    name,
                    kind: DocumentSymbolKind::Field,
                    detail: details.detail,
                    range: source_range(map, &annotation)?,
                    selection: source_range(map, &selection)?,
                    children: details.children,
                }),
                None => children.push(DocumentSymbol {
                    name: details.detail,
                    kind: DocumentSymbolKind::Field,
                    detail: String::new(),
                    range: source_range(map, &annotation)?,
                    selection: source_range(map, &annotation)?,
                    children: details.children,
                }),
            }
        }
    }
    Some(children)
}

fn source_range(map: &SourceMap<'_>, span: &Range<usize>) -> Option<SourceRange> {
    (span.start <= span.end).then(|| SourceRange::new(map, span.clone()))
}

/// gopls embeddedIdent: strip a pointer, then generic instantiations; a plain
/// identifier or a package-qualified selector exposes its name.
fn embedded_ident(source: &str, span: &Range<usize>) -> Option<(String, Range<usize>)> {
    let text = source.get(span.clone())?;
    let mut tokens = Lexer::new(text)
        .filter(|token| token.kind != TokenKind::Semicolon)
        .peekable();
    if tokens
        .peek()
        .is_some_and(|token| token.kind == TokenKind::Mul)
    {
        tokens.next();
    }
    let first = tokens
        .next()
        .filter(|token| token.kind == TokenKind::Ident)?;
    let name = if tokens
        .peek()
        .is_some_and(|token| token.kind == TokenKind::Period)
    {
        tokens.next();
        tokens
            .next()
            .filter(|token| token.kind == TokenKind::Ident)?
    } else {
        first
    };
    if tokens
        .peek()
        .is_some_and(|token| token.kind == TokenKind::LBrack)
    {
        let mut depth = 0usize;
        for token in tokens {
            match token.kind {
                TokenKind::LBrack => depth += 1,
                TokenKind::RBrack => depth = depth.checked_sub(1)?,
                _ => {}
            }
            if depth == 0 {
                break;
            }
        }
        if depth != 0 {
            return None;
        }
    } else if tokens.peek().is_some() {
        return None;
    }
    Some((
        text.get(name.span.clone())?.to_owned(),
        span.start + name.span.start..span.start + name.span.end,
    ))
}

fn signature_text(
    source: &str,
    signature: &crate::syntax::Signature,
    budget: &mut Budget,
    depth: usize,
) -> Option<String> {
    let mut out = String::new();
    budget.write(&mut out, "(")?;
    let parameters = field_list_text(source, &signature.parameters, budget, depth)?;
    budget.write(&mut out, &parameters)?;
    budget.write(&mut out, ")")?;
    write_results(source, signature, &mut out, budget, depth)?;
    Some(out)
}

fn write_results(
    source: &str,
    signature: &crate::syntax::Signature,
    out: &mut String,
    budget: &mut Budget,
    depth: usize,
) -> Option<()> {
    match signature.results.as_slice() {
        [] => {}
        [result] if result.name.is_none() => {
            budget.write(out, " ")?;
            let text = type_text(source, &result.annotation, budget, depth + 1)?;
            budget.write(out, &text)?;
        }
        results => {
            budget.write(out, " (")?;
            let results = field_list_text(source, results, budget, depth)?;
            budget.write(out, &results)?;
            budget.write(out, ")")?;
        }
    }
    Some(())
}

/// go/types `writeFieldList` with `", "` separators and grouped names.
fn field_list_text(
    source: &str,
    parameters: &[crate::syntax::Parameter],
    budget: &mut Budget,
    depth: usize,
) -> Option<String> {
    let mut out = String::new();
    let mut index = 0;
    while index < parameters.len() {
        let first = &parameters[index];
        let mut end = index + 1;
        if first.name.is_some() {
            while end < parameters.len()
                && parameters[end].name.is_some()
                && parameters[end].annotation == first.annotation
                && parameters[end].variadic == first.variadic
            {
                end += 1;
            }
        }
        if index != 0 {
            budget.write(&mut out, ", ")?;
        }
        if first.name.is_some() {
            for (slot, parameter) in parameters[index..end].iter().enumerate() {
                if slot != 0 {
                    budget.write(&mut out, ", ")?;
                }
                let name = parameter.name.as_ref()?;
                budget.write(&mut out, source.get(name.clone())?)?;
            }
            budget.write(&mut out, " ")?;
        }
        if first.variadic {
            budget.write(&mut out, "...")?;
        }
        let text = type_text(source, &first.annotation, budget, depth + 1)?;
        budget.write(&mut out, &text)?;
        index = end;
    }
    Some(out)
}

fn type_text(
    source: &str,
    span: &Range<usize>,
    budget: &mut Budget,
    depth: usize,
) -> Option<String> {
    let mut cursor = Cursor::new(source, span, budget)?;
    let mut out = String::new();
    format_constraint(&mut cursor, &mut out, budget, depth)?;
    cursor.finish()?;
    Some(out)
}

/// Interface members store the signature without its `func` keyword; gopls
/// prints `types.ExprString` of the field type, so restore it for parsing.
fn interface_signature_text(
    source: &str,
    span: &Range<usize>,
    budget: &mut Budget,
    depth: usize,
) -> Option<String> {
    let text = source.get(span.clone())?;
    let mut prepared = String::with_capacity(text.len() + 4);
    prepared.push_str("func");
    prepared.push_str(text);
    let signature = crate::syntax::parse_literal_signature(&prepared);
    if !signature.diagnostics.is_empty() {
        return None;
    }
    signature_text(&prepared, &signature, budget, depth)
}

#[derive(Clone)]
struct Tok {
    kind: TokenKind,
    span: Range<usize>,
}

struct Cursor<'s> {
    source: &'s str,
    tokens: Vec<Tok>,
    index: usize,
}

impl<'s> Cursor<'s> {
    fn new(source: &'s str, span: &Range<usize>, budget: &mut Budget) -> Option<Self> {
        if span.start > span.end || span.end > source.len() || span.len() > MAX_TEXT_SPAN {
            return None;
        }
        let mut tokens = Vec::new();
        for token in Lexer::new(&source[span.clone()]) {
            if matches!(token.kind, TokenKind::Comment | TokenKind::Semicolon) {
                continue;
            }
            budget.take(1)?;
            tokens.push(Tok {
                kind: token.kind,
                span: span.start + token.span.start..span.start + token.span.end,
            });
        }
        Some(Self {
            source,
            tokens,
            index: 0,
        })
    }

    fn peek(&self) -> Option<TokenKind> {
        self.tokens.get(self.index).map(|token| token.kind)
    }

    fn next(&mut self) -> Option<Tok> {
        let token = self.tokens.get(self.index)?.clone();
        self.index += 1;
        Some(token)
    }

    fn write(&self, out: &mut String, token: &Tok, budget: &mut Budget) -> Option<()> {
        budget.write(out, self.source.get(token.span.clone())?)
    }

    fn skip_to(&mut self, end: usize) {
        while self
            .tokens
            .get(self.index)
            .is_some_and(|token| token.span.start < end)
        {
            self.index += 1;
        }
    }

    fn finish(self) -> Option<()> {
        (self.index == self.tokens.len()).then_some(())
    }
}

fn expect(cursor: &mut Cursor<'_>, kind: TokenKind, out: &mut String, text: &str) -> Option<()> {
    cursor.next().filter(|token| token.kind == kind)?;
    out.push_str(text);
    Some(())
}

/// One interface constraint: type terms joined by `|`.
fn format_constraint(
    cursor: &mut Cursor<'_>,
    out: &mut String,
    budget: &mut Budget,
    depth: usize,
) -> Option<()> {
    format_type(cursor, out, budget, depth)?;
    while cursor.peek() == Some(TokenKind::Or) {
        cursor.next();
        budget.write(out, " | ")?;
        format_type(cursor, out, budget, depth)?;
    }
    Some(())
}

fn format_type(
    cursor: &mut Cursor<'_>,
    out: &mut String,
    budget: &mut Budget,
    depth: usize,
) -> Option<()> {
    if depth >= MAX_DEPTH {
        return None;
    }
    let token = cursor.next()?;
    match token.kind {
        TokenKind::Mul => {
            budget.write(out, "*")?;
            format_type(cursor, out, budget, depth + 1)?;
        }
        TokenKind::LBrack => {
            match cursor.peek() {
                Some(TokenKind::RBrack) => {
                    cursor.next();
                    budget.write(out, "[]")?;
                }
                Some(TokenKind::Ellipsis) => {
                    cursor.next();
                    expect(cursor, TokenKind::RBrack, out, "[...]")?;
                }
                _ => {
                    budget.write(out, "[")?;
                    format_expr(cursor, out, budget, depth + 1)?;
                    expect(cursor, TokenKind::RBrack, out, "]")?;
                }
            }
            format_type(cursor, out, budget, depth + 1)?;
        }
        TokenKind::Map => {
            budget.write(out, "map[")?;
            cursor
                .next()
                .filter(|token| token.kind == TokenKind::LBrack)?;
            format_type(cursor, out, budget, depth + 1)?;
            expect(cursor, TokenKind::RBrack, out, "]")?;
            format_type(cursor, out, budget, depth + 1)?;
        }
        TokenKind::Chan => {
            if cursor.peek() == Some(TokenKind::Arrow) {
                cursor.next();
                budget.write(out, "chan<- ")?;
            } else {
                budget.write(out, "chan ")?;
            }
            format_type(cursor, out, budget, depth + 1)?;
        }
        TokenKind::Arrow => {
            cursor
                .next()
                .filter(|token| token.kind == TokenKind::Chan)?;
            budget.write(out, "<-chan ")?;
            format_type(cursor, out, budget, depth + 1)?;
        }
        TokenKind::Func => format_function_type(cursor, out, budget, depth + 1)?,
        TokenKind::Struct => format_aggregate(cursor, out, budget, depth + 1, false)?,
        TokenKind::Interface => format_aggregate(cursor, out, budget, depth + 1, true)?,
        TokenKind::LParen => {
            budget.write(out, "(")?;
            format_type(cursor, out, budget, depth + 1)?;
            expect(cursor, TokenKind::RParen, out, ")")?;
        }
        TokenKind::Tilde => {
            budget.write(out, "~")?;
            format_type(cursor, out, budget, depth + 1)?;
        }
        TokenKind::Ident => {
            cursor.write(out, &token, budget)?;
            if cursor.peek() == Some(TokenKind::Period) {
                cursor.next();
                let field = cursor
                    .next()
                    .filter(|token| token.kind == TokenKind::Ident)?;
                budget.write(out, ".")?;
                cursor.write(out, &field, budget)?;
            }
            if cursor.peek() == Some(TokenKind::LBrack) {
                cursor.next();
                budget.write(out, "[")?;
                loop {
                    format_type(cursor, out, budget, depth + 1)?;
                    if cursor.peek() != Some(TokenKind::Comma) {
                        break;
                    }
                    cursor.next();
                    budget.write(out, ", ")?;
                }
                expect(cursor, TokenKind::RBrack, out, "]")?;
            }
        }
        _ => return None,
    }
    Some(())
}

/// go/types `ExprString` of a struct/interface literal: full field list text.
fn format_aggregate(
    cursor: &mut Cursor<'_>,
    out: &mut String,
    budget: &mut Budget,
    depth: usize,
    interface: bool,
) -> Option<()> {
    let start = cursor
        .tokens
        .get(cursor.index.checked_sub(1)?)
        .map(|token| token.span.start)?;
    let prefix = crate::syntax::parse_type_prefix(cursor.source.get(start..)?);
    if !prefix.diagnostics.is_empty() {
        return None;
    }
    let end = start + prefix.span.end;
    let mut tokens = Lexer::new(cursor.source.get(start..end)?).peekable();
    tokens.next();
    let container = tokens.next()?.span.start;
    budget.write(out, if interface { "interface{" } else { "struct{" })?;
    let mut first = true;
    if interface {
        for member in prefix
            .interface_members
            .iter()
            .filter(|member| member.container == container)
        {
            if !first {
                budget.write(out, "; ")?;
            }
            first = false;
            let annotation = start + member.annotation.start..start + member.annotation.end;
            if let Some(name) = member.name.as_ref() {
                budget.write(
                    out,
                    cursor.source.get(start + name.start..start + name.end)?,
                )?;
                let signature =
                    interface_signature_text(cursor.source, &annotation, budget, depth + 1)?;
                budget.write(out, &signature)?;
            } else {
                let text = type_text(cursor.source, &annotation, budget, depth + 1)?;
                budget.write(out, &text)?;
            }
        }
    } else {
        let fields: Vec<_> = prefix
            .fields
            .iter()
            .filter(|field| field.container == container)
            .collect();
        let mut index = 0;
        while index < fields.len() {
            let field = fields[index];
            let mut end = index + 1;
            while end < fields.len()
                && fields[end].embedded == field.embedded
                && fields[end].annotation == field.annotation
            {
                end += 1;
            }
            if !first {
                budget.write(out, "; ")?;
            }
            first = false;
            let annotation = start + field.annotation.start..start + field.annotation.end;
            let text = type_text(cursor.source, &annotation, budget, depth + 1)?;
            if field.embedded {
                budget.write(out, &text)?;
            } else {
                for (slot, entry) in fields[index..end].iter().enumerate() {
                    if slot != 0 {
                        budget.write(out, ", ")?;
                    }
                    budget.write(
                        out,
                        cursor
                            .source
                            .get(start + entry.name.start..start + entry.name.end)?,
                    )?;
                }
                budget.write(out, " ")?;
                budget.write(out, &text)?;
            }
            index = end;
        }
    }
    budget.write(out, "}")?;
    cursor.skip_to(end);
    Some(())
}

fn format_function_type(
    cursor: &mut Cursor<'_>,
    out: &mut String,
    budget: &mut Budget,
    depth: usize,
) -> Option<()> {
    let start = cursor
        .tokens
        .get(cursor.index.checked_sub(1)?)
        .map(|token| token.span.start)?;
    let prefix = crate::syntax::parse_type_prefix(cursor.source.get(start..)?);
    if !prefix.diagnostics.is_empty() {
        return None;
    }
    let end = start + prefix.span.end;
    let text = cursor.source.get(start..end)?;
    let signature = crate::syntax::parse_literal_signature(text);
    if !signature.diagnostics.is_empty() {
        return None;
    }
    budget.write(out, "func")?;
    let formatted = signature_text(text, &signature, budget, depth + 1)?;
    budget.write(out, &formatted)?;
    cursor.skip_to(end);
    Some(())
}

fn format_expr(
    cursor: &mut Cursor<'_>,
    out: &mut String,
    budget: &mut Budget,
    depth: usize,
) -> Option<()> {
    if depth >= MAX_DEPTH {
        return None;
    }
    format_unary(cursor, out, budget, depth + 1)?;
    while let Some(kind) = cursor.peek() {
        if !binary(kind) {
            break;
        }
        let token = cursor.next()?;
        budget.write(out, " ")?;
        cursor.write(out, &token, budget)?;
        budget.write(out, " ")?;
        format_unary(cursor, out, budget, depth + 1)?;
    }
    Some(())
}

fn format_unary(
    cursor: &mut Cursor<'_>,
    out: &mut String,
    budget: &mut Budget,
    depth: usize,
) -> Option<()> {
    if depth >= MAX_DEPTH {
        return None;
    }
    match cursor.peek()? {
        TokenKind::Add
        | TokenKind::Sub
        | TokenKind::Not
        | TokenKind::Xor
        | TokenKind::Mul
        | TokenKind::And
        | TokenKind::Arrow => {
            let token = cursor.next()?;
            cursor.write(out, &token, budget)?;
            format_unary(cursor, out, budget, depth + 1)
        }
        _ => {
            format_primary(cursor, out, budget, depth + 1)?;
            format_postfix(cursor, out, budget, depth + 1)
        }
    }
}

fn format_primary(
    cursor: &mut Cursor<'_>,
    out: &mut String,
    budget: &mut Budget,
    depth: usize,
) -> Option<()> {
    let token = cursor.next()?;
    match token.kind {
        TokenKind::Ident => {
            cursor.write(out, &token, budget)?;
            if cursor.peek() == Some(TokenKind::Period) {
                cursor.next();
                let field = cursor
                    .next()
                    .filter(|token| token.kind == TokenKind::Ident)?;
                budget.write(out, ".")?;
                cursor.write(out, &field, budget)?;
            }
        }
        TokenKind::Int
        | TokenKind::Float
        | TokenKind::Imag
        | TokenKind::Char
        | TokenKind::String => cursor.write(out, &token, budget)?,
        TokenKind::LParen => {
            budget.write(out, "(")?;
            format_expr(cursor, out, budget, depth + 1)?;
            expect(cursor, TokenKind::RParen, out, ")")?;
        }
        TokenKind::LBrack
        | TokenKind::Map
        | TokenKind::Chan
        | TokenKind::Func
        | TokenKind::Struct
        | TokenKind::Interface
        | TokenKind::Mul
        | TokenKind::Arrow => {
            cursor.index = cursor.index.checked_sub(1)?;
            format_type(cursor, out, budget, depth + 1)?;
        }
        _ => return None,
    }
    Some(())
}

fn format_postfix(
    cursor: &mut Cursor<'_>,
    out: &mut String,
    budget: &mut Budget,
    depth: usize,
) -> Option<()> {
    loop {
        match cursor.peek() {
            Some(TokenKind::Period) => {
                cursor.next();
                let field = cursor
                    .next()
                    .filter(|token| token.kind == TokenKind::Ident)?;
                budget.write(out, ".")?;
                cursor.write(out, &field, budget)?;
            }
            Some(TokenKind::LParen) => {
                cursor.next();
                budget.write(out, "(")?;
                if cursor.peek() != Some(TokenKind::RParen) {
                    loop {
                        format_expr(cursor, out, budget, depth + 1)?;
                        if cursor.peek() == Some(TokenKind::Ellipsis) {
                            cursor.next();
                            budget.write(out, "...")?;
                            break;
                        }
                        if cursor.peek() != Some(TokenKind::Comma) {
                            break;
                        }
                        cursor.next();
                        budget.write(out, ", ")?;
                    }
                }
                expect(cursor, TokenKind::RParen, out, ")")?;
            }
            Some(TokenKind::LBrack) => {
                cursor.next();
                budget.write(out, "[")?;
                if cursor.peek() != Some(TokenKind::Colon) {
                    format_expr(cursor, out, budget, depth + 1)?;
                }
                let mut colons = 0;
                while colons < 2 && cursor.peek() == Some(TokenKind::Colon) {
                    cursor.next();
                    budget.write(out, ":")?;
                    colons += 1;
                    if cursor.peek() != Some(TokenKind::RBrack) {
                        format_expr(cursor, out, budget, depth + 1)?;
                    }
                }
                expect(cursor, TokenKind::RBrack, out, "]")?;
            }
            _ => return Some(()),
        }
    }
}

fn binary(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Add
            | TokenKind::Sub
            | TokenKind::Mul
            | TokenKind::Quo
            | TokenKind::Rem
            | TokenKind::And
            | TokenKind::Or
            | TokenKind::Xor
            | TokenKind::Shl
            | TokenKind::Shr
            | TokenKind::AndNot
            | TokenKind::Land
            | TokenKind::Lor
            | TokenKind::Eql
            | TokenKind::Neq
            | TokenKind::Lss
            | TokenKind::Leq
            | TokenKind::Gtr
            | TokenKind::Geq
    )
}
