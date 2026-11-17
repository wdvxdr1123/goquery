//! Native descriptions for lexical bindings, fields and methods.

pub(super) use crate::analysis::query::documentation::declaration_keyword;
use crate::analysis::query::documentation::{
    control_header_statement, in_function_header, multi_name_short_declaration,
};

pub(super) use crate::analysis::query::documentation::{field_start, line_start_token};

/// Effective alias of an import, including the path suffix fallback.
fn import_alias(workspace: &Workspace, import: &crate::syntax::Import) -> Option<String> {
    if let Some(name) = workspace.import_name(import) {
        return Some(name.to_owned());
    }
    import
        .explicit_name()
        .map(str::to_owned)
        .or_else(|| import.path.rsplit('/').next().map(str::to_owned))
}

/// Whether an import alias resolves to a package that gopls could load.
///
/// Indexed packages resolve directly. Otherwise the module mount directory is
/// inspected for a host-active, non-cgo Go file: cgo-disabled contexts treat
/// cgo-only packages as unresolvable, exactly like go/types' invalid imports.
fn import_resolves(workspace: &Workspace, alias_file: &Path, alias: &str) -> bool {
    if crate::analysis::resolve::imported_symbol_iter(workspace, alias_file, alias)
        .next()
        .is_some()
    {
        return true;
    }
    let Some(stub) = workspace.files.get(alias_file) else {
        return true;
    };
    let Some(import) = stub
        .imports
        .iter()
        .find(|import| import_alias(workspace, import).as_deref() == Some(alias))
    else {
        return true;
    };
    let mut directories = Vec::new();
    for (root, module) in &workspace.modules {
        let suffix = if module == "std" {
            Some(import.path.clone())
        } else if import.path == *module {
            Some(String::new())
        } else {
            import
                .path
                .strip_prefix(module.as_str())
                .and_then(|rest| rest.strip_prefix('/'))
                .map(str::to_owned)
        };
        if let Some(suffix) = suffix {
            directories.push(if suffix.is_empty() {
                root.clone()
            } else {
                root.join(&suffix)
            });
        }
    }
    directories.into_iter().any(|directory| {
        workspace
            .files
            .range::<Path, _>((
                std::ops::Bound::Included(directory.as_path()),
                std::ops::Bound::Unbounded,
            ))
            .take_while(|(path, _)| path.starts_with(&directory))
            .any(|(path, stub)| {
                path.parent() == Some(directory.as_path())
                    && crate::workspace::production_file(path)
                    && !stub.package.is_empty()
                    && workspace.view_file_visible(alias_file, path)
            })
    })
}

/// Render a type/signature text with go/types' invalid-type replacement.
pub(super) fn replace_invalid_qualified(
    workspace: &Workspace,
    alias_file: &Path,
    text: &str,
) -> String {
    let Some(stub) = workspace.files.get(alias_file) else {
        return text.to_owned();
    };
    let unresolved: Vec<String> = stub
        .imports
        .iter()
        .filter(|import| !import.is_dot() && !import.is_blank())
        .filter_map(|import| import_alias(workspace, import))
        .filter(|name| !import_resolves(workspace, alias_file, name))
        .collect();
    if unresolved.is_empty() {
        return text.to_owned();
    }
    let mut output = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        let mut matched = None;
        for alias in &unresolved {
            let star = bytes[index] == b'*';
            let base = if star { index + 1 } else { index };
            let previous_word = star
                && bytes
                    .get(index.wrapping_sub(1))
                    .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_');
            let Some(rest) = text.get(base..) else {
                continue;
            };
            if !rest.starts_with(alias.as_str()) || previous_word {
                continue;
            }
            let mut cursor = base + alias.len();
            if bytes.get(cursor) != Some(&b'.') {
                continue;
            }
            cursor += 1;
            if !bytes
                .get(cursor)
                .is_some_and(|byte| byte.is_ascii_alphabetic() || *byte == b'_')
            {
                continue;
            }
            cursor += 1;
            while cursor < bytes.len()
                && (bytes[cursor].is_ascii_alphanumeric() || bytes[cursor] == b'_')
            {
                cursor += 1;
            }
            if bytes.get(cursor) == Some(&b'[')
                && let Some(close) = text[cursor..].find(']')
            {
                cursor += close + 1;
            }
            matched = Some(cursor);
            break;
        }
        if let Some(end) = matched {
            output.push_str("invalid type");
            index = end;
            continue;
        }
        let ch = text[index..].chars().next().unwrap_or('\0');
        output.push(ch);
        index += ch.len_utf8();
    }
    output
}

/// `types.TypeString`-style anonymous aggregate spacing and separator trimming.
pub(super) fn normalize_type(text: &str) -> String {
    let text = strip_comments(text);
    let text = canonical_multiline_aggregates(&text);
    let mut output = collapse_whitespace(&text);
    output = outside_literals(&output, |text| {
        text.replace("struct {", "struct{")
            .replace("struct{ ", "struct{")
            .replace("interface {", "interface{")
            .replace("interface{ ", "interface{")
    });
    output = outside_literals(&output, |text| {
        let mut text = text.to_owned();
        while text.contains("; }") {
            text = text.replace("; }", "}");
        }
        text.replace(" }", "}")
    });
    output = canonical_array_lengths(&output);
    output = canonical_tags(&output);
    output = expand_grouped_fields(&output);
    output.trim().to_owned()
}

/// Apply spelling-only presentation changes to code, preserving literal tokens.
fn outside_literals(text: &str, mut transform: impl FnMut(&str) -> String) -> String {
    let mut output = String::with_capacity(text.len());
    let mut start = 0;
    for token in crate::syntax::Lexer::new(text)
        .filter(|token| matches!(token.kind, TokenKind::String | TokenKind::Char))
    {
        output.push_str(&transform(&text[start..token.span.start]));
        output.push_str(&text[token.span.clone()]);
        start = token.span.end;
    }
    output.push_str(&transform(&text[start..]));
    output
}

/// At a quote, use the shared lexer to skip escaped and multiline literals.
fn literal_end(text: &str, index: usize) -> Option<usize> {
    matches!(text.as_bytes()[index], b'"' | b'\'' | b'`')
        .then(|| crate::syntax::Lexer::new(&text[index..]).next())
        .flatten()
        .map(|token| index + token.span.end)
}

/// `types.TypeString` expands grouped struct/interface field names.
fn expand_grouped_fields(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut output = String::with_capacity(text.len());
    let mut index = 0usize;
    while index < bytes.len() {
        let aggregate = crate::syntax::Lexer::new(&text[index..]).find_map(|token| {
            let keyword = match token.kind {
                TokenKind::Struct => "struct{",
                TokenKind::Interface => "interface{",
                _ => return None,
            };
            let start = index + token.span.start;
            text[start..]
                .starts_with(keyword)
                .then_some((start, keyword))
        });
        let Some((start, keyword)) = aggregate else {
            output.push_str(&text[index..]);
            break;
        };
        output.push_str(&text[index..start]);
        let open = start + keyword.len() - 1;
        let Some(close) = matching_brace(text, open) else {
            output.push_str(&text[start..]);
            break;
        };
        output.push_str(keyword);
        let body = &text[open + 1..close];
        let segments: Vec<&str> = split_top_level(body, ';');
        let mut expanded = Vec::new();
        for segment in segments {
            if !segment.trim().is_empty() {
                expand_grouped_segment(segment, &mut expanded);
            }
        }
        output.push_str(&expanded.join("; "));
        output.push('}');
        index = close + 1;
    }
    output
}

fn expand_grouped_segment(segment: &str, out: &mut Vec<String>) {
    let trimmed = segment.trim();
    let mut names = Vec::new();
    let mut tokens = crate::syntax::Lexer::new(trimmed).peekable();
    let mut end = 0;
    while let Some(token) = tokens.next() {
        if token.kind != TokenKind::Ident {
            break;
        }
        names.push(&trimmed[token.span.clone()]);
        end = token.span.end;
        if tokens
            .peek()
            .is_none_or(|token| token.kind != TokenKind::Comma)
        {
            break;
        }
        tokens.next();
    }
    if names.len() < 2 {
        out.push(trimmed.to_owned());
        return;
    }
    let rest = trimmed[end..].trim_start();
    if rest.is_empty() {
        out.push(trimmed.to_owned());
        return;
    }
    for name in names {
        out.push(format!("{name} {rest}"));
    }
}

use crate::analysis::query::signatures::label::strip_comments;

/// `types.TypeString` renders struct tags as interpreted Go strings.
fn canonical_tags(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut start = 0;
    for token in crate::syntax::Lexer::new(text).filter(|token| token.kind == TokenKind::String) {
        output.push_str(&text[start..token.span.start]);
        let literal = &text[token.span.clone()];
        match crate::syntax::decode_string_literal(literal) {
            Some(decoded) => output.push_str(&super::value::quote_bytes(&decoded)),
            None => output.push_str(literal),
        }
        start = token.span.end;
    }
    output.push_str(&text[start..]);
    output
}

/// Rewrite multiline struct/interface bodies as single-line `;`-separated ones.
fn canonical_multiline_aggregates(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut output = String::with_capacity(text.len());
    let mut index = 0usize;
    while index < bytes.len() {
        if let Some(end) = literal_end(text, index) {
            output.push_str(&text[index..end]);
            index = end;
            continue;
        }
        if text[index..].starts_with("struct")
            && bytes
                .get(index + 6)
                .is_none_or(|byte| !byte.is_ascii_alphanumeric() && *byte != b'_')
        {
            let mut open = index + 6;
            while bytes.get(open) == Some(&b' ') {
                open += 1;
            }
            if bytes.get(open) == Some(&b'{')
                && let Some(close) = matching_brace(text, open)
            {
                let body = &text[open + 1..close];
                output.push_str("struct{");
                output.push_str(&aggregate_body(&canonical_multiline_aggregates(body)));
                output.push('}');
                index = close + 1;
                continue;
            }
        }
        if text[index..].starts_with("interface")
            && bytes
                .get(index + 9)
                .is_none_or(|byte| !byte.is_ascii_alphanumeric() && *byte != b'_')
        {
            let mut open = index + 9;
            while bytes.get(open) == Some(&b' ') {
                open += 1;
            }
            if bytes.get(open) == Some(&b'{')
                && let Some(close) = matching_brace(text, open)
            {
                let body = &text[open + 1..close];
                output.push_str("interface{");
                output.push_str(&aggregate_body(&canonical_multiline_aggregates(body)));
                output.push('}');
                index = close + 1;
                continue;
            }
        }
        let ch = text[index..].chars().next().unwrap_or('\0');
        output.push(ch);
        index += ch.len_utf8();
    }
    output
}

fn aggregate_body(body: &str) -> String {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let parts: Vec<String> = split_top_level(trimmed, ';')
        .into_iter()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| line.trim_end_matches(';').trim_end().to_owned())
        .collect();
    if parts.len() <= 1 {
        return trimmed.trim_end_matches(';').trim_end().to_owned();
    }
    parts.join("; ")
}

fn matching_brace(text: &str, open: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for (offset, ch) in text[open..].char_indices() {
        if let Some(active) = quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' && active != '`' {
                escaped = true;
            } else if ch == active {
                quote = None;
            }
            continue;
        }
        match ch {
            '"' | '\'' | '`' => quote = Some(ch),
            '{' => depth += 1,
            '}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(open + offset);
                }
            }
            _ => {}
        }
    }
    None
}

/// `types.TypeString` prints array lengths as decimal constants.
fn canonical_array_lengths(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        if let Some(end) = literal_end(text, index) {
            output.push_str(&text[index..end]);
            index = end;
            continue;
        }
        if bytes[index] == b'['
            && let Some(close) = text[index + 1..].find(']')
        {
            let inner = &text[index + 1..index + 1 + close];
            if let Some(value) = array_length(inner) {
                output.push('[');
                output.push_str(&value.to_string());
                output.push(']');
                index += close + 2;
                continue;
            }
        }
        let ch = text[index..].chars().next().unwrap_or('\0');
        output.push(ch);
        index += ch.len_utf8();
    }
    output
}

fn array_length(text: &str) -> Option<i64> {
    let text = text.trim();
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
        return None;
    }
    let (digits, radix) =
        if let Some(rest) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
            (rest, 16)
        } else if let Some(rest) = text.strip_prefix("0b").or_else(|| text.strip_prefix("0B")) {
            (rest, 2)
        } else if let Some(rest) = text.strip_prefix("0o").or_else(|| text.strip_prefix("0O")) {
            (rest, 8)
        } else if text.len() > 1 && text.starts_with('0') {
            (&text[1..], 8)
        } else if text.bytes().all(|byte| byte.is_ascii_digit()) {
            (text, 10)
        } else {
            return None;
        };
    i64::from_str_radix(&digits.replace('_', ""), radix).ok()
}

/// Collapse source line breaks/tabs outside string and rune literals.
fn collapse_whitespace(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut pending_space = false;
    let mut index = 0;
    while index < text.len() {
        if let Some(end) = literal_end(text, index) {
            if pending_space && !output.is_empty() {
                output.push(' ');
            }
            pending_space = false;
            output.push_str(&text[index..end]);
            index = end;
            continue;
        }
        let ch = text[index..].chars().next().unwrap_or('\0');
        index += ch.len_utf8();
        match ch {
            ch if ch.is_whitespace() => pending_space = true,
            ch => {
                if pending_space && !output.is_empty() {
                    output.push(' ');
                }
                pending_space = false;
                output.push(ch);
            }
        }
    }
    output
}

/// Expand grouped parameter names inside function types like go/types does.
pub(super) fn normalize_signature(text: &str) -> String {
    normalize_type(&expand_function_types(text))
}

fn expand_function_types(text: &str) -> String {
    crate::analysis::query::signatures::label::function_types(text)
        .unwrap_or_else(|| text.to_owned())
}

fn split_top_level(text: &str, separator: char) -> Vec<&str> {
    let mut segments = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    for token in crate::syntax::Lexer::new(text) {
        match token.kind {
            TokenKind::LParen | TokenKind::LBrack | TokenKind::LBrace => depth += 1,
            TokenKind::RParen | TokenKind::RBrack | TokenKind::RBrace => {
                depth = depth.saturating_sub(1);
            }
            kind if depth == 0
                && (separator == ';' && kind == TokenKind::Semicolon
                    || separator == ',' && kind == TokenKind::Comma) =>
            {
                segments.push(&text[start..token.span.start]);
                start = token.span.end;
            }
            _ => {}
        }
    }
    segments.push(&text[start..]);
    segments
}

use super::content::HoverContent;
use super::{HoverLink, doc};
use crate::analysis::query::Context;
use crate::analysis::resolve::MemberDescription;
use crate::analysis::scope::{Binding, DeclarationKind, Locals, ObjectKind};
use crate::syntax::TokenKind;
use crate::text::SourceMap;
use crate::workspace::Workspace;
use std::ops::Range;
use std::path::Path;

const MAX_SIGNATURE_BYTES: usize = 16 * 1024;

pub(super) enum SourceText<'a> {
    Borrowed(&'a str),
    Owned(String),
}

impl SourceText<'_> {
    pub(super) fn text(&self) -> &str {
        match self {
            Self::Borrowed(text) => text,
            Self::Owned(text) => text,
        }
    }
}

/// One bounded read of a declaration file, borrowing the current document.
pub(super) fn declaration_source<'a>(
    context: &'a Context<'_>,
    file: &Path,
) -> Option<SourceText<'a>> {
    if file == context.file() {
        Some(SourceText::Borrowed(context.source()))
    } else {
        context.workspace().source(file).map(SourceText::Owned)
    }
}

/// Local values use only the region's checked binding type.
pub(super) fn binding_text(
    context: &Context<'_>,
    locals: &Locals,
    binding: &Binding,
) -> Option<String> {
    let workspace = context.workspace();
    let file = context.file();
    let source = context.source();
    if binding.declaration == DeclarationKind::TypeParameter {
        return locals
            .type_parameter_constraint(binding)
            .and_then(|span| source.get(span.clone()))
            .and_then(|text| {
                crate::analysis::query::signatures::label::signature_type(
                    workspace, file, file, text,
                )
            })
            .or_else(|| binding.declared_type.clone());
    }
    context
        .binding_presentation(binding)
        .map(|text| normalize_type(&text))
}

/// Local declaration display, using the region's checked binding type.
pub(super) fn local(
    context: &Context<'_>,
    locals: &Locals,
    binding: &Binding,
    range: super::SourceRange,
) -> HoverContent {
    let mut content = HoverContent::new(range);
    let file = context.file();
    let source = context.source();
    let workspace = context.workspace();
    if !in_function_header(context, binding.selection.start)
        && binding.object_kind != ObjectKind::Parameter
        && !multi_name_short_declaration(source, binding.selection.start)
        && !control_header_statement(source, binding.selection.start)
        && let Some(documents) = doc::attached(
            context,
            file,
            source,
            doc::Target {
                start: line_start_token(source, binding.selection.start),
                end: binding.selection.end,
                declaration: None,
                trailing: true,
            },
        )
    {
        content.doc = Some(documents.text);
        content.doc_links = documents.links;
    }
    content.code = match binding.declaration {
        DeclarationKind::TypeParameter => binding_text(context, locals, binding)
            .map(|constraint| format!("type parameter {} {constraint}", binding.name)),
        DeclarationKind::Type => binding
            .annotation()
            .and_then(|span| source.get(span.clone()))
            .and_then(|annotation| {
                crate::analysis::query::signatures::label::signature_type(
                    workspace, file, file, annotation,
                )
            })
            .filter(|annotation| annotation.len() <= MAX_SIGNATURE_BYTES)
            .map(|annotation| {
                let line_start = source[..binding.selection.start]
                    .rfind('\n')
                    .map_or(0, |index| index + 1);
                let prefix = &source[line_start..binding.selection.start];
                let indentation = &prefix[..prefix
                    .bytes()
                    .take_while(|byte| matches!(byte, b' ' | b'\t'))
                    .count()];
                let spec = format!(
                    "{} {}",
                    binding.name,
                    declaration_without_indent(&annotation, indentation)
                );
                format!("type {}", super::declaration::format_spec(&spec))
            }),
        DeclarationKind::Constant => local_constant(context, binding),
        DeclarationKind::Value | DeclarationKind::Variadic => {
            binding_text(context, locals, binding).map(|ty| format!("var {} {ty}", binding.name))
        }
    };
    if matches!(
        binding.declaration,
        DeclarationKind::Type | DeclarationKind::TypeParameter
    ) && let Some(code) = content.code.take()
    {
        content.code = Some(replace_invalid_qualified(workspace, file, &code));
    }
    content
}

/// Remove a local declaration's enclosing indentation without changing literals.
fn declaration_without_indent(text: &str, indentation: &str) -> String {
    if indentation.is_empty() {
        return text.to_owned();
    }
    // Struct tags may be multiline raw literals: their contents are values,
    // so only source indentation outside a literal may be removed.
    let mut literals = crate::syntax::Lexer::new(text)
        .filter(|token| matches!(token.kind, TokenKind::String | TokenKind::Char))
        .peekable();
    let mut output = String::with_capacity(text.len());
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        while literals
            .peek()
            .is_some_and(|token| token.span.end <= offset)
        {
            literals.next();
        }
        let in_literal = literals
            .peek()
            .is_some_and(|token| token.span.start < offset && offset < token.span.end);
        output.push_str(if offset == 0 || in_literal {
            line
        } else {
            line.strip_prefix(indentation).unwrap_or(line)
        });
        offset += line.len();
    }
    output
}

fn local_constant(context: &Context<'_>, binding: &Binding) -> Option<String> {
    let source = context.source();
    let selection = [
        u32::try_from(binding.selection.start).ok()?,
        u32::try_from(binding.selection.end).ok()?,
    ];
    let (ty, value, span) = context.with_sema(|sema| {
        let (region, arena) =
            sema.region_and_arena_at(context.file(), source, selection[0], None)?;
        let (id, _) = crate::analysis::query::signatures::resolve_local_binding_id(
            region,
            selection,
            binding.scope_start(),
        )?;
        let operand = region.local_type(id)?;
        (operand.mode == crate::analysis::types::Mode::Constant).then_some(())?;
        let span = region.local_init(id)?;
        Some((
            arena.format_type(
                operand.ty,
                crate::analysis::types::Qualifier::Presentation(context.file()),
            )?,
            operand.value,
            span[0] as usize..span[1] as usize,
        ))
    })?;
    if span.len() > MAX_SIGNATURE_BYTES {
        return None;
    }
    let expression = source.get(span.clone())?;
    let own = binding.selection.end < span.start;
    let mut literal = super::value::literal(expression).or_else(|| {
        let crate::analysis::types::Constant::Int(value) = value? else {
            return None;
        };
        Some(super::value::Literal {
            value: value.to_string(),
            declaration: own.then(|| expression.to_owned()),
            untyped: None,
        })
    })?;
    if !own {
        literal.declaration = None;
    }
    format!(
        "const {} {} = {}",
        binding.name,
        normalize_type(&ty),
        literal.assigned()
    )
    .into()
}

/// Bindings declared by an enclosing function header: parameters, results,
/// receivers and type parameters.
pub(super) fn header_binding(
    context: &Context<'_>,
    span: &Range<usize>,
    range: super::SourceRange,
) -> Result<Option<HoverContent>, (i32, String)> {
    let source = context.source();
    for symbol in &context.workspace().files[context.file()].symbols {
        let Some(body) = symbol.body() else {
            continue;
        };
        let Some(declaration) = symbol.declaration() else {
            continue;
        };
        if !(declaration.start <= span.start && span.end <= body.start) {
            continue;
        }
        let Some(header) = source.get(declaration.start..body.start) else {
            continue;
        };
        if header.len() > MAX_SIGNATURE_BYTES {
            continue;
        }
        let parsed = crate::syntax::parse_signature(header);
        if !parsed.diagnostics.is_empty() {
            continue;
        }
        let locals = Locals::from_header(source, declaration.start, body.clone(), &parsed);
        if let Some(binding) = locals
            .definition_at(span, &source[span.clone()])
            .filter(|binding| {
                binding.selection == *span
                    || binding.declaration == DeclarationKind::TypeParameter
                        && parsed.type_references.iter().any(|reference| {
                            reference.qualifier.is_none()
                                && (declaration.start + reference.name.start
                                    ..declaration.start + reference.name.end)
                                    == *span
                        })
            })
        {
            return Ok(Some(local(context, &locals, binding, range)));
        }
    }
    Ok(None)
}

pub(super) fn member_id(
    context: &Context<'_>,
    identity: crate::analysis::obj::MemberDeclId<'_>,
    span: &Range<usize>,
    range: super::SourceRange,
) -> HoverContent {
    let Some(description) = identity.describe(context.workspace()) else {
        return HoverContent::new(range);
    };
    member(context, description, span, range)
}

pub(super) fn member(
    context: &Context<'_>,
    member: MemberDescription<'_>,
    span: &Range<usize>,
    range: super::SourceRange,
) -> HoverContent {
    let mut content = HoverContent::new(range);
    let Some(source_text) = declaration_source(context, member.file) else {
        return content;
    };
    let source = source_text.text();
    let map = SourceMap::new(source);
    let Some(data) = member.data_in_source(source, &map) else {
        return content;
    };
    let Some(annotation) = data.annotation.as_deref() else {
        return content;
    };
    let Some(identity) = member.identity_in_source(source) else {
        return content;
    };
    let selected = [u32::try_from(span.start).ok(), u32::try_from(span.end).ok()];
    let [Some(start), Some(end)] = selected else {
        return content;
    };
    let Some(view) = context.with_sema(|sema| {
        sema.member_type_view(context.file(), context.source(), [start, end], identity)
    }) else {
        return content;
    };
    let ty = if matches!(member.kind, crate::syntax::MemberKind::Field)
        && crate::syntax::Lexer::new(annotation)
            .next()
            .is_some_and(|token| token.kind == TokenKind::Func)
    {
        let parsed = crate::syntax::parse_literal_signature(annotation);
        let Some((signature, _)) = view.callable.as_ref().and_then(|callable| {
            crate::analysis::query::signatures::label::signature_from_view(
                annotation, &parsed, "", callable,
            )
        }) else {
            return content;
        };
        normalize_signature(&format!("func{}", signature.label))
    } else {
        normalize_type(&view.type_text)
    };
    let base = member.owner.end;
    let end = map.offset(member.owner.range.end).unwrap_or(source.len());
    let declaration = source.get(base..end);
    let entries = declaration.map(crate::syntax::aggregate_members);
    let entry = match (&declaration, &entries) {
        (Some(declaration), Some(entries)) => member.entry(declaration, entries),
        _ => None,
    };
    content.code = match member.kind {
        crate::syntax::MemberKind::Field => {
            let mut code = format!("field {} {ty}", member.name);
            if let (Some(declaration), Some(entry)) = (declaration, entry) {
                if let Some(tag) = field_tag(declaration, entry.annotation.end) {
                    code.push(' ');
                    code.push_str(&tag);
                }
                let declaring = member.file == context.file()
                    && map
                        .offset(data.selection.start)
                        .zip(map.offset(data.selection.end))
                        .is_some_and(|(start, end)| (start..end) == *span);
                if declaring {
                    let summary =
                        super::size::field_layout(context, member.file, member.owner, member.name)
                            .map(|field| {
                                let mut summary =
                                    format!("size={}", super::size::number(field.size));
                                if let Some(class) = super::size::size_class(field.size)
                                    .filter(|class| *class > field.size)
                                {
                                    summary.push_str(&format!(
                                        ", class={}",
                                        super::size::number(class)
                                    ));
                                }
                                summary.push_str(&format!(
                                    ", offset={}",
                                    super::size::number(field.offset)
                                ));
                                summary
                            });
                    if let Some(summary) = summary {
                        code.push_str(" // ");
                        code.push_str(&summary);
                    }
                }
            }
            Some(code)
        }
        crate::syntax::MemberKind::Method | crate::syntax::MemberKind::Interface => {
            let Some(owner) = view.receiver.as_deref() else {
                return content;
            };
            let parsed = crate::syntax::parse_method_signature(annotation);
            let Some((signature, _)) = view.callable.as_ref().and_then(|callable| {
                crate::analysis::query::signatures::label::signature_from_view(
                    annotation,
                    &parsed,
                    member.name,
                    callable,
                )
            }) else {
                return content;
            };
            let label = normalize_signature(&signature.label);
            Some(format!("func ({owner}) {label}"))
        }
    };
    if let Some((declaration, entry)) = declaration.zip(entry) {
        content.doc = doc::attached(
            context,
            member.file,
            source,
            doc::Target {
                start: field_start(declaration, entry.selection.start).map_or(
                    line_start_token(source, base + entry.selection.start),
                    |start| base + start,
                ),
                end: base + entry.annotation.end,
                declaration: None,
                trailing: true,
            },
        )
        .map(|documents| {
            content.doc_links = documents.links;
            documents.text
        });
    }
    if matches!(member.kind, crate::syntax::MemberKind::Field)
        && content.footer.is_none()
        && let Some(version) =
            super::version::added_field(context, member.file, &member.owner.name, member.name)
    {
        content.footer = Some(format!("Added in {version}"));
    }
    if matches!(
        member.kind,
        crate::syntax::MemberKind::Method | crate::syntax::MemberKind::Interface
    ) && content.footer.is_none()
        && let Some(version) =
            super::version::added_method(context, member.file, &member.owner.name, member.name)
    {
        content.footer = Some(format!("Added in {version}"));
    }
    if matches!(member.kind, crate::syntax::MemberKind::Field)
        && let Some(identity) = member.identity_in_source(source)
        && let Some(promotion) = context.with_sema(|sema| {
            sema.selected_promotion(
                context.file(),
                context.source(),
                [
                    u32::try_from(span.start).ok()?,
                    u32::try_from(span.end).ok()?,
                ],
            )
        })
        && promotion.object == crate::analysis::obj::ObjectRef::Field(identity)
        && !promotion.through.is_empty()
        && let Some(code) = content.code.as_mut()
    {
        code.push_str(" // through ");
        code.push_str(&promotion.through.join(", "));
    }
    content.link = member_link(context, member, &member.owner.name);
    content
}

/// Source text of a struct field tag immediately after its type.
fn field_tag(declaration: &str, annotation_end: usize) -> Option<String> {
    let rest = declaration.get(annotation_end..)?;
    let token = crate::syntax::Lexer::new(rest).next()?;
    if token.kind != TokenKind::String {
        return None;
    }
    rest.get(token.span).map(str::to_owned)
}

fn member_link(
    context: &Context<'_>,
    member: MemberDescription<'_>,
    owner_name: &str,
) -> Option<HoverLink> {
    let workspace = context.workspace();
    if !crate::syntax::is_exported_name(member.name) || !crate::syntax::is_exported_name(owner_name)
    {
        return None;
    }
    if member
        .file
        .file_name()?
        .to_string_lossy()
        .ends_with("_test.go")
    {
        return None;
    }
    if gopls_test_view_link(context) || platform_test_variant(context) {
        return None;
    }
    let consumer = context.file();
    if consumer
        .file_name()
        .is_some_and(|name| name.to_string_lossy().ends_with("_test.go"))
        && super::same_package(context, consumer, member.file)
    {
        return None;
    }
    Some(HoverLink {
        symbol: format!(
            "({}.{}).{}",
            workspace.files[member.file].package, owner_name, member.name
        ),
        path: link_import_path(workspace, member.file)?,
        anchor: format!("{owner_name}.{}", member.name),
    })
}

/// Import path as rendered in pkg.go.dev links: dependency modules keep their
/// resolved `module@version` prefix.
pub(super) fn link_import_path(workspace: &Workspace, origin: &Path) -> Option<String> {
    let path = workspace.import_path(origin)?;
    let (root, module) = workspace
        .modules
        .iter()
        .filter(|(root, _)| origin.starts_with(root))
        .max_by_key(|(root, _)| root.components().count())?;
    if module == "std" {
        return Some(path);
    }
    let Some((_, version)) = root
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.rsplit_once('@'))
    else {
        return Some(path);
    };
    let resolved = format!("{module}@{version}");
    Some(match path.strip_prefix(module.as_str()) {
        Some(rest) => format!("{resolved}{rest}"),
        None => resolved,
    })
}

pub(super) fn package_link(
    context: &Context<'_>,
    origin: &Path,
    name: &str,
    symbol: String,
) -> Option<HoverLink> {
    let workspace = context.workspace();
    if !crate::syntax::is_exported_name(name) {
        return None;
    }
    if origin.file_name()?.to_string_lossy().ends_with("_test.go") {
        return None;
    }
    if gopls_test_view_link(context) || platform_test_variant(context) {
        return None;
    }
    // gopls resolves objects through a test variant package (ForTest), whose
    // declaration links are suppressed. The package under test (same directory)
    // is loaded as its test variant even from a separate `_test` package.
    let consumer = context.file();
    if consumer
        .file_name()
        .is_some_and(|name| name.to_string_lossy().ends_with("_test.go"))
        && (super::same_package(context, consumer, origin)
            || (consumer.parent() == origin.parent()
                && context.workspace().orphan_view_target(consumer).is_none()))
    {
        return None;
    }
    Some(HoverLink {
        symbol,
        path: link_import_path(workspace, origin)?,
        anchor: name.to_owned(),
    })
}

/// Enclosing function literal `(func_start, body_start, body_end)` for a cursor.
fn nested_literal(source: &str, offset: usize) -> Option<(usize, usize, usize)> {
    let tokens: Vec<_> = crate::syntax::Lexer::new(source)
        .filter(|token| !token.inserted)
        .collect();
    let mut best: Option<(usize, usize, usize)> = None;
    for (index, token) in tokens.iter().enumerate() {
        if token.kind != TokenKind::Func || token.span.start >= offset {
            continue;
        }
        let Some(_open) = tokens
            .get(index + 1)
            .filter(|token| token.kind == TokenKind::LParen)
        else {
            continue;
        };
        let mut depth = 0usize;
        let mut close = None;
        for (position, token) in tokens.iter().enumerate().skip(index + 1) {
            match token.kind {
                TokenKind::LParen => depth += 1,
                TokenKind::RParen => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        close = Some(position);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(close) = close else { continue };
        let mut body_index = None;
        for (position, token) in tokens.iter().enumerate().skip(close + 1) {
            match token.kind {
                TokenKind::LBrace => {
                    body_index = Some(position);
                    break;
                }
                TokenKind::Semicolon | TokenKind::RBrace => break,
                _ => {}
            }
        }
        let Some(body_index) = body_index else {
            continue;
        };
        let body_start = tokens[body_index].span.start;
        let mut depth = 0usize;
        let mut body_end = None;
        for token in tokens.iter().skip(body_index) {
            match token.kind {
                TokenKind::LBrace => depth += 1,
                TokenKind::RBrace => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        body_end = Some(token.span.end);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(body_end) = body_end else { continue };
        if token.span.start < offset
            && offset < body_end
            && best.is_none_or(|(start, _, _)| token.span.start > start)
        {
            best = Some((token.span.start, body_start, body_end));
        }
    }
    best
}

/// Resolve a parameter of an enclosing function literal that the cursor scope
/// did not record (callbacks inside call arguments).
pub(super) fn nested_function_binding(
    context: &Context<'_>,
    offset: usize,
    selection: &Range<usize>,
    range: super::SourceRange,
) -> Option<HoverContent> {
    let source = context.source();
    let (func_start, body_start, body_end) = nested_literal(source, offset)?;
    let header = source.get(func_start..body_start)?;
    if header.len() > MAX_SIGNATURE_BYTES {
        return None;
    }
    let parsed = crate::syntax::parse_literal_signature(header);
    if !parsed.diagnostics.is_empty() {
        return None;
    }
    let locals = Locals::from_header(source, func_start, body_start..body_end, &parsed);
    let binding = locals
        .definition_at(selection, source.get(selection.clone())?)
        .filter(|binding| binding.selection == *selection)?;
    Some(local(context, &locals, binding, range))
}

/// Resolve a value name to an enclosing function literal parameter.
pub(super) fn nested_function_local(
    context: &Context<'_>,
    offset: usize,
    name: &str,
    range: super::SourceRange,
) -> Option<HoverContent> {
    let source = context.source();
    let literal = nested_literal(source, offset);
    let (func_start, body_start, body_end) = literal?;
    let header = source.get(func_start..body_start)?;
    if header.len() > MAX_SIGNATURE_BYTES {
        return None;
    }
    let parsed = crate::syntax::parse_literal_signature(header);
    if !parsed.diagnostics.is_empty() {
        return None;
    }
    let locals = Locals::from_header(source, func_start, body_start..body_end, &parsed);
    let binding = locals
        .visible_bindings(offset)
        .find(|binding| binding.name == name)?;
    Some(local(context, &locals, binding, range))
}

/// A `_test.go` file whose name carries the host GOOS/GOARCH suffix: gopls
/// resolves it through a dynamic platform view whose metadata hides links.
pub(super) fn platform_test_variant(context: &Context<'_>) -> bool {
    let consumer = context.file();
    let Some(name) = consumer.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let Some(stem) = name.strip_suffix("_test.go") else {
        return false;
    };
    let Some(suffix) = stem.rsplit('_').next() else {
        return false;
    };
    let host_goos = if cfg!(windows) { "windows" } else { "linux" };
    let host_goarch = if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "amd64"
    };
    suffix == host_goos || suffix == host_goarch
}

/// gopls suppresses declaration links for host-active detached test packages,
/// which it resolves through their test variant metadata.
pub(super) fn gopls_test_view_link(context: &Context<'_>) -> bool {
    let workspace = context.workspace();
    let consumer = context.file();
    consumer
        .file_name()
        .is_some_and(|name| name.to_string_lossy().ends_with("_test.go"))
        && workspace.is_detached(consumer)
        && workspace.orphan_view_target(consumer).is_none()
        && workspace
            .files
            .get(consumer)
            .is_some_and(|stub| stub.package.ends_with("_test"))
}

/// `[len(packageVAR)]` folded to its declared array literal length.
fn array_literal_length(context: &Context<'_>, origin: &Path, name: &str) -> Option<usize> {
    let workspace = context.workspace();
    let (file, symbol) = crate::analysis::resolve::unqualified_symbol(workspace, origin, name)?;
    if !symbol.is_var() {
        return None;
    }
    context.with_sema(|sema| {
        let operand = sema.operand_of_decl(crate::analysis::obj::DeclId { file, symbol })?;
        usize::try_from(sema.arena_mut().array_length(operand.ty)?).ok()
    })
}

pub(super) fn fold_array_lengths(context: &Context<'_>, origin: &Path, text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut index = 0usize;
    while let Some(offset) = text[index..].find("len(") {
        let start = index + offset;
        output.push_str(&text[index..start]);
        let open = start + "len".len();
        let mut depth = 0usize;
        let mut end = None;
        for (position, ch) in text[open..].char_indices() {
            match ch {
                '(' => depth += 1,
                ')' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        end = Some(open + position);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(end) = end else {
            output.push_str(&text[start..]);
            return output;
        };
        let inner = text[open + 1..end].trim();
        let identifier = !inner.is_empty()
            && inner
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_');
        if identifier && let Some(length) = array_literal_length(context, origin, inner) {
            output.push_str(&length.to_string());
        } else {
            output.push_str(&text[start..=end]);
        }
        index = end + 1;
    }
    output.push_str(&text[index..]);
    output
}

/// Declared package name for a presentation label.
pub(super) fn package_name(workspace: &Workspace, file: &Path) -> String {
    workspace
        .files
        .get(file)
        .map_or_else(String::new, |stub| stub.package.clone())
}
