//! Identity-based identifier categories; protocol numbers belong to features.
use crate::analysis::check::Inference;
use crate::analysis::scope::universe::{builtin_function, builtin_type};
use crate::analysis::scope::{Identity, ObjectKind, Resolver};
use crate::analysis::source::{Name, Role, SourceAnalysis};
use crate::analysis::types::{Mode, Operand, TypeArena};
use crate::syntax::ExprKind;
use crate::workspace::Workspace;
use std::ops::Range;
use std::path::Path;

pub(crate) fn classify(
    analysis: &SourceAnalysis,
    resolver: &Resolver<'_>,
    source: &str,
    name: &Name,
    identity: &Option<Identity<'_>>,
) -> ObjectKind {
    if matches!(name.role, Role::EmbeddedType(_)) {
        return ObjectKind::Property;
    }
    if (matches!(name.role, Role::Name | Role::Type(None))
        || matches!(name.role, Role::LiteralKey(_) | Role::LiteralElementKey(_))
            && matches!(identity, Some(Identity::Local(_))))
        && let Some(binding) = analysis
            .locals
            .definition_at(&name.span, &source[name.span.clone()])
    {
        return binding.object_kind;
    }
    if identity.is_none() && matches!(name.role, Role::Name) {
        let spelling = &source[name.span.clone()];
        if resolver.imported_name(spelling) {
            return ObjectKind::Namespace;
        }
        if builtin_type(spelling) {
            return ObjectKind::Type;
        }
        if builtin_function(spelling) {
            return ObjectKind::Function;
        }
    }
    match &name.role {
        Role::BlankDeclaration { kind, .. } => *kind,
        // A field declaration in a named or anonymous aggregate is a property;
        // interface methods keep their method identity through the member lookup.
        Role::FieldDeclaration(_) => match identity {
            Some(Identity::Member(member)) if member.kind == crate::syntax::MemberKind::Method => {
                ObjectKind::Method
            }
            _ => ObjectKind::Property,
        },
        Role::Parameter => ObjectKind::Parameter,
        Role::Result => ObjectKind::Variable,
        Role::Namespace => ObjectKind::Namespace,
        Role::Type(Some(qualifier)) => {
            // An unresolved import-qualified type spelling falls back to gopls'
            // syntactic selector category (function when called, else variable).
            if identity.is_none()
                && resolver.imported_name(source.get(qualifier.clone()).unwrap_or_default())
            {
                return unresolved_selector_kind(source, name.span.end);
            }
            ObjectKind::Type
        }
        Role::Type(None) => ObjectKind::Type,
        _ => {
            if identity.is_none()
                && is_unresolved_imported_selector(analysis, resolver, source, name)
            {
                return unresolved_selector_kind(source, name.span.end);
            }
            if identity.is_none()
                && let Some(kind) = native_selector_kind(resolver, analysis, source, name)
            {
                return kind;
            }
            match identity {
                Some(Identity::Member(member)) => {
                    if member.kind == crate::syntax::MemberKind::Method {
                        ObjectKind::Method
                    } else {
                        ObjectKind::Property
                    }
                }
                Some(Identity::Global(_, symbol)) => {
                    if symbol.is_function() {
                        ObjectKind::Function
                    } else if symbol.is_method() {
                        ObjectKind::Method
                    } else if symbol.is_type() {
                        ObjectKind::Type
                    } else {
                        ObjectKind::Variable
                    }
                }
                _ if matches!(name.role, Role::Member(_) | Role::ExpressionMember(_)) => {
                    // Without object identity, gopls still marks a directly
                    // called selector as a function; other selectors inherit a
                    // member-style property category.
                    match unresolved_selector_kind(source, name.span.end) {
                        ObjectKind::Function => ObjectKind::Function,
                        _ => ObjectKind::Property,
                    }
                }
                _ if matches!(name.role, Role::LiteralKey(_) | Role::LiteralElementKey(_)) => {
                    // A key of a literal whose written type names an unresolved
                    // imported package has no field identity; gopls falls back
                    // to its syntactic variable category.
                    if identity.is_none()
                        && unresolved_imported_literal_type(analysis, resolver, source, name)
                    {
                        ObjectKind::Variable
                    } else {
                        ObjectKind::Property
                    }
                }
                _ if matches!(name.role, Role::Field) => ObjectKind::Property,
                _ => ObjectKind::Variable,
            }
        }
    }
}

/// gopls' syntactic fallback for a selector whose object is unavailable: a
/// directly called selector is a function, otherwise a variable.
fn unresolved_selector_kind(source: &str, end: usize) -> ObjectKind {
    let mut index = end.min(source.len());
    loop {
        while index < source.len() && source.as_bytes()[index].is_ascii_whitespace() {
            index += 1;
        }
        if source.get(index..index + 2) == Some("//") {
            index = source[index..]
                .find('\n')
                .map_or(source.len(), |offset| index + offset + 1);
            continue;
        }
        if source.get(index..index + 2) == Some("/*") {
            index = source[index + 2..]
                .find("*/")
                .map_or(source.len(), |offset| index + 2 + offset + 2);
            continue;
        }
        break;
    }
    if source.as_bytes().get(index) == Some(&b'(') {
        ObjectKind::Function
    } else {
        ObjectKind::Variable
    }
}

/// Whether this occurrence is a selector whose qualifier is an imported package
/// name that the workspace cannot resolve (a dependency missing offline). gopls
/// then classifies the selector syntactically.
fn is_unresolved_imported_selector(
    analysis: &SourceAnalysis,
    resolver: &Resolver<'_>,
    source: &str,
    name: &Name,
) -> bool {
    let (qualifier, offset) = match &name.role {
        Role::Member(Some(qualifier)) => {
            let Some(text) = source.get(qualifier.clone()) else {
                return false;
            };
            (text, qualifier.start)
        }
        Role::ExpressionMember(operand) => {
            let Some(text) = source.get(operand.clone()) else {
                return false;
            };
            if !is_simple_identifier(text) {
                return false;
            }
            (text, operand.start)
        }
        _ => return false,
    };
    analysis.locals.resolve_at(offset, qualifier).is_none() && resolver.imported_name(qualifier)
}

fn is_simple_identifier(text: &str) -> bool {
    let mut bytes = text.bytes();
    bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

/// Whether a composite-literal key's written type names an imported package the
/// workspace cannot resolve. Anonymous/local literal types keep field identity.
fn unresolved_imported_literal_type(
    analysis: &SourceAnalysis,
    resolver: &Resolver<'_>,
    source: &str,
    name: &Name,
) -> bool {
    let (text, offset) = match &name.role {
        Role::LiteralKey(ty) => match source.get(ty.clone()) {
            Some(text) => (text.to_owned(), ty.start),
            None => return false,
        },
        Role::LiteralElementKey(id) => {
            match analysis.literal_element_type(resolver.workspace(), resolver.file(), source, *id)
            {
                Some((text, offset)) => (text, offset),
                None => return false,
            }
        }
        _ => return false,
    };
    let text = text.trim_start_matches(['*', '&']).trim_start();
    let Some((qualifier, _)) = text.split_once('.') else {
        return false;
    };
    is_simple_identifier(qualifier)
        && analysis.locals.resolve_at(offset, qualifier).is_none()
        && resolver.imported_name(qualifier)
}

/// Resolve a selector's member kind from the native type graph when the spelling
/// based resolver cannot. The receiver type comes from the checked operand, so a
/// method is never inferred from a same-named method elsewhere.
fn native_selector_kind(
    resolver: &Resolver<'_>,
    analysis: &SourceAnalysis,
    source: &str,
    name: &Name,
) -> Option<ObjectKind> {
    let key = resolver.selector_cache_key(analysis, source, name);
    if let Some(key) = &key
        && let Some(value) = resolver.cached_selector_kind(key)
    {
        return value;
    }
    let value = (|| {
        let span = match &name.role {
            Role::ExpressionMember(operand) => operand.start..name.span.end,
            Role::Member(Some(qualifier)) => {
                let text = source.get(qualifier.clone())?;
                if let Some(binding) = analysis.locals.resolve_at(qualifier.start, text) {
                    binding.object_kind.is_value().then_some(())?;
                } else if !resolver
                    .workspace()
                    .unqualified_symbol(resolver.file(), text)
                    .is_some_and(|(_, symbol)| symbol.is_value())
                {
                    return None;
                }
                qualifier.start..name.span.end
            }
            _ => return None,
        };
        native_selector_kind_at(resolver, analysis, source, span, &source[name.span.clone()])
    })();
    if let Some(key) = key {
        resolver.store_selector_kind(key, value);
    }
    value
}

fn native_selector_kind_at(
    resolver: &Resolver<'_>,
    analysis: &SourceAnalysis,
    source: &str,
    span: Range<usize>,
    field: &str,
) -> Option<ObjectKind> {
    let workspace = resolver.workspace();
    let file = resolver.file();
    if span.len() > 16 * 1024 {
        return None;
    }
    let parsed = crate::syntax::parse_expression(source.get(span.clone())?);
    if !parsed.diagnostics.is_empty() {
        return None;
    }
    let ExprKind::Selector {
        operand,
        field: field_span,
    } = &parsed.nodes[parsed.root.0].kind
    else {
        return None;
    };
    if source.get(span.start + field_span.start..span.start + field_span.end) != Some(field) {
        return None;
    }
    let inference = Inference::new(
        workspace,
        file,
        source,
        &analysis.locals,
        &parsed.nodes,
        span.start,
    )
    .with_scoped_bindings(resolver.scoped_bindings());
    let mut arena = TypeArena::new(workspace);
    let input = receiver_operand(&inference, &mut arena, &parsed.nodes, *operand, 0);
    let Some(input) = input else {
        // The receiver may be a call whose result the arena cannot import from a
        // dependency-only origin. Resolve the member on the declared result type
        // by its package identity, never by a workspace-wide name match.
        return declared_selector_kind(workspace, &inference, &parsed.nodes, *operand, field);
    };
    let ty = input.ty;
    if arena.is_interface(ty)? {
        // Interface selectors can only be methods; a checked miss stays unknown.
        return arena
            .selector_operand(input, file, field)?
            .map(|_| ObjectKind::Method);
    }
    if arena.method_declaration(ty, file, field).is_some() {
        return Some(ObjectKind::Method);
    }
    if input.mode == Mode::Variable
        && let Some(pointer) = arena.pointer(ty)
        && arena.method_declaration(pointer, file, field).is_some()
    {
        return Some(ObjectKind::Method);
    }
    // Only a checked concrete field projects as a property.
    arena
        .selector_operand(input, file, field)?
        .map(|_| ObjectKind::Property)
}

/// Call result of an interface method selected on a native interface value. The
/// interface declaration supplies the result annotation; no implementation is
/// inferred and no argument is checked.
fn interface_method_result(
    inference: &Inference<'_, '_>,
    arena: &mut TypeArena<'_>,
    nodes: &[crate::syntax::Expr],
    function: crate::syntax::ExprId,
    result: usize,
    count: usize,
) -> Option<Operand> {
    let ExprKind::Selector { operand, field } = &nodes[function.0].kind else {
        return None;
    };
    let receiver = receiver_operand(inference, arena, nodes, *operand, 0)?;
    if !arena.is_interface(receiver.ty)? {
        return None;
    }
    let name = inference
        .source
        .get(inference.base + field.start..inference.base + field.end)?;
    let spelling = arena.render(receiver.ty, inference.file)?;
    let root = crate::analysis::scope::named_root(&spelling);
    let (path, symbol) = inference.workspace.type_declaration(inference.file, root)?;
    let text = inference
        .workspace
        .source_slice(path, symbol.end..symbol.declaration()?.end)?;
    let parsed = crate::syntax::parse_type(&text);
    if !parsed.diagnostics.is_empty() {
        return None;
    }
    let member = parsed.interface_members.iter().find(|member| {
        member
            .name
            .as_ref()
            .and_then(|span| text.get(span.clone()))
            .is_some_and(|member| member == name)
    })?;
    let annotation = text.get(member.annotation.clone())?;
    let header = format!("func _{annotation}");
    let signature = crate::syntax::parse_signature(&header);
    if !signature.diagnostics.is_empty() || signature.results.len() != count || result >= count {
        return None;
    }
    let annotation = header.get(signature.results[result].annotation.clone())?;
    Some(Operand {
        mode: Mode::Value,
        ty: arena.parse(path, annotation)?,
        value: None,
    })
}

/// Result type of a method call resolved through the native member walk, which
/// also covers promoted methods. The declaration header only projects a type; no
/// argument is checked and no implementation is inferred.
fn native_method_call_result(
    inference: &Inference<'_, '_>,
    arena: &mut TypeArena<'_>,
    nodes: &[crate::syntax::Expr],
    function: crate::syntax::ExprId,
    result: usize,
    count: usize,
) -> Option<Operand> {
    let ExprKind::Selector { operand, field } = &nodes[function.0].kind else {
        return None;
    };
    let receiver = receiver_operand(inference, arena, nodes, *operand, 0)?;
    let name = inference
        .source
        .get(inference.base + field.start..inference.base + field.end)?;
    let mut declaration = arena.method_declaration(receiver.ty, inference.file, name);
    if declaration.is_none()
        && receiver.mode == Mode::Variable
        && let Some(pointer) = arena.pointer(receiver.ty)
    {
        declaration = arena.method_declaration(pointer, inference.file, name);
    }
    let (path, symbol) = declaration?;
    let annotation = method_result_annotation(inference, path, symbol, result, count)?;
    let ty = arena
        .parse(inference.file, &annotation)
        .or_else(|| arena.parse(path, &annotation))?;
    Some(Operand {
        mode: Mode::Value,
        ty,
        value: None,
    })
}

fn method_result_annotation(
    inference: &Inference<'_, '_>,
    path: &Path,
    symbol: &crate::index::IndexedSymbol,
    result: usize,
    count: usize,
) -> Option<String> {
    let declaration = symbol.declaration()?;
    let end = symbol.body().map_or(declaration.end, |body| body.start);
    let header = inference
        .workspace
        .source_slice(path, declaration.start..end)?;
    let signature = crate::syntax::parse_signature(&header);
    if !signature.diagnostics.is_empty() || signature.results.len() != count || result >= count {
        return None;
    }
    header
        .get(signature.results[result].annotation.clone())
        .map(str::to_owned)
}

/// Membership of a selector on a call result whose dependency-only origin cannot
/// be imported into the arena. The call result annotation locates the declared
/// type through the declaring file's imports; the member lookup then uses that
/// declaration scope. No workspace-wide name search is involved.
fn declared_selector_kind(
    workspace: &Workspace,
    inference: &Inference<'_, '_>,
    nodes: &[crate::syntax::Expr],
    receiver: crate::syntax::ExprId,
    field: &str,
) -> Option<ObjectKind> {
    let ExprKind::Call { function, .. } = &nodes[receiver.0].kind else {
        return None;
    };
    let (annotation_path, annotation) = call_result_annotation(inference, nodes, *function)?;
    let name = crate::analysis::scope::named_type(&annotation)?;
    let (path, symbol) = workspace.type_declaration(&annotation_path, &name)?;
    let member = workspace.scoped_receiver_member(path, path, &symbol.name, field)?;
    match member {
        crate::analysis::method_set::ReceiverMember::Method(..) => Some(ObjectKind::Method),
        crate::analysis::method_set::ReceiverMember::Declaration(member) => {
            if member.kind == crate::syntax::MemberKind::Method {
                Some(ObjectKind::Method)
            } else {
                Some(ObjectKind::Property)
            }
        }
    }
}

/// Declaration file and written result annotation of a call, resolved either
/// through the callee symbol or through the native method declaration of a
/// selector callee.
fn call_result_annotation(
    inference: &Inference<'_, '_>,
    nodes: &[crate::syntax::Expr],
    function: crate::syntax::ExprId,
) -> Option<(std::path::PathBuf, String)> {
    if let Some((path, symbol)) = inference.callee(function)
        && symbol.is_callable()
    {
        return Some((
            path.clone(),
            method_result_annotation(inference, path, symbol, 0, 1)?,
        ));
    }
    let ExprKind::Selector { operand, field } = &nodes[function.0].kind else {
        return None;
    };
    let mut arena = TypeArena::new(inference.workspace);
    let receiver = receiver_operand(inference, &mut arena, nodes, *operand, 0)?;
    let name = inference
        .source
        .get(inference.base + field.start..inference.base + field.end)?;
    let (path, symbol) = arena.method_declaration(receiver.ty, inference.file, name)?;
    Some((
        path.to_path_buf(),
        method_result_annotation(inference, path, symbol, 0, 1)?,
    ))
}

/// Result type of a resolved function/method call, read from the declaration
/// header in its own file. This only projects the type; it never proves the call
/// arguments, so unsupported actuals cannot hide the receiver's declared type.
fn call_result_type(
    inference: &Inference<'_, '_>,
    arena: &mut TypeArena<'_>,
    function: crate::syntax::ExprId,
    result: usize,
    count: usize,
) -> Option<Operand> {
    let (file, symbol) = inference.callee(function)?;
    if !symbol.is_callable() {
        return None;
    }
    let declaration = symbol.declaration()?;
    let end = symbol.body().map_or(declaration.end, |body| body.start);
    let header = inference
        .workspace
        .source_slice(file, declaration.start..end)?;
    let signature = crate::syntax::parse_signature(&header);
    if !signature.diagnostics.is_empty() || signature.results.len() != count || result >= count {
        return None;
    }
    let result_span = &signature.results[result].annotation;
    // A formal type parameter needs call-site instantiation. Parsing it in the
    // package scope could instead bind an unrelated declaration of that name.
    if signature.type_references.iter().any(|reference| {
        let root = reference.qualifier.as_ref().unwrap_or(&reference.name);
        result_span.start <= root.start
            && root.end <= result_span.end
            && signature.type_parameters.iter().any(|parameter| {
                parameter
                    .name
                    .as_ref()
                    .is_some_and(|name| header.get(name.clone()) == header.get(root.clone()))
            })
    }) {
        return None;
    }
    let annotation = header.get(result_span.clone())?;
    Some(Operand {
        mode: Mode::Value,
        ty: annotation_type_id(inference.workspace, arena, file, annotation)?,
        value: None,
    })
}

/// Parse a declaration annotation in its declaring file. Closed files whose
/// import scope is not reachable through `arena.parse` fall back to resolving a
/// pointer-to-qualified-name spelling through the file's own imports.
fn annotation_type_id(
    workspace: &Workspace,
    arena: &mut TypeArena<'_>,
    origin: &Path,
    annotation: &str,
) -> Option<crate::analysis::types::TypeId> {
    if let Some(ty) = arena.parse(origin, annotation) {
        return Some(ty);
    }
    let mut rest = annotation.trim();
    let mut pointers = 0usize;
    while let Some(next) = rest.strip_prefix('*') {
        pointers += 1;
        if pointers > 16 {
            return None;
        }
        rest = next.trim_start();
    }
    if pointers == 0 || rest.contains(['[', ']', ' ', ';']) {
        return None;
    }
    let (alias, name) = rest.split_once('.')?;
    if !is_simple_identifier(alias) || !is_simple_identifier(name) {
        return None;
    }
    let mut candidates = workspace
        .imported_symbol_iter(origin, alias)
        .filter(|(_, symbol)| symbol.is_type() && symbol.name == name);
    let (path, _) = candidates.next()?;
    if candidates.next().is_some() {
        return None;
    }
    let mut ty = arena.parse(path, name)?;
    for _ in 0..pointers {
        ty = arena.pointer(ty)?;
    }
    Some(ty)
}

/// Type of a package-level value whose declared annotation is written in its own
/// file; the caller's source slice is not retained and only the type text is used.
fn package_value_operand(
    workspace: &Workspace,
    path: &Path,
    symbol: &crate::index::IndexedSymbol,
    arena: &mut TypeArena<'_>,
) -> Option<Operand> {
    let declaration = symbol.declaration()?;
    let spec = workspace.source_slice(path, declaration)?;
    let mut tokens = crate::syntax::Lexer::new(&spec).peekable();
    tokens
        .next()
        .filter(|token| token.kind == crate::syntax::TokenKind::Ident)?;
    while tokens
        .peek()
        .is_some_and(|token| token.kind == crate::syntax::TokenKind::Comma)
    {
        tokens.next();
        tokens.next()?;
    }
    let token = tokens.peek()?;
    if token.kind == crate::syntax::TokenKind::Assign {
        return None;
    }
    let start = token.span.start;
    let prefix = crate::syntax::parse_type_prefix(&spec[start..]);
    if !prefix.diagnostics.is_empty() {
        return None;
    }
    let text = spec.get(start..start + prefix.span.end)?;
    Some(Operand {
        mode: Mode::Variable,
        ty: arena.parse(path, text)?,
        value: None,
    })
}

/// Native element operand of a written range expression; the legacy binding walk
/// remains only as a fallback for unsupported iteration forms.
fn native_range_operand(
    inference: &Inference<'_, '_>,
    arena: &mut TypeArena<'_>,
    span: Range<usize>,
    result: usize,
    count: usize,
    depth: usize,
) -> Option<Operand> {
    if depth >= 32 || span.len() > 16 * 1024 {
        return None;
    }
    let parsed = crate::syntax::parse_expression(inference.source.get(span.clone())?);
    if !parsed.diagnostics.is_empty() {
        return None;
    }
    let mut nested = Inference::new(
        inference.workspace,
        inference.file,
        inference.source,
        inference.locals,
        &parsed.nodes,
        span.start,
    );
    if let Some(cache) = inference.shared_scoped_bindings() {
        nested = nested.with_scoped_bindings(cache);
    }
    let container = receiver_operand(&nested, arena, &parsed.nodes, parsed.root, depth + 1)?;
    let ty = arena.range_slot(
        container,
        result,
        count,
        inference.workspace.integer_bits()?,
    )?;
    Some(Operand {
        mode: Mode::Value,
        ty,
        value: None,
    })
}

pub(super) fn receiver_operand(
    inference: &Inference<'_, '_>,
    arena: &mut TypeArena<'_>,
    nodes: &[crate::syntax::Expr],
    id: crate::syntax::ExprId,
    depth: usize,
) -> Option<Operand> {
    if depth >= 32 {
        return None;
    }
    let expression = &nodes[id.0];
    match &expression.kind {
        ExprKind::Name => {
            let offset = inference.base + expression.span.start;
            let name = inference.source.get(
                inference.base + expression.span.start..inference.base + expression.span.end,
            )?;
            if let Some(binding) = inference.locals.resolve_at(offset, name) {
                if let Some(span) = binding.annotation() {
                    let annotation = inference.source.get(span.clone())?;
                    if let super::scope::AnnotationType::Resolved(ty) =
                        inference.annotation_type(arena, annotation, span.start)
                    {
                        return Some(Operand {
                            mode: Mode::Value,
                            ty,
                            value: None,
                        });
                    }
                }
                if let Some(operand) = binding
                    .range_initializer()
                    .and_then(|(span, result, count)| {
                        native_range_operand(
                            inference,
                            arena,
                            span.clone(),
                            result,
                            count,
                            depth + 1,
                        )
                    })
                    .or_else(|| inference.range_binding(arena, binding))
                {
                    return Some(operand);
                }
                if let Some((span, 0, 1)) = binding.initializer() {
                    let text = inference.source.get(span.clone())?;
                    if text.len() <= 16 * 1024 {
                        let parsed = crate::syntax::parse_expression(text);
                        if parsed.diagnostics.is_empty() {
                            let mut nested = Inference::new(
                                inference.workspace,
                                inference.file,
                                inference.source,
                                inference.locals,
                                &parsed.nodes,
                                span.start,
                            );
                            if let Some(cache) = inference.shared_scoped_bindings() {
                                nested = nested.with_scoped_bindings(cache);
                            }
                            return receiver_operand(
                                &nested,
                                arena,
                                &parsed.nodes,
                                parsed.root,
                                depth + 1,
                            );
                        }
                    }
                }
                inference
                    .parameterized_operand(arena, id)
                    .or_else(|| inference.value_operand(arena, id))
            } else if let Some((path, symbol)) =
                inference.workspace.unqualified_symbol(inference.file, name)
                && symbol.is_value()
            {
                package_value_operand(inference.workspace, path, symbol, arena)
            } else {
                inference
                    .parameterized_operand(arena, id)
                    .or_else(|| inference.value_operand(arena, id))
            }
        }
        ExprKind::Call {
            function,
            arguments,
            variadic,
        } => {
            // A resolved package-level function call has a definitive declared
            // result annotation. Resolve it before the speculative call paths,
            // which may exhaust the shared arena budget on unsupported callees.
            if inference
                .callee(*function)
                .is_some_and(|(_, symbol)| symbol.is_function())
                && let Some(found) = call_result_type(inference, arena, *function, 0, 1)
            {
                return Some(found);
            }
            if let Some(target) = inference.conversion_target(arena, *function) {
                Some(Operand {
                    mode: Mode::Value,
                    ty: target,
                    value: None,
                })
            } else if let Some(found) =
                interface_method_result(inference, arena, nodes, *function, 0, 1)
            {
                Some(found)
            } else if let Some(Some(found)) =
                inference.native_call(arena, *function, arguments, *variadic, 0, 1)
            {
                Some(found)
            } else if let Some(found) =
                native_method_call_result(inference, arena, nodes, *function, 0, 1)
            {
                Some(found)
            } else if let Some(found) = call_result_type(inference, arena, *function, 0, 1) {
                Some(found)
            } else if let Some(scoped) =
                inference.call_result(*function, arguments, *variadic, 0, 1)
            {
                // Parse the declaration-scoped result spelling back to a TypeId;
                // the receiver kind only needs the type, never a consumer name.
                Some(Operand {
                    mode: Mode::Value,
                    ty: arena.parse(scoped.scope, &scoped.text)?,
                    value: None,
                })
            } else {
                inference.value_operand(arena, id)
            }
        }
        ExprKind::Parenthesized { expression } => {
            receiver_operand(inference, arena, nodes, *expression, depth + 1)
        }
        ExprKind::Selector { operand, field } => {
            let base = receiver_operand(inference, arena, nodes, *operand, depth + 1)?;
            let name = inference
                .source
                .get(inference.base + field.start..inference.base + field.end)?;
            arena.selector_operand(
                base,
                inference.workspace.files.get_key_value(inference.file)?.0,
                name,
            )?
        }
        ExprKind::Index { operand, arguments } => {
            let base = receiver_operand(inference, arena, nodes, *operand, depth + 1)?;
            let index = arguments.first().copied()?;
            let index =
                receiver_operand(inference, arena, nodes, index, depth + 1).or_else(|| {
                    // Index slots of ranged containers are not needed to project
                    // the element type; a plain int keeps the key check bounded
                    // without inferring a value.
                    arena
                        .basic(super::super::types::BasicKind::Int)
                        .map(|ty| Operand {
                            mode: Mode::Variable,
                            ty,
                            value: None,
                        })
                })?;
            arena.index(base, index, 0, 1, inference.workspace.integer_bits()?)
        }
        ExprKind::TypeAssertion {
            operand,
            asserted_type,
        } => {
            // go/types records the asserted type even when satisfiability needs
            // facts this query does not prove; member-kind classification only
            // uses the projected type, never a value conversion.
            let _ = operand;
            let target = inference.type_target(arena, asserted_type)?;
            Some(Operand {
                mode: Mode::Value,
                ty: target,
                value: None,
            })
        }
        ExprKind::Slice { .. } => inference.sliced_operand(arena, id),
        ExprKind::Unary { operator, operand } => {
            match inference
                .source
                .get(inference.base + operator.start..inference.base + operator.end)
            {
                Some("&") => {
                    let inner = receiver_operand(inference, arena, nodes, *operand, depth + 1)?;
                    inner.mode.eq(&Mode::Variable).then_some(())?;
                    Some(Operand {
                        mode: Mode::Value,
                        ty: arena.pointer(inner.ty)?,
                        value: None,
                    })
                }
                _ => {
                    let inner = receiver_operand(inference, arena, nodes, *operand, depth + 1)?;
                    let operator = inference
                        .source
                        .get(inference.base + operator.start..inference.base + operator.end)?;
                    arena
                        .unary(operator, inner, inference.workspace.integer_bits()?)
                        .or_else(|| inference.value_operand(arena, id))
                }
            }
        }
        _ => inference
            .parameterized_operand(arena, id)
            .or_else(|| inference.value_operand(arena, id)),
    }
}
