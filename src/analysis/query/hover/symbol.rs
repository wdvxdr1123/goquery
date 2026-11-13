//! Package-level declaration descriptions (functions, types, vars, consts).
use super::content::HoverContent;
use super::render::{self, declaration_source};
use super::{HoverLink, doc, import_qualifier};
use crate::analysis::query::Context;
use crate::index::{IndexedSymbol as Symbol, SymbolKind};
use crate::syntax::{Lexer, TokenKind};
use std::path::Path;

const MAX_DECLARATION_BYTES: usize = 256 * 1024;
const MAX_METHODS: usize = 128;

pub(super) fn declare(
    context: &Context<'_>,
    file: &Path,
    symbol: &Symbol,
    range: super::SourceRange,
) -> Result<Option<HoverContent>, (i32, String)> {
    describe(context, file, symbol, range, true)
}

pub(super) fn reference(
    context: &Context<'_>,
    origin: &Path,
    symbol: &Symbol,
    range: super::SourceRange,
) -> Result<Option<HoverContent>, (i32, String)> {
    describe(context, origin, symbol, range, false)
}

fn describe(
    context: &Context<'_>,
    origin: &Path,
    symbol: &Symbol,
    range: super::SourceRange,
    declaring: bool,
) -> Result<Option<HoverContent>, (i32, String)> {
    let Some(source_text) = declaration_source(context, origin) else {
        return Ok(None);
    };
    let source = source_text.text();
    let content = match symbol.kind {
        SymbolKind::Function | SymbolKind::Method => {
            function(context, origin, source, symbol, range)
        }
        SymbolKind::Type => type_symbol(context, origin, source, symbol, range, declaring),
        SymbolKind::Variable | SymbolKind::Constant => {
            value_symbol(context, origin, source, symbol, range, declaring)
        }
    };
    let Some(mut content) = content else {
        return Ok(None);
    };
    let workspace = context.workspace();
    let consumer = context.file();
    if !symbol.is_type()
        && let Some(code) = content.code.take()
    {
        content.code = Some(render::replace_invalid_qualified(
            workspace, consumer, &code,
        ));
    }
    for block in &mut content.extra {
        *block = render::replace_invalid_qualified(workspace, consumer, block);
    }
    if content.footer.is_none()
        && let Some(version) = super::version::added(context, origin, symbol)
    {
        content.footer = Some(format!("Added in {version}"));
    }
    Ok(Some(content))
}

fn function(
    context: &Context<'_>,
    origin: &Path,
    source: &str,
    symbol: &Symbol,
    range: super::SourceRange,
) -> Option<HoverContent> {
    let workspace = context.workspace();
    let consumer = context.file();
    let declaration = symbol.declaration()?;
    let end = symbol.body().map_or(declaration.end, |body| body.start);
    let header = source.get(declaration.start..end)?;
    if header.len() > MAX_DECLARATION_BYTES {
        return None;
    }
    let parsed = crate::syntax::parse_signature(header);
    if !parsed.diagnostics.is_empty() {
        return None;
    }
    let (signature, _) =
        workspace.declared_signature(consumer, origin, header, &parsed, &symbol.name)?;
    let mut code = String::from("func ");
    if symbol.is_method() {
        let receiver_line = receiver_line(context, origin, header, &parsed)?;
        code.push_str(&receiver_line);
        code.push(' ');
    } else {
        code.push_str(&import_qualifier(context, consumer, origin)?);
    }
    code.push_str(&render::normalize_signature(&signature.label));
    if symbol.generic && !parsed.type_parameters.is_empty() {
        let mut parameters = Vec::with_capacity(parsed.type_parameters.len());
        for parameter in &parsed.type_parameters {
            let Some(name) = parameter
                .name
                .as_ref()
                .and_then(|name| header.get(name.clone()))
            else {
                continue;
            };
            let constraint = header.get(parameter.annotation.clone())?;
            let constraint = workspace
                .signature_type(consumer, origin, constraint)
                .map(|text| render::normalize_type(&text))
                .unwrap_or_else(|| constraint.trim().to_owned());
            parameters.push(format!("{name} {constraint}"));
        }
        if !parameters.is_empty()
            && let Some(open) = code.find('(')
        {
            code.insert_str(open, &format!("[{}]", parameters.join(", ")));
        }
    }
    let mut content = HoverContent::new(range);
    content.code = Some(code);
    content.doc = attach(
        context,
        origin,
        source,
        declaration.start,
        declaration.end,
        Some(declaration.start),
        false,
    )
    .map(|documents| {
        content.doc_links = documents.links;
        documents.text
    });
    content.link = if symbol.is_method() {
        method_link(context, origin, symbol)
    } else {
        render::package_link(
            context,
            origin,
            &symbol.name,
            format!(
                "{}.{}",
                render::package_name(workspace, origin),
                symbol.name
            ),
        )
    };
    Some(content)
}

/// `(name Type)` receiver prefix, or `(Type)` for unnamed receivers.
fn receiver_line(
    context: &Context<'_>,
    origin: &Path,
    header: &str,
    parsed: &crate::syntax::Signature,
) -> Option<String> {
    let workspace = context.workspace();
    let receiver = parsed.receiver.first()?;
    let annotation = header.get(receiver.annotation.clone())?;
    let receiver_type = workspace.signature_type(context.file(), origin, annotation)?;
    let name = receiver
        .name
        .as_ref()
        .map(|span| header[span.clone()].trim())
        .filter(|name| !name.is_empty() && *name != "_");
    Some(match name {
        Some(name) => format!("({name} {receiver_type})"),
        None => format!("({receiver_type})"),
    })
}

fn method_link(context: &Context<'_>, origin: &Path, symbol: &Symbol) -> Option<HoverLink> {
    if render::gopls_test_view_link(context) || render::platform_test_variant(context) {
        return None;
    }
    let workspace = context.workspace();
    let receiver = symbol.receiver.as_deref()?;
    let (_, owner) = workspace.type_declaration(origin, receiver)?;
    if !crate::syntax::is_exported_name(&symbol.name)
        || !crate::syntax::is_exported_name(&owner.name)
    {
        return None;
    }
    if origin.file_name()?.to_string_lossy().ends_with("_test.go") {
        return None;
    }
    let consumer_file = context.file();
    if consumer_file
        .file_name()
        .is_some_and(|name| name.to_string_lossy().ends_with("_test.go"))
        && super::same_package(context, consumer_file, origin)
    {
        return None;
    }
    Some(HoverLink {
        symbol: format!(
            "({}.{}).{}",
            render::package_name(workspace, origin),
            owner.name,
            symbol.name
        ),
        path: render::link_import_path(workspace, origin)?,
        anchor: format!("{}.{}", owner.name, symbol.name),
    })
}

fn type_symbol(
    context: &Context<'_>,
    origin: &Path,
    source: &str,
    symbol: &Symbol,
    range: super::SourceRange,
    declaring: bool,
) -> Option<HoverContent> {
    let workspace = context.workspace();
    let declaration = symbol.declaration()?;
    let spec = source.get(declaration.clone())?;
    if spec.len() > MAX_DECLARATION_BYTES {
        return None;
    }
    let mut type_decl = format!("type {}", canonical_declaration_text(spec));
    if declaring && let Some(layout) = super::size::type_layout(context, origin, symbol) {
        let wasted =
            super::size::struct_layout(context, origin, symbol).and_then(super::size::wasted);
        let summary = super::size::size_summary(layout, wasted);
        let line_end = type_decl.find('\n').unwrap_or(type_decl.len());
        type_decl.insert_str(line_end, &format!(" // {summary}"));
    }
    // gopls appends the aliased named type's own declaration for aliases.
    if symbol.alias
        && let Some(target) = symbol
            .type_info
            .as_ref()
            .and_then(|info| info.target.as_ref())
        && let Some(rhs) = rhs_type_declaration(context, origin, source, &target.name)
        && !rhs.alias
    {
        let text = rhs.text;
        type_decl.push_str("\n\n");
        type_decl.push_str("type ");
        type_decl.push_str(text.trim_end());
    }
    let type_decl = render::fold_array_lengths(context, origin, &type_decl);
    let mut content = HoverContent::new(range);
    content.code = Some(type_decl);
    // gopls omits the method set for interface references; declarations and
    // struct embeddings still list it.
    let header = spec.split('{').next().unwrap_or(spec);
    let interface_type = header.split_whitespace().any(|word| word == "interface");
    let interface_reference = interface_type && !declaring;
    if !interface_reference
        && let Some(promoted) = super::promote::promoted_fields(context, origin, symbol)
    {
        content.extra.push(promoted);
    }
    let methods = if symbol.alias {
        workspace
            .embedded_target(origin, &symbol.name)
            .and_then(|(file, owner)| {
                if owner.generic {
                    return None;
                }
                let source = declaration_source(context, file)?;
                method_lines(context, file, owner, source.text())
            })
    } else {
        method_lines(context, origin, symbol, source)
    };
    if let Some(methods) = methods {
        content.extra.push(methods);
    }
    content.doc = attach(
        context,
        origin,
        source,
        symbol.start,
        declaration.end,
        render::declaration_keyword(source, symbol.start),
        true,
    )
    .map(|documents| {
        content.doc_links = documents.links;
        documents.text
    });
    content.link = render::package_link(
        context,
        origin,
        &symbol.name,
        format!(
            "{}.{}",
            render::package_name(workspace, origin),
            symbol.name
        ),
    );
    Some(content)
}

/// Declaration text of a type alias' named RHS type.
fn rhs_type_declaration(
    context: &Context<'_>,
    origin: &Path,
    source: &str,
    name: &str,
) -> Option<RhsDeclaration> {
    let workspace = context.workspace();
    let base = name.split('[').next().unwrap_or(name);
    let (path, rhs) = workspace.type_declaration(origin, base)?;
    let declaration = rhs.declaration()?;
    let text = if path.as_path() == origin {
        source.get(declaration).map(str::to_owned)
    } else {
        let text = declaration_source(context, path)?;
        text.text().get(declaration).map(str::to_owned)
    }?;
    Some(RhsDeclaration {
        alias: rhs.alias,
        text,
    })
}

struct RhsDeclaration {
    alias: bool,
    text: String,
}

/// Rendered method set lines, sorted by method name like gopls' method set.
fn method_lines(
    context: &Context<'_>,
    origin: &Path,
    owner: &Symbol,
    owner_source: &str,
) -> Option<String> {
    let workspace = context.workspace();
    let consumer = context.file();
    let same_package = super::same_package(context, consumer, origin);
    // Resolve the owner in its declaration scope, but select production/test
    // methods from the actual query file's package view.
    let mut entries: Vec<(String, usize, String)> = Vec::new();
    let mut methods: Vec<(&std::path::PathBuf, &Symbol)> = workspace
        .scoped_receiver_methods(consumer, origin, &owner.name)
        .into_iter()
        .filter(|(_, method)| same_package || crate::syntax::is_exported_name(&method.name))
        .take(MAX_METHODS)
        .collect();
    methods.sort_by(|left, right| left.1.name.cmp(&right.1.name));
    let mut sources: std::collections::HashMap<std::path::PathBuf, String> =
        std::collections::HashMap::new();
    for (path, method) in methods {
        if path.as_path() != origin && !sources.contains_key(path.as_path()) {
            let Some(text) = workspace.source(path) else {
                continue;
            };
            sources.insert(path.clone(), text);
        }
        let source = if path.as_path() == origin {
            owner_source
        } else {
            sources
                .get(path.as_path())
                .map_or(owner_source, String::as_str)
        };
        if let Some(line) = method_line(context, path, source, method) {
            entries.push((method.name.clone(), 0, line));
        }
    }
    let mut visited = vec![(origin.to_path_buf(), owner.start)];
    collect_promoted_methods(
        context,
        origin,
        owner_source,
        owner,
        0,
        &mut visited,
        &mut entries,
    );
    if entries.is_empty() {
        return None;
    }
    let mut best: std::collections::HashMap<String, (usize, String)> =
        std::collections::HashMap::new();
    for (name, depth, line) in entries {
        match best.get(&name) {
            Some((existing, _)) if *existing <= depth => {}
            _ => {
                best.insert(name, (depth, line));
            }
        }
    }
    let mut names: Vec<&String> = best.keys().collect();
    names.sort();
    let lines: Vec<&str> = names
        .into_iter()
        .filter_map(|name| best.get(name).map(|(_, line)| line.as_str()))
        .take(MAX_METHODS)
        .collect();
    (!lines.is_empty()).then(|| lines.join("\n"))
}

/// Methods promoted through embedded struct/interface fields and interfaces.
fn collect_promoted_methods(
    context: &Context<'_>,
    file: &Path,
    source: &str,
    symbol: &Symbol,
    depth: usize,
    visited: &mut Vec<(std::path::PathBuf, usize)>,
    entries: &mut Vec<(String, usize, String)>,
) {
    if depth >= 8 || entries.len() > MAX_METHODS || visited.len() > 64 {
        return;
    }
    let workspace = context.workspace();
    let consumer = context.file();
    // Re-resolve by name so declaration offsets match this file's source view.
    let Some((canonical_file, canonical)) = workspace.type_declaration(file, &symbol.name) else {
        return;
    };
    let file = canonical_file.as_path();
    let symbol = canonical;
    let declaration = match symbol.declaration() {
        Some(declaration) => declaration,
        None => return,
    };
    let Some(tail) = source.get(symbol.end..declaration.end) else {
        return;
    };
    let mut body = tail.trim_start().trim_start_matches('=').trim_start();
    if body.starts_with('[')
        && let Some(offset) = crate::syntax::aggregate_offset(body)
        && let Some(rest) = body.get(offset..)
    {
        body = rest.trim_start();
    }
    if body.starts_with("interface") {
        let same_package = super::same_package(context, consumer, file);
        let Some(owner_display) = render::type_text(context, file, &symbol.name) else {
            return;
        };
        let tokens: Vec<_> = crate::syntax::Lexer::new(body).collect();
        let mut index = 1usize;
        if tokens
            .get(index)
            .is_some_and(|token| token.kind == crate::syntax::TokenKind::LBrace)
        {
            index += 1;
        }
        loop {
            while tokens
                .get(index)
                .is_some_and(|token| token.kind == crate::syntax::TokenKind::Semicolon)
            {
                index += 1;
            }
            let Some(first) = tokens.get(index) else {
                break;
            };
            if first.kind == crate::syntax::TokenKind::RBrace {
                break;
            }
            let start = index;
            let mut nesting = 0usize;
            let mut end = index;
            while index < tokens.len() {
                let kind = tokens[index].kind;
                if nesting == 0
                    && matches!(
                        kind,
                        crate::syntax::TokenKind::Semicolon | crate::syntax::TokenKind::RBrace
                    )
                {
                    end = index;
                    if kind == crate::syntax::TokenKind::Semicolon {
                        index += 1;
                    }
                    break;
                }
                match kind {
                    crate::syntax::TokenKind::LParen
                    | crate::syntax::TokenKind::LBrack
                    | crate::syntax::TokenKind::LBrace => nesting += 1,
                    crate::syntax::TokenKind::RParen
                    | crate::syntax::TokenKind::RBrack
                    | crate::syntax::TokenKind::RBrace => nesting = nesting.saturating_sub(1),
                    _ => {}
                }
                index += 1;
                end = index;
            }
            let Some(header) = tokens.get(start..end) else {
                break;
            };
            let Some(first) = header.first() else {
                continue;
            };
            if first.kind == crate::syntax::TokenKind::RBrace {
                break;
            }
            if first.kind != crate::syntax::TokenKind::Ident {
                continue;
            }
            let name = body.get(first.span.clone()).unwrap_or("");
            if header
                .get(1)
                .is_some_and(|token| token.kind == crate::syntax::TokenKind::LParen)
            {
                // gopls lists only promoted methods for interface types, not
                // the interface's own declared methods.
                if depth == 0 {
                    continue;
                }
                if !crate::syntax::is_exported_name(name) && !same_package {
                    continue;
                }
                let Some(last) = header.last() else {
                    continue;
                };
                let Some(annotation) = body.get(header[1].span.start..last.span.end) else {
                    continue;
                };
                if let Some(label) = render::interface_source_label(context, file, name, annotation)
                {
                    entries.push((
                        name.to_owned(),
                        depth,
                        format!("func ({owner_display}) {label}"),
                    ));
                }
            } else {
                let Some(last) = header.last() else {
                    continue;
                };
                let Some(text) = body.get(first.span.start..last.span.end) else {
                    continue;
                };
                let name = text.split(['[', ' ']).next().unwrap_or(text).trim();
                recurse_promoted(context, file, name, depth + 1, visited, entries);
            }
            if tokens
                .get(index)
                .is_some_and(|token| token.kind == crate::syntax::TokenKind::RBrace)
            {
                break;
            }
        }
        return;
    }
    let Some(frames) = super::size::struct_fields(context, file, symbol) else {
        return;
    };
    for frame in frames {
        if !frame.embedded {
            continue;
        }
        // Exported methods remain promoted through unexported embeddings.
        // Visibility is checked on each resulting method, not on the path.
        let annotation = frame.annotation.trim().trim_start_matches('*').trim();
        let Some(name) = annotation.split('[').next().map(str::trim) else {
            continue;
        };
        recurse_promoted(context, file, name, depth + 1, visited, entries);
    }
}

fn recurse_promoted(
    context: &Context<'_>,
    file: &Path,
    name: &str,
    depth: usize,
    visited: &mut Vec<(std::path::PathBuf, usize)>,
    entries: &mut Vec<(String, usize, String)>,
) {
    if name.is_empty() {
        return;
    }
    let workspace = context.workspace();
    let consumer = context.file();
    let Some((target_file, target)) = workspace.type_declaration(file, name) else {
        return;
    };
    if target.generic
        || visited
            .iter()
            .any(|(visited_file, start)| visited_file == target_file && *start == target.start)
    {
        return;
    }
    let same_package = super::same_package(context, consumer, target_file);
    let mut methods: Vec<(&std::path::PathBuf, &Symbol)> = workspace
        .scoped_receiver_methods(consumer, target_file, &target.name)
        .into_iter()
        .filter(|(_, method)| same_package || crate::syntax::is_exported_name(&method.name))
        .take(MAX_METHODS)
        .collect();
    methods.sort_by(|left, right| left.1.name.cmp(&right.1.name));
    for (path, method) in methods {
        let Some(source) = workspace.source(path) else {
            continue;
        };
        if let Some(line) = method_line(context, path, &source, method) {
            entries.push((method.name.clone(), depth, line));
        }
    }
    let Some(source) = render::declaration_source(context, target_file) else {
        return;
    };
    let source = source.text().to_owned();
    visited.push((target_file.clone(), target.start));
    collect_promoted_methods(
        context,
        target_file,
        &source,
        target,
        depth,
        visited,
        entries,
    );
    visited.pop();
}

fn method_line(
    context: &Context<'_>,
    origin: &Path,
    source: &str,
    method: &Symbol,
) -> Option<String> {
    let workspace = context.workspace();
    let consumer = context.file();
    let declaration = method.declaration()?;
    let end = method.body().map_or(declaration.end, |body| body.start);
    let header = source.get(declaration.start..end)?;
    if header.len() > MAX_DECLARATION_BYTES {
        return None;
    }
    let parsed = crate::syntax::parse_signature(header);
    if !parsed.diagnostics.is_empty() {
        return None;
    }
    let (signature, _) =
        workspace.declared_signature(consumer, origin, header, &parsed, &method.name)?;
    let receiver = receiver_line(context, origin, header, &parsed)?;
    Some(format!(
        "func {receiver} {}",
        render::normalize_signature(&signature.label)
    ))
}

/// Ordinal and inherited expression/type for a constant inside a grouped
/// declaration: `const ( A = iota; B; C )`.
pub(super) struct ConstGroupContext {
    pub(super) iota_index: usize,
    pub(super) last_annotation: Option<String>,
    pub(super) last_expression: Option<String>,
}

pub(super) fn const_group_context(source: &str, symbol_start: usize) -> Option<ConstGroupContext> {
    let mut keyword_end = None;
    let mut expecting_paren = false;
    let mut group = None;
    for token in crate::syntax::Lexer::new(source) {
        if token.span.start > symbol_start {
            break;
        }
        if token.inserted {
            continue;
        }
        match token.kind {
            TokenKind::Const => {
                keyword_end = Some(token.span.end);
                expecting_paren = true;
            }
            TokenKind::LParen if expecting_paren => {
                group = Some((keyword_end?, token.span.end));
                expecting_paren = false;
            }
            _ => expecting_paren = false,
        }
    }
    let (_, group_start) = group?;
    let text = source.get(group_start..)?;
    let tokens: Vec<crate::syntax::Token> = Lexer::new(text).collect();
    let mut specs: Vec<(usize, usize)> = Vec::new();
    let mut start = 0usize;
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate() {
        match token.kind {
            TokenKind::LParen | TokenKind::LBrack | TokenKind::LBrace => depth += 1,
            TokenKind::RParen | TokenKind::RBrack | TokenKind::RBrace => {
                if depth == 0 {
                    specs.push((start, index));
                    break;
                }
                depth = depth.saturating_sub(1);
            }
            TokenKind::Semicolon if depth == 0 => {
                specs.push((start, index));
                start = index + 1;
            }
            _ => {}
        }
    }
    let mut last_annotation: Option<String> = None;
    let mut last_expression: Option<String> = None;
    for (iota_index, (begin, end)) in specs.iter().enumerate() {
        let spec_tokens = tokens.get(*begin..*end)?;
        if spec_tokens.is_empty() {
            continue;
        }
        let spec_start = group_start + spec_tokens.first()?.span.start;
        let spec_end = group_start + spec_tokens.last()?.span.end;
        if spec_start <= symbol_start && symbol_start <= spec_end {
            return Some(ConstGroupContext {
                iota_index,
                last_annotation,
                last_expression,
            });
        }
        let Some(assign) = spec_tokens
            .iter()
            .position(|token| token.kind == TokenKind::Assign)
        else {
            continue;
        };
        let assign_start = group_start + spec_tokens[assign].span.start;
        let spec_text = source.get(spec_start..spec_end)?;
        let shape = value_shape(spec_text)?;
        last_annotation = shape.annotation.map(str::to_owned);
        last_expression = Some(
            spec_text
                .get(assign_start - spec_start..)?
                .trim_start_matches('=')
                .trim()
                .to_owned(),
        );
    }
    None
}

/// Declared type of a constant conversion expression such as `int64(...)`.
fn conversion_expression_type(
    context: &Context<'_>,
    origin: &Path,
    expression: &str,
) -> Option<String> {
    let parsed = crate::syntax::parse_expression(expression);
    if !parsed.diagnostics.is_empty() {
        return None;
    }
    let root = parsed.nodes.get(parsed.root.0)?;
    let crate::syntax::ExprKind::Call {
        function,
        arguments,
        ..
    } = &root.kind
    else {
        return None;
    };
    if arguments.len() != 1 {
        return None;
    }
    let callee = parsed.nodes.get(function.0)?;
    let text = match &callee.kind {
        crate::syntax::ExprKind::Name => expression.get(callee.span.clone())?,
        crate::syntax::ExprKind::Selector { operand, field } => {
            let operand = parsed.nodes.get(operand.0)?;
            if !matches!(operand.kind, crate::syntax::ExprKind::Name) {
                return None;
            }
            let alias = expression.get(operand.span.clone())?;
            let name = expression.get(field.clone())?;
            let workspace = context.workspace();
            if !workspace
                .imported_symbol_iter(origin, alias)
                .any(|(_, symbol)| symbol.is_type() && symbol.name == name)
            {
                return None;
            }
            expression.get(callee.span.clone())?
        }
        _ => return None,
    };
    let workspace = context.workspace();
    let builtin = matches!(
        text,
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
            | "rune"
            | "byte"
    );
    if !builtin {
        let name = text.split('.').next_back()?;
        let is_type = workspace
            .unqualified_symbol(origin, name)
            .is_some_and(|(_, symbol)| symbol.is_type());
        if !is_type {
            return None;
        }
    }
    workspace
        .signature_type(context.file(), origin, text)
        .map(|rendered| render::normalize_type(&rendered))
        .or_else(|| Some(render::normalize_type(text)))
}

/// Declared type of a typed constant referenced by a constant expression.
fn expression_const_type(context: &Context<'_>, origin: &Path, expression: &str) -> Option<String> {
    expression_const_type_at(context, origin, expression, 0)
}

fn expression_const_type_at(
    context: &Context<'_>,
    origin: &Path,
    expression: &str,
    depth: usize,
) -> Option<String> {
    let tokens: Vec<_> = Lexer::new(expression)
        .filter(|token| !token.inserted)
        .collect();
    for (index, token) in tokens.iter().enumerate() {
        if token.kind != TokenKind::Ident {
            continue;
        }
        let mut name = expression.get(token.span.clone())?.to_owned();
        if tokens
            .get(index + 1)
            .is_some_and(|token| token.kind == TokenKind::Period)
            && let Some(part) = tokens
                .get(index + 2)
                .filter(|token| token.kind == TokenKind::Ident)
        {
            name.push('.');
            name.push_str(expression.get(part.span.clone())?);
        }
        let workspace = context.workspace();
        let resolved = if let Some((alias, constant)) = name.split_once('.') {
            workspace
                .imported_symbol_iter(origin, alias)
                .find(|(_, symbol)| symbol.is_const() && symbol.name == constant)
                .map(|(path, symbol)| (path.clone(), symbol))
        } else {
            workspace
                .unqualified_symbol(origin, &name)
                .filter(|(_, symbol)| symbol.is_const())
                .map(|(path, symbol)| (path.clone(), symbol))
        };
        let Some((file, symbol)) = resolved else {
            continue;
        };
        let Some(source_text) = render::declaration_source(context, &file) else {
            continue;
        };
        let source = source_text.text();
        let declaration = symbol.declaration()?;
        let spec = source.get(declaration)?;
        let shape = value_shape(spec);
        let group = const_group_context(source, symbol.start);
        let (annotation, value_text) =
            if let Some(value) = shape.as_ref().and_then(|s| s.value_text) {
                (shape.as_ref().and_then(|s| s.annotation), Some(value))
            } else {
                let Some(group) = group.as_ref() else {
                    continue;
                };
                (
                    group.last_annotation.as_deref(),
                    group.last_expression.as_deref(),
                )
            };
        if let Some(annotation) = annotation {
            let consumer = context.file();
            let rendered = workspace.signature_type(consumer, &file, annotation)?;
            let consumer_package = workspace
                .files
                .get(consumer)
                .map(|stub| stub.package.as_str());
            let origin_package = workspace.files.get(&file).map(|stub| stub.package.as_str());
            if !rendered.contains('.')
                && !annotation.contains('.')
                && consumer_package != origin_package
            {
                return Some(
                    alias_rhs_text(context, &file, annotation).unwrap_or_else(|| {
                        format!("{}.{}", render::package_name(workspace, &file), rendered)
                    }),
                );
            }
            return Some(rendered);
        }
        if depth >= 16 {
            return None;
        }
        let Some(value_text) = value_text else {
            continue;
        };
        if let Some(inferred) = expression_const_type_at(context, &file, value_text, depth + 1) {
            return Some(inferred);
        }
    }
    None
}

/// go/format output for a type spec: LF endings and only attached comments.
fn canonical_declaration_text(text: &str) -> String {
    let text = text.replace("\r\n", "\n").replace('\r', "");
    let lines: Vec<&str> = text.split('\n').collect();
    let mut output: Vec<&str> = Vec::with_capacity(lines.len());
    let mut index = 0usize;
    while index < lines.len() {
        let trimmed = lines[index].trim_start();
        if trimmed.starts_with("//") {
            let mut end = index;
            while end < lines.len() && lines[end].trim_start().starts_with("//") {
                end += 1;
            }
            if !comment_attached(&lines, end) {
                index = end;
                detached_comment_gap(&mut output, &lines[end..]);
                while index < lines.len() && lines[index].trim().is_empty() {
                    index += 1;
                }
                continue;
            }
            output.extend_from_slice(&lines[index..end]);
            index = end;
            continue;
        }
        if trimmed.starts_with("/*") {
            let mut end = index;
            while end < lines.len() {
                let closes = lines[end].contains("*/");
                end += 1;
                if closes {
                    break;
                }
            }
            if !comment_attached(&lines, end) {
                index = end;
                detached_comment_gap(&mut output, &lines[end..]);
                while index < lines.len() && lines[index].trim().is_empty() {
                    index += 1;
                }
                continue;
            }
            output.extend_from_slice(&lines[index..end]);
            index = end;
            continue;
        }
        output.push(lines[index]);
        index += 1;
    }
    output.join("\n")
}

/// Maintain separation after omitting unattached comment groups.
fn detached_comment_gap(output: &mut Vec<&str>, remaining: &[&str]) {
    // Removing a trailing, unattached comment must not leave a phantom field
    // separator before the aggregate's closing brace (e.g. reflect.Value).
    if remaining
        .iter()
        .find(|line| !line.trim().is_empty())
        .is_some_and(|line| line.trim_start().starts_with('}'))
    {
        while output.last().is_some_and(|line| line.trim().is_empty()) {
            output.pop();
        }
    } else if output
        .last()
        .is_some_and(|line| !line.trim().is_empty() && !line.trim_end().ends_with('{'))
    {
        output.push("");
    }
}

/// A comment group is printed only when it documents the following node.
fn comment_attached(lines: &[&str], next: usize) -> bool {
    let Some(line) = lines.get(next) else {
        return false;
    };
    let trimmed = line.trim();
    !trimmed.is_empty() && !trimmed.starts_with('}')
}

/// Evaluate a named constant expression (possibly inherited from its group).
fn evaluate_named_constant(
    context: &Context<'_>,
    origin: &Path,
    name: &str,
    depth: usize,
) -> Option<(Option<String>, String)> {
    if depth >= 16 || name.is_empty() {
        return None;
    }
    let workspace = context.workspace();
    let resolved = if let Some((alias, constant)) = name.split_once('.') {
        workspace
            .imported_symbol_iter(origin, alias)
            .find(|(_, symbol)| symbol.is_const() && symbol.name == constant)
            .map(|(path, symbol)| (path.clone(), symbol))
    } else {
        workspace
            .unqualified_symbol(origin, name)
            .filter(|(_, symbol)| symbol.is_const())
            .map(|(path, symbol)| (path.clone(), symbol))
    };
    let (file, symbol) = resolved?;
    let source_text = render::declaration_source(context, &file)?;
    let source = source_text.text();
    let declaration = symbol.declaration()?;
    let spec = source.get(declaration.clone())?;
    let shape = value_shape(spec)?;
    let group = const_group_context(source, symbol.start);
    let (annotation, expression) = if let Some(value) = shape.value_text {
        (shape.annotation.map(str::to_owned), value.to_owned())
    } else {
        let group = group.as_ref()?;
        (
            group.last_annotation.clone(),
            group.last_expression.clone()?,
        )
    };
    let iota = group.as_ref().map_or(0, |group| group.iota_index);
    let substituted = super::value::substitute_iota(&expression, iota);
    if let Some(literal) = super::value::literal(&substituted) {
        return Some((annotation, literal.value));
    }
    if let Some(value) = super::value::integer_expression(&substituted) {
        return Some((annotation, value.to_string()));
    }
    let trimmed = substituted.trim();
    evaluate_named_constant(context, &file, trimmed, depth + 1)
        .map(|(_, value)| (annotation, value))
}

/// Resolve a constant by its unqualified or import-qualified name.
fn resolve_constant<'a>(
    workspace: &'a crate::workspace::Workspace,
    origin: &Path,
    name: &str,
) -> Option<(std::path::PathBuf, &'a Symbol)> {
    if let Some((alias, constant)) = name.split_once('.') {
        workspace
            .imported_symbol_iter(origin, alias)
            .find(|(_, symbol)| symbol.is_const() && symbol.name == constant)
            .map(|(path, symbol)| (path.clone(), symbol))
    } else {
        workspace
            .unqualified_symbol(origin, name)
            .filter(|(_, symbol)| symbol.is_const())
            .map(|(path, symbol)| (path.clone(), symbol))
    }
}

const MAX_STRING_CONSTANT_DEPTH: usize = 16;

/// Evaluate a string constant expression through its named constant
/// declarations. gopls evaluates every constant expression with go/constant;
/// string concatenation is the form hover needs here, because gopls always
/// describes a string constant by its evaluated value.
fn evaluate_string_constant(
    context: &Context<'_>,
    origin: &Path,
    expression: &str,
    depth: usize,
) -> Option<(Option<String>, Vec<u8>)> {
    if depth >= MAX_STRING_CONSTANT_DEPTH {
        return None;
    }
    super::value::string_expression(expression, depth, &mut |name, next| {
        let (file, symbol) = resolve_constant(context.workspace(), origin, name)?;
        let source_text = render::declaration_source(context, &file)?;
        let source = source_text.text();
        let declaration = symbol.declaration()?;
        let spec = source.get(declaration)?;
        let shape = value_shape(spec)?;
        let group = const_group_context(source, symbol.start);
        let (annotation, expression) = if let Some(value) = shape.value_text {
            (shape.annotation.map(str::to_owned), value.to_owned())
        } else {
            let group = group.as_ref()?;
            (
                group.last_annotation.clone(),
                group.last_expression.clone()?,
            )
        };
        let iota = group.as_ref().map_or(0, |group| group.iota_index);
        let substituted = super::value::substitute_iota(&expression, iota);
        let (nested, bytes) = evaluate_string_constant(context, &file, &substituted, next)?;
        let annotation = annotation.or(nested).map(|text| {
            context
                .workspace()
                .requalified_type(context.file(), &file, text.trim())
                .unwrap_or(text)
        });
        Some((annotation, bytes))
    })
}

/// RHS source text of an alias declaration (`type FileMode = fs.FileMode`).
fn alias_rhs_text(context: &Context<'_>, origin: &Path, name: &str) -> Option<String> {
    let workspace = context.workspace();
    let (file, symbol) = workspace.type_declaration(origin, name)?;
    if !symbol.alias {
        return None;
    }
    let source_text = render::declaration_source(context, file)?;
    let source = source_text.text();
    let declaration = symbol.declaration()?;
    let text = source.get(declaration)?;
    let equals = text.find('=')?;
    let rhs = text.get(equals + 1..)?.trim();
    (!rhs.is_empty()).then(|| rhs.to_owned())
}

fn value_symbol(
    context: &Context<'_>,
    origin: &Path,
    source: &str,
    symbol: &Symbol,
    range: super::SourceRange,
    declaring: bool,
) -> Option<HoverContent> {
    let workspace = context.workspace();
    let consumer = context.file();
    let declaration = symbol.declaration()?;
    let spec = source.get(declaration.clone())?;
    if spec.len() > MAX_DECLARATION_BYTES {
        return None;
    }
    let shape = value_shape(spec)?;
    let mut content = HoverContent::new(range);
    let qualifier = import_qualifier(context, consumer, origin)?;
    let code = match symbol.kind {
        SymbolKind::Constant => {
            let group = const_group_context(source, symbol.start);
            let (annotation, expression, own) = if let Some(value) = shape.value_text {
                (shape.annotation.map(str::to_owned), value.to_owned(), true)
            } else if let Some(group) = &group {
                (
                    group.last_annotation.clone(),
                    group.last_expression.clone()?,
                    false,
                )
            } else {
                return None;
            };
            let iota = group.as_ref().map_or(0, |group| group.iota_index);
            let substituted = super::value::substitute_iota(&expression, iota);
            let mut literal = super::value::literal(&substituted);
            let mut evaluated = None;
            let mut named_type = None;
            if literal.is_none() {
                let simple = super::value::integer_expression(&substituted);
                let resolved = if simple.is_none() {
                    super::size::const_integer(context, origin, &substituted)
                } else {
                    None
                };
                if let Some(value) = simple.or(resolved) {
                    evaluated = Some(value);
                    literal = Some(super::value::Literal {
                        value: value.to_string(),
                        declaration: own.then(|| expression.clone()),
                        untyped: Some("int"),
                    });
                } else if let Some((ty, value)) =
                    evaluate_named_constant(context, origin, substituted.trim(), 0)
                {
                    named_type = ty;
                    literal = Some(super::value::Literal {
                        value: value.clone(),
                        declaration: own.then(|| expression.clone()),
                        untyped: None,
                    });
                    evaluated = value.parse().ok();
                } else if let Some((ty, bytes)) =
                    evaluate_string_constant(context, origin, &substituted, 0)
                {
                    named_type = ty;
                    literal = Some(super::value::Literal {
                        value: super::value::quote_bytes(&bytes),
                        declaration: own.then(|| expression.clone()),
                        untyped: Some("string"),
                    });
                }
            }
            let mut literal = literal?;
            if own {
                if literal.declaration.is_none() {
                    literal.declaration = Some(expression.trim().to_owned());
                }
            } else {
                literal.declaration = None;
            }
            let ty = match annotation {
                Some(annotation) => match render::alias_text(&annotation) {
                    Some(alias) => alias.to_owned(),
                    None => {
                        let rendered = workspace.signature_type(consumer, origin, &annotation)?;
                        if annotation.contains('.') && !rendered.contains('.') {
                            annotation.trim().to_owned()
                        } else if !rendered.contains('.')
                            && !annotation.contains('.')
                            && workspace
                                .files
                                .get(consumer)
                                .zip(workspace.files.get(origin))
                                .is_some_and(|(a, b)| a.package != b.package)
                        {
                            alias_rhs_text(context, origin, &annotation).unwrap_or_else(|| {
                                format!("{}.{}", render::package_name(workspace, origin), rendered)
                            })
                        } else {
                            rendered
                        }
                    }
                },
                None => named_type
                    .or_else(|| conversion_expression_type(context, origin, &expression))
                    .or_else(|| expression_const_type(context, origin, &expression))
                    .unwrap_or_else(|| format!("untyped {}", literal.untyped.unwrap_or("int"))),
            };
            let ty = render::normalize_type(&ty);
            let assigned = match (evaluated, ty == "time.Duration") {
                (Some(value), true) => {
                    let duration = super::value::duration_string(value);
                    match &literal.declaration {
                        Some(declaration) => format!("{declaration} // {duration}"),
                        None => duration,
                    }
                }
                _ => literal.assigned(),
            };
            let _ = declaring;
            format!("const {qualifier}{} {ty} = {assigned}", symbol.name)
        }
        _ => {
            if let Some(annotation) = shape.annotation {
                let ty = match render::alias_text(annotation) {
                    Some(alias) => alias.to_owned(),
                    None => {
                        let rendered = workspace.signature_type(consumer, origin, annotation)?;
                        if annotation.contains('.') && !rendered.contains('.') {
                            annotation.trim().to_owned()
                        } else {
                            render::normalize_type(&rendered)
                        }
                    }
                };
                format!("var {qualifier}{} {ty}", symbol.name)
            } else {
                let inferred = shape.value_text.and_then(|text| {
                    crate::analysis::check::inferred_array_literal(workspace, origin, text)
                });
                let ty = if let Some(inferred) = inferred {
                    render::normalize_type(&inferred?.text)
                } else {
                    shape
                        .value_text
                        .and_then(|text| {
                            render::call_result_type(context, origin, text, 0, 1, true)
                        })
                        .or_else(|| {
                            shape
                                .value_text
                                .and_then(|text| render::conversion_type(context, origin, text))
                        })
                        .or_else(|| shape.value_text.and_then(render::function_literal_text))
                        .or_else(|| {
                            shape
                                .value_text
                                .and_then(|text| render::function_value_type(context, origin, text))
                        })
                        .or_else(|| {
                            shape
                                .value_text
                                .and_then(super::value::default_type)
                                .map(|text| render::normalize_type(&text))
                        })?
                };
                format!("var {qualifier}{} {ty}", symbol.name)
            }
        }
    };
    let code = render::fold_array_lengths(context, origin, &code);
    content.code = Some(code);
    content.doc = attach(
        context,
        origin,
        source,
        symbol.start,
        declaration.end,
        render::declaration_keyword(source, symbol.start),
        true,
    )
    .map(|documents| {
        content.doc_links = documents.links;
        documents.text
    });
    content.link = render::package_link(
        context,
        origin,
        &symbol.name,
        format!(
            "{}.{}",
            render::package_name(workspace, origin),
            symbol.name
        ),
    );
    Some(content)
}

struct ValueShape<'a> {
    annotation: Option<&'a str>,
    value_text: Option<&'a str>,
}

/// Split one value spec into its optional written type and first initializer.
fn value_shape(spec: &str) -> Option<ValueShape<'_>> {
    let tokens: Vec<_> = Lexer::new(spec).collect();
    let mut index = 0;
    if tokens
        .get(index)
        .is_some_and(|token| token.kind == TokenKind::Ident)
    {
        index += 1;
        while tokens
            .get(index)
            .is_some_and(|token| token.kind == TokenKind::Comma)
            && tokens
                .get(index + 1)
                .is_some_and(|token| token.kind == TokenKind::Ident)
        {
            index += 2;
        }
    }
    let rest_start = tokens.get(index)?.span.start;
    for token in tokens.iter().skip(index) {
        if token.kind != TokenKind::Assign {
            continue;
        }
        let values = spec.get(token.span.end..)?;
        return Some(ValueShape {
            annotation: spec
                .get(rest_start..token.span.start)
                .map(str::trim)
                .filter(|text| !text.is_empty()),
            value_text: first_value(values),
        });
    }
    Some(ValueShape {
        annotation: spec
            .get(rest_start..)
            .map(str::trim)
            .filter(|text| !text.is_empty()),
        value_text: None,
    })
}

/// First comma-separated initializer after `=`.
fn first_value(text: &str) -> Option<&str> {
    let mut depth = 0usize;
    for token in Lexer::new(text) {
        match token.kind {
            TokenKind::LParen | TokenKind::LBrack | TokenKind::LBrace => depth += 1,
            TokenKind::RParen | TokenKind::RBrack | TokenKind::RBrace => {
                depth = depth.saturating_sub(1);
            }
            TokenKind::Comma if depth == 0 => return text.get(..token.span.start).map(str::trim),
            _ => {}
        }
    }
    let trimmed = text.trim();
    (!trimmed.is_empty()).then_some(trimmed)
}

fn attach(
    context: &Context<'_>,
    origin: &Path,
    source: &str,
    start: usize,
    end: usize,
    declaration: Option<usize>,
    trailing: bool,
) -> Option<doc::Documents> {
    doc::attached(
        context,
        origin,
        source,
        doc::Target {
            start,
            end,
            declaration,
            trailing,
        },
    )
}
