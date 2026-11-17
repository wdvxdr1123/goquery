//! Package-level declaration descriptions (functions, types, vars, consts).
use super::content::HoverContent;
use super::render::{self, declaration_source};
use super::{HoverLink, doc, import_qualifier};
use crate::analysis::query::Context;
use crate::index::{IndexedSymbol as Symbol, SymbolKind};
use std::path::Path;

const MAX_DECLARATION_BYTES: usize = 256 * 1024;

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
            value_symbol(context, origin, source, symbol, range)
        }
    };
    let Some(mut content) = content else {
        return Ok(None);
    };
    let workspace = context.workspace();
    let consumer = context.file();
    if symbol.is_callable()
        && let Some(code) = content.code.take()
    {
        content.code = Some(render::replace_invalid_qualified(
            workspace, consumer, &code,
        ));
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
    let (signature, _) = crate::analysis::query::signatures::label::declared_signature(
        workspace,
        consumer,
        origin,
        header,
        &parsed,
        &symbol.name,
    )?;
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
            let constraint = crate::analysis::query::signatures::label::signature_type(
                workspace, consumer, origin, constraint,
            )
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
    let receiver_type = crate::analysis::query::signatures::label::signature_type(
        workspace,
        context.file(),
        origin,
        annotation,
    )?;
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
    let (_, owner) =
        crate::analysis::resolve::resolve_type_declaration(workspace, origin, receiver)?;
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
    let mut type_decl = format!("type {}", super::declaration::format_spec(spec));
    if declaring && let Some(layout) = super::size::type_layout(context, origin, symbol) {
        let wasted =
            super::size::struct_layout(context, origin, symbol).and_then(super::size::wasted);
        let summary = super::size::size_summary(layout, wasted);
        let line_end = type_decl.find('\n').unwrap_or(type_decl.len());
        type_decl.insert_str(line_end, &format!(" // {summary}"));
    }
    // gopls appends the aliased named type's own declaration for aliases.
    if let Some(text) = rhs_type_declaration(context, origin, source, symbol) {
        type_decl.push_str("\n\n");
        type_decl.push_str("type ");
        type_decl.push_str(super::declaration::format_spec(&text).trim_end());
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
    let methods = method_lines(context, origin, symbol);
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
    alias: &Symbol,
) -> Option<String> {
    let rhs = context.with_sema(|sema| {
        sema.alias_rhs_declaration(crate::analysis::obj::DeclId {
            file: origin,
            symbol: alias,
        })
    })?;
    let declaration = rhs.symbol.declaration()?;
    if rhs.file == origin {
        source.get(declaration).map(str::to_owned)
    } else {
        context.workspace().source_slice(rhs.file, declaration)
    }
}

/// Native method selection owns visibility, collisions and instantiation.
/// Source headers attach names only; no source text proves a method or type.
fn method_lines(context: &Context<'_>, origin: &Path, owner: &Symbol) -> Option<String> {
    let methods = context.with_sema(|sema| {
        sema.declaration_methods(
            crate::analysis::obj::DeclId {
                file: origin,
                symbol: owner,
            },
            context.file(),
        )
    })?;
    let mut lines = Vec::new();
    let mut header_bytes = 0usize;
    let mut output_bytes = 0usize;
    for method in methods {
        let mut receiver = method.receiver.clone();
        let mut label = format!("{}{}", method.name, method.signature.strip_prefix("func")?);
        if method.results.len() == 1 {
            let mut parameters = method.parameters.clone();
            if method.variadic {
                let last = parameters.last_mut()?;
                *last = format!("...{}", last.strip_prefix("[]")?);
            }
            label = format!(
                "{}({}) {}",
                method.name,
                parameters.join(", "),
                method.results[0]
            );
        }
        if let Some((header, parsed)) = method_header(context, &method, &mut header_bytes)? {
            if parsed.parameters.len() != method.parameters.len()
                || parsed.results.len() != method.results.len()
                || parsed
                    .parameters
                    .last()
                    .is_some_and(|parameter| parameter.variadic)
                    != method.variadic
            {
                return None;
            }
            let mut annotations = method
                .parameters
                .iter()
                .enumerate()
                .map(|(index, ty)| {
                    if method.variadic && index + 1 == method.parameters.len() {
                        ty.strip_prefix("[]").map(str::to_owned)
                    } else {
                        Some(ty.clone())
                    }
                })
                .chain(method.results.iter().map(|ty| Some(ty.clone())));
            label = crate::analysis::query::signatures::label::signature_label(
                &header,
                &parsed,
                &method.name,
                |_| annotations.next().flatten(),
            )?
            .0
            .label;
            if let Some(name) = parsed
                .receiver
                .first()
                .and_then(|receiver| receiver.name.as_ref())
                .and_then(|name| header.get(name.clone()))
                .filter(|name| !name.is_empty() && *name != "_")
            {
                receiver = format!("{name} {receiver}");
            }
        }
        let line = format!("func ({receiver}) {}", render::normalize_signature(&label));
        output_bytes = output_bytes.checked_add(line.len() + 1)?;
        if output_bytes > crate::analysis::budget::MAX_BYTES {
            return None;
        }
        lines.push(line);
    }
    (!lines.is_empty()).then(|| lines.join("\n"))
}

/// A bounded header from an exact native method identity. None inside Some
/// denotes a universe method, which has no source parameter names to attach.
fn method_header(
    context: &Context<'_>,
    method: &crate::analysis::sema::MethodView<'_>,
    read_bytes: &mut usize,
) -> Option<Option<(String, crate::syntax::Signature)>> {
    use crate::analysis::obj::{MethodOrigin, ObjectRef};
    const HEADER_BYTES: usize = crate::analysis::budget::MAX_BYTES;
    let (file, span, interface) = match method.object {
        ObjectRef::Method(MethodOrigin::Concrete(declaration)) => {
            let span = declaration.symbol.declaration()?;
            let end = declaration
                .symbol
                .body()
                .map_or(span.end, |body| body.start);
            (declaration.file, span.start..end, None)
        }
        ObjectRef::Method(MethodOrigin::Interface(member)) if member.builtin => return Some(None),
        ObjectRef::Method(MethodOrigin::Interface(member)) => {
            let member = member.indexed(context.workspace())?;
            let owner = member.owner?;
            (
                member.file,
                owner.end..owner.declaration()?.end,
                Some(member),
            )
        }
        _ => return None,
    };
    if span.len() > HEADER_BYTES {
        return None;
    }
    *read_bytes = read_bytes.checked_add(span.len())?;
    if *read_bytes > 64 * 1024 {
        return None;
    }
    let source = context.workspace().source_slice(file, span.clone())?;
    let header = if let Some(member) = interface {
        let entries = crate::syntax::aggregate_members(&source);
        let entry = entries.iter().find(|entry| {
            span.start + entry.selection.start == member.selection[0] as usize
                && span.start + entry.selection.end == member.selection[1] as usize
        })?;
        format!(
            "func {}{}",
            method.name,
            source.get(entry.annotation.clone())?
        )
    } else {
        source
    };
    let parsed = crate::syntax::parse_signature(&header);
    if !parsed.diagnostics.is_empty() {
        return None;
    }
    Some(Some((header, parsed)))
}

/// Evaluate a named constant expression (possibly inherited from its group).
fn evaluate_named_constant(
    context: &Context<'_>,
    origin: &Path,
    name: &str,
    depth: usize,
    budget: &mut super::value::TextBudget,
) -> Option<String> {
    budget.step()?;
    if depth >= 16 || name.is_empty() {
        return None;
    }
    let resolved = if let Some((alias, constant)) = name.split_once('.') {
        context
            .resolver()
            .imported_symbol_iter(origin, alias)
            .find(|(_, symbol)| symbol.is_const() && symbol.name == constant)
            .map(|(path, symbol)| (path.clone(), symbol))
    } else {
        context
            .resolver()
            .unqualified_symbol(origin, name)
            .filter(|(_, symbol)| symbol.is_const())
            .map(|(path, symbol)| (path.clone(), symbol))
    };
    let (file, symbol) = resolved?;
    let span = super::value::declaration_span(context, &file, symbol)?;
    budget.read(span.len())?;
    let source = context.workspace().source_slice(&file, span.clone())?;
    let display = super::value::declaration_in(context, &file, &source, span.start, symbol)?;
    let substituted = super::value::substitute_iota(display.value?, display.iota);
    if let Some(literal) = super::value::literal(&substituted) {
        return Some(literal.value);
    }
    if let Some(value) = super::value::integer_expression(&substituted) {
        return Some(value.to_string());
    }
    let trimmed = substituted.trim();
    evaluate_named_constant(context, &file, trimmed, depth + 1, budget)
}

/// Resolve a constant by its unqualified or import-qualified name.
fn resolve_constant<'a>(
    context: &Context<'a>,
    origin: &Path,
    name: &str,
) -> Option<(std::path::PathBuf, &'a Symbol)> {
    if let Some((alias, constant)) = name.split_once('.') {
        context
            .resolver()
            .imported_symbol_iter(origin, alias)
            .find(|(_, symbol)| symbol.is_const() && symbol.name == constant)
            .map(|(path, symbol)| (path.clone(), symbol))
    } else {
        context
            .resolver()
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
    budget: &mut super::value::TextBudget,
) -> Option<Vec<u8>> {
    if depth >= MAX_STRING_CONSTANT_DEPTH {
        return None;
    }
    super::value::string_expression(expression, depth, budget, &mut |name, next, budget| {
        let (file, symbol) = resolve_constant(context, origin, name)?;
        let span = super::value::declaration_span(context, &file, symbol)?;
        budget.read(span.len())?;
        let source = context.workspace().source_slice(&file, span.clone())?;
        let display = super::value::declaration_in(context, &file, &source, span.start, symbol)?;
        let substituted = super::value::substitute_iota(display.value?, display.iota);
        evaluate_string_constant(context, &file, &substituted, next, budget)
    })
}

/// Type display comes only from the checked declaration operand.
fn declaration_value_type(
    context: &Context<'_>,
    origin: &Path,
    symbol: &Symbol,
) -> Option<(String, Option<crate::analysis::types::Constant>)> {
    let (file, stub) = context.workspace().files.get_key_value(origin)?;
    let symbol = stub.symbols.iter().find(|candidate| {
        candidate.start == symbol.start
            && candidate.end == symbol.end
            && candidate.kind == symbol.kind
    })?;
    context.with_sema(|sema| {
        let operand = sema.operand_of_decl(crate::analysis::obj::DeclId { file, symbol })?;
        let text = sema.arena_mut().format_type(
            operand.ty,
            crate::analysis::types::Qualifier::Presentation(context.file()),
        )?;
        Some((render::normalize_type(&text), operand.value))
    })
}

fn value_symbol(
    context: &Context<'_>,
    origin: &Path,
    source: &str,
    symbol: &Symbol,
    range: super::SourceRange,
) -> Option<HoverContent> {
    let workspace = context.workspace();
    let declaration = symbol.declaration()?;
    let display = super::value::declaration(context, origin, source, symbol)?;
    let mut content = HoverContent::new(range);
    let qualifier = import_qualifier(context, context.file(), origin)?;
    let (ty, constant) = declaration_value_type(context, origin, symbol)?;
    let code = match symbol.kind {
        SymbolKind::Constant => {
            let expression = display.value?.to_owned();
            let own = display.own;
            let substituted = super::value::substitute_iota(&expression, display.iota);
            let mut literal = super::value::literal(&substituted);
            let mut evaluated = None;
            if literal.is_none() {
                let mut budget = super::value::TextBudget::new();
                if let Some(crate::analysis::types::Constant::Int(value)) = constant {
                    evaluated = i64::try_from(value).ok();
                    literal = Some(super::value::Literal {
                        value: value.to_string(),
                        declaration: own.then(|| expression.clone()),
                        untyped: Some("int"),
                    });
                } else if matches!(
                    constant,
                    Some(crate::analysis::types::Constant::String { .. })
                ) {
                    let bytes =
                        evaluate_string_constant(context, origin, &substituted, 0, &mut budget)?;
                    literal = Some(super::value::Literal {
                        value: super::value::quote_bytes(&bytes),
                        declaration: None,
                        untyped: Some("string"),
                    });
                } else if let Some(value) =
                    evaluate_named_constant(context, origin, substituted.trim(), 0, &mut budget)
                {
                    literal = Some(super::value::Literal {
                        value: value.clone(),
                        declaration: own.then(|| expression.clone()),
                        untyped: None,
                    });
                    evaluated = value.parse().ok();
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
            format!("const {qualifier}{} {ty} = {assigned}", symbol.name)
        }
        _ => {
            format!("var {qualifier}{} {ty}", symbol.name)
        }
    };
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
