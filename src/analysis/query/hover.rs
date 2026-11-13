//! Hover semantics: resolve the identifier under the cursor and describe it
//! using request-local metadata, source views and existing native inference.
mod builtin;
mod content;
mod doc;
mod package;
mod promote;
mod render;
mod size;
mod symbol;
mod value;
mod version;
mod view;

pub(crate) use content::{HoverContent, HoverLink};

use super::{Context, SourceRange};
use crate::analysis::check::navigation::Target;
use crate::syntax::{Lexer, TokenKind};
use crate::text::SourceMap;
use std::ops::Range;
use std::path::Path;

/// Resolve the identifier under the cursor into hover facts.
pub(crate) fn execute(
    context: &Context<'_>,
    offset: usize,
) -> Result<Option<HoverContent>, (i32, String)> {
    let source = context.source();
    let file = context.file();
    if detached_with_custom_tags(context.workspace(), file, source) {
        // gopls cannot load a package for an orphaned custom-tag file.
        return Err((
            0,
            format!(
                "no package metadata for file {}",
                crate::workspace::uri(file)
            ),
        ));
    }
    let Some(span) = identifier_at(source, offset) else {
        return Ok(None);
    };
    let map = SourceMap::new(source);
    let name = &source[span.clone()];
    let scope = match context.cursor_scope(offset) {
        Ok(scope) => scope,
        Err(_) => {
            if let Ok(navigation) = context.navigation_target(offset) {
                match navigation.target {
                    Target::Member(member) => {
                        let content = render::member(
                            context,
                            member,
                            None,
                            &span,
                            super::SourceRange::new(&map, span.clone()),
                        );
                        if !content.is_empty() {
                            return Ok(Some(content));
                        }
                    }
                    Target::Global(path, symbol) => {
                        if let Ok(Some(content)) = symbol::reference(
                            context,
                            path,
                            symbol,
                            super::SourceRange::new(&map, span.clone()),
                        ) {
                            return Ok(Some(content));
                        }
                    }
                    _ => {}
                }
            }
            if let Some(code) = render::anonymous_field_near(context, offset, name) {
                let mut content = HoverContent::new(super::SourceRange::new(&map, span.clone()));
                content.code = Some(code);
                return Ok(Some(content));
            }
            if let Some(code) = on_disk_type(context, name) {
                let mut content = HoverContent::new(super::SourceRange::new(&map, span.clone()));
                content.code = Some(code);
                return Ok(Some(content));
            }
            return Ok(None);
        }
    };
    let range = SourceRange::new(&map, span.clone());
    // A declaration name always takes precedence over a use of the same name.
    if let Some(symbol) = context.workspace().files.get(file).and_then(|stub| {
        stub.symbols
            .iter()
            .find(|symbol| symbol.start == span.start && symbol.end == span.end)
    }) {
        return symbol::declare(context, file, symbol, range);
    }
    if let Some(binding) = scope
        .locals
        .definition_at(&span, name)
        .filter(|binding| binding.selection == span)
        .or_else(|| {
            scope
                .locals
                .visible_bindings(offset)
                .find(|binding| binding.selection == span)
        })
    {
        return Ok(Some(render::local(context, &scope.locals, binding, range)));
    }
    if let Some(hover) = render::header_binding(context, &span, range)? {
        return Ok(Some(hover));
    }
    if is_package_clause_name(source, span.start) {
        return Ok(package::clause(context, range));
    }
    if let Some(hover) = package::import_alias(context, &span, range) {
        return Ok(Some(hover));
    }
    let navigation = match context.navigation_target(offset) {
        Ok(navigation) => navigation,
        Err(_) => return Ok(None),
    };
    match navigation.target {
        Target::Global(path, symbol) => {
            let mut content = symbol::reference(context, path, symbol, range)?;
            if let Some(content_ref) = content.as_mut()
                && let Some(code) = content_ref.code.as_deref()
                && let Some(instantiated) =
                    instantiated_generic(context, &scope, &span, path, symbol, code)
            {
                content_ref.code = Some(instantiated);
            }
            if symbol.is_method()
                && let Some(content_ref) = content.as_mut()
                && let Some(promoted) = promoted_method(context, &scope, &span, path, symbol)
            {
                if let Some(code) = content_ref.code.as_mut() {
                    code.push_str(&promoted.suffix);
                }
                if content_ref.link.is_none()
                    && let Some(link) = promoted.outer_link(context)
                {
                    content_ref.link = Some(link);
                }
            }
            if content.is_none()
                && let Some(hover) = variant_value_hover(context, name, &span, range)
            {
                return Ok(Some(hover));
            }
            return Ok(content);
        }
        Target::Member(member) => {
            let receiver = member_receiver(context, &scope, &span);
            let receiver = receiver
                .as_ref()
                .map(|(path, text)| (path.as_path(), text.as_str()));
            let content = render::member(context, member, receiver, &span, range);
            if content.code.is_none()
                && let Some(hover) = selector_fallback(context, offset, &scope, &span, range)
            {
                return Ok(Some(hover));
            }
            return Ok(Some(content));
        }
        Target::Local(selection) => {
            if let Some(binding) = scope
                .locals
                .visible_bindings(offset)
                .find(|binding| binding.selection == selection)
            {
                let hover = render::local(context, &scope.locals, binding, range);
                return Ok(Some(hover));
            }
            if let Some(hover) = render::nested_function_binding(context, offset, &selection, range)
            {
                return Ok(Some(hover));
            }
        }
        Target::Opaque | Target::Unresolved => {
            if let Some(hover) = selector_fallback(context, offset, &scope, &span, range) {
                return Ok(Some(hover));
            }
        }
    }
    if let Some(hover) = package_initializer_target(context, offset, &span, range)? {
        return Ok(Some(hover));
    }
    if let Some(hover) = package::qualifier(context, &span, range) {
        return Ok(Some(hover));
    }
    if let Some(hover) = builtin::hover(context, name, range) {
        return Ok(Some(hover));
    }
    // Method value of the builtin `error` interface: gopls special-cases Error.
    if name == "Error"
        && source
            .get(..span.start)
            .is_some_and(|prefix| prefix.trim_end().ends_with('.'))
    {
        let mut content = HoverContent::new(range);
        content.code = Some("func (error).Error() string".to_owned());
        return Ok(Some(content));
    }
    if let Some(hover) = render::nested_function_local(context, offset, name, range) {
        return Ok(Some(hover));
    }
    if let Some(hover) = anonymous_struct_field(context, offset, &span, range) {
        return Ok(Some(hover));
    }
    if let Some(code) = render::anonymous_field_near(context, offset, name) {
        let mut content = HoverContent::new(range);
        content.code = Some(code);
        return Ok(Some(content));
    }
    if let Some(code) = on_disk_type(context, name) {
        let mut content = HoverContent::new(range);
        content.code = Some(code);
        return Ok(Some(content));
    }
    if let Some(hover) = variant_value_hover(context, name, &span, range) {
        return Ok(Some(hover));
    }
    Ok(None)
}

const MAX_VARIANT_BYTES: usize = 4 * 1024 * 1024;

/// A type declaration from a same-directory file that the workspace did not
/// admit (build-excluded files answer from the host view in gopls).
fn on_disk_type(context: &Context<'_>, name: &str) -> Option<String> {
    let source = context.source();
    let directory = context.file().parent()?;
    let package = crate::syntax::stub(source).package;
    let mut files: Vec<_> = std::fs::read_dir(directory)
        .ok()?
        .take(256)
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "go"))
        .collect();
    files.sort();
    let mut total = 0usize;
    for path in files.into_iter().take(64) {
        let text = context
            .workspace()
            .source(&path)
            .or_else(|| std::fs::read_to_string(&path).ok())?;
        total = total.saturating_add(text.len());
        if total > MAX_VARIANT_BYTES {
            break;
        }
        let stub = crate::syntax::stub(&text);
        if stub.package != package {
            continue;
        }
        let Some(symbol) = stub
            .symbols
            .iter()
            .find(|symbol| symbol.name == name && symbol.kind == crate::syntax::SymbolKind::Type)
        else {
            continue;
        };
        let declaration = text.get(symbol.range.clone())?;
        return Some(format!("type {}", declaration.trim_end()));
    }
    None
}

/// A var/const declaration visible only in a detached document's build view.
fn variant_value_hover(
    context: &Context<'_>,
    name: &str,
    span: &Range<usize>,
    range: super::SourceRange,
) -> Option<HoverContent> {
    let workspace = context.workspace();
    let file = context.file();
    let target = workspace.orphan_view_target(file);
    let source = context.source();
    let alias = {
        let prefix = source.get(..span.start)?.trim_end().strip_suffix('.')?;
        let start = prefix
            .char_indices()
            .rev()
            .find(|(_, ch)| !(ch.is_alphanumeric() || *ch == '_'))
            .map_or(0, |(offset, ch)| offset + ch.len_utf8());
        Some(&prefix[start..])
    };
    let (directory, package, import_path) = match alias {
        Some(alias) if !alias.is_empty() => {
            let import = workspace.files.get(file)?.imports.iter().find(|import| {
                !import.is_dot()
                    && !import.is_blank()
                    && workspace.import_name(import) == Some(alias)
            })?;
            let directory = view::import_directory(workspace, &import.path)?;
            let package = workspace
                .declared_package_name(&import.path)
                .map(str::to_owned)
                .unwrap_or_else(|| alias.to_owned());
            (directory, package, Some(import.path.clone()))
        }
        _ => (
            file.parent()?.to_path_buf(),
            workspace.files.get(file)?.package.clone(),
            None,
        ),
    };
    let test_export = target.is_none()
        && file
            .file_name()
            .is_some_and(|name| name.to_string_lossy().ends_with("_test.go"))
        && import_path.is_some()
        && directory == *file.parent()?;
    if target.is_none() && !test_export {
        return None;
    }
    let location = view::package_symbol(
        workspace,
        target.as_ref(),
        &directory,
        &package,
        name,
        target.is_none(),
    )
    .or_else(|| {
        target
            .as_ref()
            .and_then(|_| view::package_symbol(workspace, None, &directory, &package, name, false))
    })?;
    let source = workspace
        .source(&location.path)
        .or_else(|| std::fs::read_to_string(&location.path).ok())?;
    if source.len() > MAX_VARIANT_BYTES {
        return None;
    }
    let stub = crate::syntax::stub(&source);
    let symbol = stub
        .symbols
        .iter()
        .find(|symbol| symbol.selection.start == location.offset && symbol.receiver.is_none())?;
    let declaration = source.get(symbol.range.clone())?.trim();
    let declaration = declaration
        .strip_prefix("var ")
        .or_else(|| declaration.strip_prefix("const "))
        .unwrap_or(declaration);
    let rest = declaration.strip_prefix(name)?;
    let rest = rest.trim_start();
    let code = match symbol.kind {
        crate::syntax::SymbolKind::Type => format!("type {name} {}", rest.trim()),
        crate::syntax::SymbolKind::Variable => {
            if let Some(expression) = rest.strip_prefix('=') {
                let expression = expression.trim();
                let operand = expression.strip_prefix('&')?;
                let inner = view::package_symbol(
                    workspace,
                    target.as_ref(),
                    &directory,
                    &package,
                    operand,
                    true,
                )?;
                let inner_source = workspace
                    .source(&inner.path)
                    .or_else(|| std::fs::read_to_string(&inner.path).ok())?;
                let inner_stub = crate::syntax::stub(&inner_source);
                let inner_symbol = inner_stub
                    .symbols
                    .iter()
                    .find(|symbol| symbol.selection.start == inner.offset)?;
                let inner_text = inner_source.get(inner_symbol.range.clone())?;
                let inner_text = inner_text
                    .trim()
                    .strip_prefix("var ")
                    .unwrap_or(inner_text.trim())
                    .strip_prefix(operand)?
                    .trim();
                if inner_text.is_empty() {
                    return None;
                }
                format!("var {package}.{name} *{inner_text}")
            } else {
                format!("var {package}.{name} {rest}")
            }
        }
        crate::syntax::SymbolKind::Constant => {
            let (annotation, expression) = rest.split_once('=')?;
            let expression = expression.trim();
            let literal = value::literal(expression)?;
            let annotation = annotation.trim();
            let ty = if annotation.is_empty() {
                format!("untyped {}", literal.untyped?)
            } else {
                annotation.to_owned()
            };
            format!(
                "const {package}.{name} {ty} = {}",
                if expression == literal.value {
                    literal.value
                } else {
                    format!("{expression} // {}", literal.value)
                }
            )
        }
        _ => return None,
    };
    let _ = import_path;
    let mut content = HoverContent::new(range);
    content.code = Some(code);
    content.link = render::package_link(context, &location.path, name, format!("{package}.{name}"));
    Some(content)
}

/// Field declaration inside an anonymous `struct { ... }` type expression.
type AnonymousStructCandidate = (usize, usize, Option<(usize, usize)>);

fn anonymous_struct_field(
    context: &Context<'_>,
    offset: usize,
    span: &Range<usize>,
    range: super::SourceRange,
) -> Option<HoverContent> {
    let source = context.source();
    let file = context.file();
    let tokens: Vec<_> = Lexer::new(source).filter(|token| !token.inserted).collect();
    let mut candidate: Option<AnonymousStructCandidate> = None;
    for (index, token) in tokens.iter().enumerate() {
        if token.span.start >= offset {
            break;
        }
        if token.kind != TokenKind::Struct {
            continue;
        }
        let Some(brace) = tokens
            .get(index + 1)
            .filter(|token| token.kind == TokenKind::LBrace)
        else {
            continue;
        };
        let mut depth = 0usize;
        let mut type_end = None;
        let mut close_index = None;
        for (position, close) in tokens.iter().enumerate().skip(index + 1) {
            match close.kind {
                TokenKind::LBrace => depth += 1,
                TokenKind::RBrace => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        type_end = Some(close.span.end);
                        close_index = Some(position);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(type_end) = type_end else {
            continue;
        };
        let _ = brace;
        // A composite literal of this anonymous type: `[]struct{...}{...}`.
        let mut literal = None;
        if let Some(position) = close_index
            && tokens
                .get(position + 1)
                .is_some_and(|token| token.kind == TokenKind::LBrace)
        {
            let mut depth = 0usize;
            for close in tokens.iter().skip(position + 1) {
                match close.kind {
                    TokenKind::LBrace => depth += 1,
                    TokenKind::RBrace => {
                        depth = depth.saturating_sub(1);
                        if depth == 0 {
                            if token.span.start < offset && offset < close.span.end {
                                literal = Some((tokens[position + 1].span.start, close.span.end));
                            }
                            break;
                        }
                    }
                    _ => {}
                }
            }
        }
        let contained = (token.span.start < offset && offset < type_end) || literal.is_some();
        if contained && candidate.is_none_or(|(start, _, _)| token.span.start > start) {
            candidate = Some((token.span.start, type_end, literal));
        }
    }
    let (start, type_end, _literal) = candidate?;
    let text = source.get(start..type_end)?;
    let entries = crate::syntax::aggregate_members(text);
    let entry = if span.end <= type_end {
        entries.iter().find(|entry| {
            start + entry.selection.start == span.start && start + entry.selection.end == span.end
        })?
    } else {
        let name = &source[span.clone()];
        entries
            .iter()
            .find(|entry| text.get(entry.selection.clone()) == Some(name))?
    };
    let annotation = text.get(entry.annotation.clone())?;
    let declaring =
        start + entry.selection.start == span.start && start + entry.selection.end == span.end;
    let workspace = context.workspace();
    let mut ty = render::normalize_type(&workspace.signature_type(file, file, annotation)?);
    ty = render::replace_invalid_qualified(workspace, file, &ty);
    let mut content = HoverContent::new(range);
    let mut code = format!("field {} {ty}", &source[span.clone()]);
    if declaring
        && let Some(layout) = size::struct_text_layout(context, file, text)
        && let Some(field) = layout
            .fields
            .iter()
            .find(|field| field.name.as_str() == &source[span.clone()])
    {
        let mut summary = format!("size={}", size::number(field.size));
        if let Some(class) = size::size_class(field.size).filter(|class| *class > field.size) {
            summary.push_str(&format!(", class={}", size::number(class)));
        }
        summary.push_str(&format!(", offset={}", size::number(field.offset)));
        code.push_str(" // ");
        code.push_str(&summary);
    }
    content.code = Some(code);
    let field_start = render::field_start(text, entry.selection.start).map_or(
        render::line_start_token(source, start + entry.selection.start),
        |offset| start + offset,
    );
    if let Some(documents) = doc::attached(
        context,
        file,
        source,
        doc::Target {
            start: field_start,
            end: start + entry.annotation.end,
            declaration: None,
            trailing: true,
        },
    ) {
        content.doc = Some(documents.text);
        content.doc_links = documents.links;
    }
    Some(content)
}

/// References inside package-level value initializers: gopls resolves them with
/// the file's package scope, while the body-only navigation path does not.
fn package_initializer_target(
    context: &Context<'_>,
    offset: usize,
    span: &Range<usize>,
    range: super::SourceRange,
) -> Result<Option<HoverContent>, (i32, String)> {
    let workspace = context.workspace();
    let file = context.file();
    let source = context.source();
    let Some(stub) = workspace.files.get(file) else {
        return Ok(None);
    };
    for symbol in &stub.symbols {
        if !symbol.is_value() || offset <= symbol.end {
            continue;
        }
        let Some(declaration) = symbol.declaration() else {
            continue;
        };
        if !(declaration.start <= offset && offset <= declaration.end) {
            continue;
        }
        let Some(text) = source.get(declaration.start..declaration.end) else {
            continue;
        };
        let Some(assign) = Lexer::new(text).find(|token| token.kind == TokenKind::Assign) else {
            continue;
        };
        let base = declaration.start + assign.span.end;
        let Some(expression_text) = source.get(base..declaration.end) else {
            continue;
        };
        let parsed = crate::syntax::parse_expression(expression_text);
        if !parsed.diagnostics.is_empty() {
            continue;
        }
        for (index, expression) in parsed.nodes.iter().enumerate() {
            let selected = match &expression.kind {
                crate::syntax::ExprKind::Name => expression.span.clone(),
                crate::syntax::ExprKind::Selector { field, .. } => field.clone(),
                _ => continue,
            };
            if (base + selected.start..base + selected.end) != *span {
                continue;
            }
            let locals = crate::analysis::scope::Locals::empty();
            let inference = crate::analysis::check::Inference::new(
                workspace,
                file,
                source,
                &locals,
                &parsed.nodes,
                base,
            );
            let Some(selection) = inference.selection(crate::syntax::ExprId(index)) else {
                return Ok(None);
            };
            return Ok(Some(match selection {
                crate::analysis::check::Selection::Global(path, symbol) => {
                    let Some(content) = symbol::reference(context, path, symbol, range)? else {
                        return Ok(None);
                    };
                    content
                }
                crate::analysis::check::Selection::Member(member) => {
                    render::member(context, member, None, span, range)
                }
            }));
        }
    }
    Ok(None)
}

const MAX_GENERIC_HEADER: usize = 16 * 1024;

/// gopls' call-site instantiation note: the substituted signature followed by
/// the generic declaration signature as a comment.
fn instantiated_generic(
    context: &Context<'_>,
    scope: &crate::analysis::scope::CursorScope,
    span: &Range<usize>,
    path: &Path,
    symbol: &crate::index::IndexedSymbol,
    rendered: &str,
) -> Option<String> {
    if !symbol.generic || !symbol.is_callable() {
        return None;
    }
    let body = scope.body.as_ref()?;
    let expressions = &body.tree.expressions;
    let base = body.base;
    let source = context.source();
    let (call_index, function_id) = expressions.iter().enumerate().find_map(|(index, entry)| {
        let crate::syntax::ExprKind::Call { function, .. } = &entry.kind else {
            return None;
        };
        let callee = &expressions[function.0];
        let selected = match &callee.kind {
            crate::syntax::ExprKind::Name => callee.span.clone(),
            crate::syntax::ExprKind::Index { operand, .. } => expressions[operand.0].span.clone(),
            _ => return None,
        };
        ((base + selected.start..base + selected.end) == *span).then_some((index, function.0))
    })?;
    let crate::syntax::ExprKind::Call { arguments, .. } = &expressions[call_index].kind else {
        return None;
    };
    let call_arguments = arguments.clone();
    let explicit: Option<Vec<String>> = match &expressions[function_id].kind {
        crate::syntax::ExprKind::Index { arguments, .. } => Some(
            arguments
                .iter()
                .filter_map(|argument| {
                    let expression = expressions.get(argument.0)?;
                    source
                        .get(base + expression.span.start..base + expression.span.end)
                        .map(|text| text.trim().to_owned())
                })
                .collect(),
        ),
        _ => None,
    };
    let source_text = render::declaration_source(context, path)?;
    let declaration_source = source_text.text();
    let declaration = symbol.declaration()?;
    let end = symbol.body().map_or(declaration.end, |body| body.start);
    let header = declaration_source.get(declaration.start..end)?;
    if header.len() > MAX_GENERIC_HEADER {
        return None;
    }
    let signature = crate::syntax::parse_signature(header);
    if !signature.diagnostics.is_empty() || signature.type_parameters.is_empty() {
        return None;
    }
    let names: Vec<&str> = signature
        .type_parameters
        .iter()
        .filter_map(|parameter| parameter.name.as_ref())
        .filter_map(|name| header.get(name.clone()))
        .map(str::trim)
        .collect();
    if names.is_empty() {
        return None;
    }
    let mut substitutions: std::collections::HashMap<&str, String> =
        std::collections::HashMap::new();
    if let Some(explicit) = explicit.filter(|explicit| explicit.len() == names.len()) {
        for (name, argument) in names.iter().zip(explicit) {
            substitutions.insert(name, argument);
        }
    } else {
        for (index, parameter) in signature.parameters.iter().enumerate() {
            let Some(argument) = call_arguments.get(index) else {
                break;
            };
            let Some(annotation) = header.get(parameter.annotation.clone()) else {
                continue;
            };
            let Some(actual) = argument_type(context, scope, expressions, base, *argument) else {
                continue;
            };
            if !unify_type(annotation, &actual, &names, &mut substitutions) {
                return None;
            }
        }
    }
    if substitutions.len() != names.len() {
        return None;
    }
    let mut code = rendered.to_owned();
    for (name, value) in &substitutions {
        code = replace_identifier(&code, name, value);
    }
    code = strip_type_parameter_list(&code);
    let comment = generic_signature_comment(header, &signature)?;
    let separator = if code.len() > 60 { "\n" } else { " " };
    Some(format!("{code}{separator}// {comment}"))
}

/// Consumer type of a call argument, for generic parameter inference.
fn argument_type(
    context: &Context<'_>,
    scope: &crate::analysis::scope::CursorScope,
    expressions: &[crate::syntax::Expr],
    base: usize,
    id: crate::syntax::ExprId,
) -> Option<String> {
    let expression = expressions.get(id.0)?;
    let source = context.source();
    let text = source.get(base + expression.span.start..base + expression.span.end)?;
    let workspace = context.workspace();
    match &expression.kind {
        crate::syntax::ExprKind::Name => {
            let binding = scope
                .locals
                .resolve_at(base + expression.span.start, text)?;
            let annotation = binding.declared_type.clone().or_else(|| {
                binding
                    .annotation()
                    .and_then(|span| source.get(span.clone()).map(str::to_owned))
            })?;
            workspace.signature_type(context.file(), context.file(), &annotation)
        }
        crate::syntax::ExprKind::Literal => value::default_type(text),
        crate::syntax::ExprKind::Composite {
            literal_type: Some(literal_type),
            ..
        } => {
            let literal = expressions.get(literal_type.0)?;
            source
                .get(base + literal.span.start..base + literal.span.end)
                .map(str::trim)
                .map(str::to_owned)
        }
        crate::syntax::ExprKind::Call { function, .. } => {
            if let Some(result) =
                render::call_result_type(context, context.file(), text, 0, 1, false)
            {
                return Some(result);
            }
            let inference = crate::analysis::check::Inference::new(
                context.workspace(),
                context.file(),
                source,
                &scope.locals,
                expressions,
                base,
            );
            let crate::analysis::check::Selection::Global(path, symbol) =
                inference.selection(*function)?
            else {
                return None;
            };
            if !symbol.is_callable() {
                return None;
            }
            let source_text = render::declaration_source(context, path)?;
            let declaration = symbol.declaration()?;
            let end = symbol.body().map_or(declaration.end, |body| body.start);
            let header = source_text.text().get(declaration.start..end)?;
            let signature = crate::syntax::parse_signature(header);
            let result = signature.results.first()?;
            let annotation = header.get(result.annotation.clone())?;
            context
                .workspace()
                .signature_type(context.file(), path, annotation)
                .map(|text| render::normalize_type(&text))
        }
        crate::syntax::ExprKind::Unary { operator, operand } => {
            let operator = source.get(base + operator.start..base + operator.end)?;
            let inner = argument_type(context, scope, expressions, base, *operand)?;
            (operator == "&").then(|| format!("*{inner}"))
        }
        _ => None,
    }
}

/// Structural single-type-parameter unification for common container shapes.
fn unify_type<'a>(
    annotation: &str,
    actual: &str,
    names: &[&'a str],
    substitutions: &mut std::collections::HashMap<&'a str, String>,
) -> bool {
    let annotation = annotation.trim();
    let actual = actual.trim();
    if !names
        .iter()
        .any(|name| contains_identifier(annotation, name))
    {
        // Parameters independent of the type parameter list never constrain it.
        return true;
    }
    if let Some(name) = names.iter().find(|name| **name == annotation) {
        if let Some(existing) = substitutions.get(name) {
            return existing == actual;
        }
        substitutions.insert(name, actual.to_owned());
        return true;
    }
    if let Some(rest) = annotation.strip_prefix("[]")
        && let Some(actual) = actual.strip_prefix("[]")
    {
        return unify_type(rest, actual, names, substitutions);
    }
    if let Some(rest) = annotation.strip_prefix('*')
        && let Some(actual) = actual.strip_prefix('*')
    {
        return unify_type(rest, actual, names, substitutions);
    }
    if let Some(rest) = annotation.strip_prefix("...")
        && let Some(actual) = actual.strip_prefix("[]")
    {
        return unify_type(rest, actual, names, substitutions);
    }
    false
}

/// Whether `text` contains `name` as a whole identifier.
fn contains_identifier(text: &str, name: &str) -> bool {
    let bytes = text.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        let boundary =
            index == 0 || !(bytes[index - 1].is_ascii_alphanumeric() || bytes[index - 1] == b'_');
        if boundary
            && text[index..].starts_with(name)
            && bytes
                .get(index + name.len())
                .is_none_or(|byte| !byte.is_ascii_alphanumeric() && *byte != b'_')
        {
            return true;
        }
        index += 1;
    }
    false
}

/// Remove the declared type-parameter list from an instantiated signature.
fn strip_type_parameter_list(code: &str) -> String {
    let Some(open) = code.find('[') else {
        return code.to_owned();
    };
    let Some(params) = code.find('(') else {
        return code.to_owned();
    };
    if open > params {
        return code.to_owned();
    }
    let mut depth = 0usize;
    for (offset, ch) in code[open..].char_indices() {
        match ch {
            '[' => depth += 1,
            ']' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    let mut output = String::with_capacity(code.len());
                    output.push_str(&code[..open]);
                    output.push_str(&code[open + offset + 1..]);
                    return output;
                }
            }
            _ => {}
        }
    }
    code.to_owned()
}

/// Replace whole identifier occurrences of `name` with `value`.
fn replace_identifier(text: &str, name: &str, value: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        let boundary =
            index == 0 || !(bytes[index - 1].is_ascii_alphanumeric() || bytes[index - 1] == b'_');
        if boundary
            && text[index..].starts_with(name)
            && bytes
                .get(index + name.len())
                .is_none_or(|byte| !byte.is_ascii_alphanumeric() && *byte != b'_')
        {
            output.push_str(value);
            index += name.len();
            continue;
        }
        let ch = text[index..].chars().next().unwrap_or('\0');
        output.push(ch);
        index += ch.len_utf8();
    }
    output
}

/// The generic declaration rendered as gopls' trailing comment.
fn generic_signature_comment(header: &str, signature: &crate::syntax::Signature) -> Option<String> {
    let mut type_parameters = Vec::new();
    for parameter in &signature.type_parameters {
        let name = header.get(parameter.name.as_ref()?.clone())?.trim();
        let constraint = header.get(parameter.annotation.clone())?.trim();
        type_parameters.push(format!("{name} {constraint}"));
    }
    let parameters = parameter_list(header, &signature.parameters)?;
    let results = result_list(header, &signature.results)?;
    let mut comment = format!("func[{}]({parameters})", type_parameters.join(", "));
    if !results.is_empty() {
        comment.push(' ');
        comment.push_str(&results);
    }
    Some(comment)
}

fn parameter_list(header: &str, parameters: &[crate::syntax::Parameter]) -> Option<String> {
    let mut parts = Vec::with_capacity(parameters.len());
    for parameter in parameters {
        let annotation = header.get(parameter.annotation.clone())?.trim();
        let annotation = if parameter.variadic {
            format!("...{annotation}")
        } else {
            annotation.to_owned()
        };
        match parameter
            .name
            .as_ref()
            .and_then(|name| header.get(name.clone()))
        {
            Some(name) => parts.push(format!("{} {annotation}", name.trim())),
            None => parts.push(annotation),
        }
    }
    Some(parts.join(", "))
}

fn result_list(header: &str, results: &[crate::syntax::Parameter]) -> Option<String> {
    if results.is_empty() {
        return Some(String::new());
    }
    if results.len() == 1 && results[0].name.is_none() {
        return Some(header.get(results[0].annotation.clone())?.trim().to_owned());
    }
    Some(format!("({})", parameter_list(header, results)?))
}

/// Resolve a selector whose navigation target inference failed by typing the
/// receiver with the request-local binding facts, then looking up the member.
fn selector_fallback(
    context: &Context<'_>,
    offset: usize,
    scope: &crate::analysis::scope::CursorScope,
    span: &Range<usize>,
    range: super::SourceRange,
) -> Option<HoverContent> {
    let body = scope.body.as_ref()?;
    let source = context.source();
    let file = context.file();
    let workspace = context.workspace();
    let expressions = &body.tree.expressions;
    let base = body.base;
    let found = expressions.iter().enumerate().find_map(|(index, entry)| {
        let crate::syntax::ExprKind::Selector { operand, field } = &entry.kind else {
            return None;
        };
        ((base + field.start..base + field.end) == *span).then_some((index, *operand))
    });
    let (index, operand) = found?;
    let operand_expression = expressions.get(operand.0)?;
    let receiver = match &operand_expression.kind {
        crate::syntax::ExprKind::Name => {
            let name = source
                .get(base + operand_expression.span.start..base + operand_expression.span.end)?;
            let nested = render::nested_locals(context, offset);
            let locals = if scope
                .locals
                .resolve_at(base + operand_expression.span.start, name)
                .is_some()
            {
                &scope.locals
            } else {
                nested.as_ref()?
            };
            let binding = locals.resolve_at(base + operand_expression.span.start, name)?;
            let native = workspace.binding_type(file, source, locals, binding);
            let rendered = native
                .clone()
                .or_else(|| render::binding_text(workspace, file, source, locals, binding))
                .or_else(|| render::binding_initializer_type(context, source, locals, binding, 0));
            rendered?
        }
        _ => {
            let native = crate::analysis::check::Inference::new(
                workspace,
                file,
                source,
                &scope.locals,
                expressions,
                base,
            )
            .receiver_type(crate::syntax::ExprId(index));
            native?
        }
    };
    let name = source.get(span.clone())?;
    if let Some(member) = workspace.scoped_receiver_member(file, file, &receiver, name) {
        return match member {
            crate::analysis::method_set::ReceiverMember::Method(path, symbol) => {
                symbol::reference(context, path, symbol, range).ok()?
            }
            crate::analysis::method_set::ReceiverMember::Declaration(member) => {
                Some(render::member(
                    context,
                    member,
                    Some((file, receiver.as_str())),
                    span,
                    range,
                ))
            }
        };
    }
    if let Some(content) = render::anonymous_struct_member(context, &receiver, name, range) {
        return Some(content);
    }
    qualified_member(context, &receiver, name, range)
}

/// Resolve a member of a package-qualified receiver type even when the
/// consumer does not import that package spelling.
fn qualified_member(
    context: &Context<'_>,
    receiver: &str,
    name: &str,
    range: super::SourceRange,
) -> Option<HoverContent> {
    let mut ty = receiver.trim();
    while let Some(rest) = ty.strip_prefix('*') {
        ty = rest.trim();
    }
    let (package, type_name) = ty.split_once('.')?;
    let type_name = type_name.split(['[', ' ']).next()?;
    let workspace = context.workspace();
    for (path, stub) in workspace.files.iter().take(4096) {
        if stub.package != package {
            continue;
        }
        let Some(import_path) = workspace.import_path(path) else {
            continue;
        };
        if import_path == package || import_path.rsplit('/').next() != Some(package) {
            continue;
        }
        let Some((_, symbol)) = workspace.type_declaration(path, type_name) else {
            continue;
        };
        let lookup = path.as_path();
        if let Some((method_file, method)) = workspace
            .scoped_receiver_methods(context.file(), lookup, &symbol.name)
            .into_iter()
            .find(|(_, method)| {
                method.name == name && crate::syntax::is_exported_name(&method.name)
            })
            && let Ok(content) = symbol::reference(context, method_file, method, range)
            && content.is_some()
        {
            return content;
        }
    }
    None
}

/// A promoted method selection: the `// through` suffix and its outer receiver
/// type, used for gopls' enclosing-receiver link.
struct PromotedMethod {
    suffix: String,
    method_name: String,
    receiver_file: std::path::PathBuf,
    receiver_text: String,
}

impl PromotedMethod {
    /// Link gopls derives from the enclosing (outermost) receiver type when the
    /// declaring receiver is not exported.
    fn outer_link(&self, context: &Context<'_>) -> Option<HoverLink> {
        if render::gopls_test_view_link(context) || render::platform_test_variant(context) {
            return None;
        }
        let workspace = context.workspace();
        let receiver = self.receiver_text.trim_start_matches('*');
        let receiver = receiver.split('[').next().unwrap_or(receiver);
        let (file, symbol) = workspace.type_declaration(&self.receiver_file, receiver)?;
        if !crate::syntax::is_exported_name(&symbol.name) {
            return None;
        }
        if file.file_name()?.to_string_lossy().ends_with("_test.go") {
            return None;
        }
        let consumer = context.file();
        if consumer
            .file_name()?
            .to_string_lossy()
            .ends_with("_test.go")
            && same_package(context, consumer, file)
        {
            return None;
        }
        let package = render::package_name(workspace, file);
        Some(HoverLink {
            symbol: format!("({package}.{}).{}", symbol.name, self.method_name),
            path: render::link_import_path(workspace, file)?,
            anchor: format!("{}.{}", symbol.name, self.method_name),
        })
    }
}

/// gopls appends `// through ...` when the selected method is promoted.
fn promoted_method(
    context: &Context<'_>,
    scope: &crate::analysis::scope::CursorScope,
    span: &Range<usize>,
    method_file: &Path,
    method: &crate::index::IndexedSymbol,
) -> Option<PromotedMethod> {
    let (receiver_file, receiver_text) = method_receiver(context, scope, span)?;
    let owner = method.receiver.as_deref();
    let owner = owner?;
    let (owner_file, owner_symbol) = context.workspace().type_declaration(method_file, owner)?;
    let path = promote::promotion_path(
        context,
        &receiver_file,
        &receiver_text,
        owner_file,
        owner_symbol,
    );
    let path = path?;
    (!path.is_empty()).then(|| PromotedMethod {
        suffix: format!(" // through {}", path.join(", ")),
        method_name: method.name.clone(),
        receiver_file,
        receiver_text,
    })
}

/// Receiver type of a selected method, from inference with a binding fallback.
fn method_receiver(
    context: &Context<'_>,
    scope: &crate::analysis::scope::CursorScope,
    span: &Range<usize>,
) -> Option<(std::path::PathBuf, String)> {
    if let Some(receiver) = member_receiver(context, scope, span) {
        return Some(receiver);
    }
    let body = scope.body.as_ref()?;
    let source = context.source();
    let base = body.base;
    let operand = body.tree.expressions.iter().find_map(|expression| {
        let crate::syntax::ExprKind::Selector { operand, field } = &expression.kind else {
            return None;
        };
        ((base + field.start..base + field.end) == *span).then_some(*operand)
    })?;
    let expression = body.tree.expressions.get(operand.0)?;
    let crate::syntax::ExprKind::Name = expression.kind else {
        return None;
    };
    let name = source.get(expression.span.start + base..expression.span.end + base)?;
    let binding = scope
        .locals
        .resolve_at(expression.span.start + base, name)?;
    let text = binding.declared_type.clone().or_else(|| {
        binding
            .annotation()
            .and_then(|annotation| source.get(annotation.clone()).map(str::to_owned))
    })?;
    let rendered = context
        .workspace()
        .signature_type(context.file(), context.file(), &text)?;
    Some((context.file().to_path_buf(), rendered))
}

/// Receiver expression of a selected field/method, for instantiated members.
fn member_receiver(
    context: &Context<'_>,
    scope: &crate::analysis::scope::CursorScope,
    span: &Range<usize>,
) -> Option<(std::path::PathBuf, String)> {
    let body = scope.body.as_ref()?;
    let source = context.source();
    let file = context.file();
    let workspace = context.workspace();
    let expressions = &body.tree.expressions;
    let base = body.base;
    for (index, expression) in expressions.iter().enumerate() {
        let crate::syntax::ExprKind::Selector { field, .. } = &expression.kind else {
            continue;
        };
        if (base + field.start..base + field.end) != *span {
            continue;
        }
        let ty = crate::analysis::check::Inference::new(
            workspace,
            file,
            source,
            &scope.locals,
            expressions,
            base,
        )
        .scoped_receiver_type(crate::syntax::ExprId(index))?;
        return Some((ty.scope.to_path_buf(), ty.text));
    }
    // Composite literal keys: the receiver is the written literal type.
    for (index, expression) in expressions.iter().enumerate() {
        let crate::syntax::ExprKind::KeyValue { key, .. } = &expression.kind else {
            continue;
        };
        if (base + expressions[key.0].span.start..base + expressions[key.0].span.end) != *span {
            continue;
        }
        for parent in expressions {
            let crate::syntax::ExprKind::Composite {
                literal_type: Some(literal_type),
                elements,
            } = &parent.kind
            else {
                continue;
            };
            if !elements.contains(&crate::syntax::ExprId(index)) {
                continue;
            }
            let span = &expressions[literal_type.0].span;
            let text = source.get(base + span.start..base + span.end)?;
            return Some((file.to_path_buf(), text.to_owned()));
        }
    }
    None
}

/// Whether an orphaned open file is excluded by a non-portability build tag.
///
/// gopls answers hover for GOOS/GOARCH-excluded files but reports missing
/// package metadata for files behind custom tags (boringcrypto, integration,
/// exclude, ...). Detached admission otherwise mirrors the former.
fn detached_with_custom_tags(
    workspace: &crate::workspace::Workspace,
    file: &Path,
    source: &str,
) -> bool {
    if !workspace.is_detached(file) {
        return false;
    }
    const PLATFORM_TAGS: &[&str] = &[
        "aix",
        "android",
        "darwin",
        "dragonfly",
        "freebsd",
        "hurd",
        "illumos",
        "ios",
        "js",
        "linux",
        "netbsd",
        "openbsd",
        "plan9",
        "solaris",
        "wasip1",
        "windows",
        "zos",
        "386",
        "amd64",
        "amd64p32",
        "arm",
        "arm64",
        "arm64be",
        "armbe",
        "loong64",
        "mips",
        "mipsle",
        "mips64",
        "mips64le",
        "ppc64",
        "ppc64le",
        "riscv64",
        "s390x",
        "sparc64",
        "wasm",
        "unix",
        "cgo",
        "gc",
        "gccgo",
        "race",
        "msan",
        "asan",
        "purego",
    ];
    let mut constrained = false;
    for line in source.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if !line.starts_with("//") {
            break;
        }
        let Some(constraint) = line
            .strip_prefix("//go:build")
            .or_else(|| line.strip_prefix("// +build"))
        else {
            continue;
        };
        constrained = true;
        for word in constraint.split(|ch: char| !(ch.is_alphanumeric() || ch == '_' || ch == '.')) {
            if word.is_empty() || matches!(word, "and" | "or" | "not" | "true" | "false") {
                continue;
            }
            let known = PLATFORM_TAGS.contains(&word)
                || word
                    .strip_prefix("go1.")
                    .is_some_and(|minor| minor.chars().all(|ch| ch.is_ascii_digit()));
            if !known {
                // gopls reports missing metadata only when the directory has a
                // package it could have loaded; a lone custom-tag file in its
                // own directory is still answered as an orphan package.
                return directory_has_other_go(file);
            }
        }
    }
    let _ = constrained;
    false
}

/// Whether the file's directory contains another Go source file.
fn directory_has_other_go(file: &Path) -> bool {
    let Some(directory) = file.parent() else {
        return false;
    };
    let Ok(entries) = std::fs::read_dir(directory) else {
        return true;
    };
    entries.take(64).filter_map(Result::ok).any(|entry| {
        let path = entry.path();
        path != *file && path.extension().is_some_and(|extension| extension == "go")
    })
}

/// Whether the identifier token contains or ends at the cursor offset.
fn identifier_at(source: &str, offset: usize) -> Option<Range<usize>> {
    let mut previous = None;
    for token in Lexer::new(source) {
        if token.span.start > offset {
            break;
        }
        if token.kind != TokenKind::Ident {
            previous = None;
            continue;
        }
        if offset <= token.span.end {
            return Some(token.span);
        }
        previous = Some(token.span);
    }
    previous.filter(|span| span.end == offset)
}

fn is_package_clause_name(source: &str, offset: usize) -> bool {
    let mut tokens = Lexer::new(source);
    let Some(package) = tokens
        .next()
        .filter(|token| token.kind == TokenKind::Package)
    else {
        return false;
    };
    tokens.next().is_some_and(|token| {
        token.kind == TokenKind::Ident && token.span.start == offset && package.span.start < offset
    })
}

pub(super) fn same_package(context: &Context<'_>, file: &Path, origin: &Path) -> bool {
    let workspace = context.workspace();
    let Some((_, origin_stub)) = workspace.files.get_key_value(origin) else {
        return false;
    };
    let Some((_, file_stub)) = workspace.files.get_key_value(file) else {
        return false;
    };
    origin.parent() == file.parent() && origin_stub.package == file_stub.package
}

/// Package-level qualifier for a declaration from another package, if the
/// consumer file imports it under a spellable name.
pub(super) fn import_qualifier(
    context: &Context<'_>,
    file: &Path,
    origin: &Path,
) -> Option<String> {
    if same_package(context, file, origin) {
        return Some(String::new());
    }
    let workspace = context.workspace();
    let import = workspace.import_path(origin)?;
    let stub = workspace.files.get(file)?;
    // The go/types file qualifier keeps the last import of a package path.
    let candidate = stub
        .imports
        .iter()
        .rev()
        .find(|candidate| !candidate.is_blank() && candidate.path == import)?;
    let alias = workspace.import_name(candidate)?;
    if candidate.is_dot() {
        Some(String::new())
    } else {
        Some(format!("{alias}."))
    }
}
