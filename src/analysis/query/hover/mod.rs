//! Hover semantics: resolve the identifier under the cursor and describe it
//! using request-local metadata, source views and existing native inference.
mod builtin;
mod content;
mod declaration;
mod doc;
mod generic;
mod member;
mod package;
mod promote;
mod render;
mod size;
mod symbol;
mod value;
mod version;

pub(crate) use content::{HoverContent, HoverLink};

use super::navigation::Target;
use super::{Context, SourceRange};
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
    if !context.workspace().orphan_metadata_available(file) {
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
    let range = SourceRange::new(&map, span.clone());
    if let Some(content) = member::checked(context, &span, range) {
        return Ok(Some(content));
    }
    let scope = match context.cursor_scope(offset) {
        Ok(scope) => scope,
        Err(_) => {
            if let Ok(target) = context.navigation_target_for_identifier(offset, &span) {
                match target {
                    Target::Member(member) => {
                        let content = render::member_id(context, member, &span, range);
                        if !content.is_empty() {
                            return Ok(Some(content));
                        }
                    }
                    Target::Global(path, symbol) => {
                        if let Ok(Some(content)) = symbol::reference(context, path, symbol, range) {
                            return Ok(Some(content));
                        }
                    }
                    _ => {}
                }
            }
            return Ok(None);
        }
    };
    if !scope.available {
        return Ok(None);
    }
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
    let target = match context.navigation_target_for_identifier(offset, &span) {
        Ok(target) => target,
        Err(_) => return Ok(None),
    };
    match target {
        Target::Global(path, symbol) => {
            let mut content = symbol::reference(context, path, symbol, range)?;
            if let Some(content_ref) = content.as_mut()
                && content_ref.code.is_some()
                && let Some(instantiated) =
                    generic::instantiated(context, &scope, &span, path, symbol)
            {
                content_ref.code = Some(instantiated);
            }
            if symbol.is_method()
                && let Some(content_ref) = content.as_mut()
                && let Some(promoted) = promoted_method(context, &span, path, symbol)
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
            return Ok(content);
        }
        Target::Member(member) => {
            let content = render::member_id(context, member, &span, range);
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
        Target::Unresolved | Target::Builtin(_) | Target::CrossFile(..) => {}
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
    if let Some(hover) = render::nested_function_local(context, offset, name, range) {
        return Ok(Some(hover));
    }
    Ok(None)
}

/// Package initializers share the same source span across hover consumers.
fn value_initializer<'a>(context: &'a Context<'_>, offset: usize) -> Option<(usize, &'a str)> {
    let source = context.source();
    let stub = context.workspace().files.get(context.file())?;
    let declaration = stub.symbols.iter().find_map(|symbol| {
        let declaration = symbol.declaration()?;
        (symbol.is_value()
            && symbol.end < offset
            && declaration.start <= offset
            && offset <= declaration.end)
            .then_some(declaration)
    })?;
    let text = source.get(declaration.clone())?;
    let assign = Lexer::new(text).find(|token| token.kind == TokenKind::Assign)?;
    let base = declaration.start + assign.span.end;
    Some((base, source.get(base..declaration.end)?))
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
    let resolved = if let (Ok(s0), Ok(s1)) = (u32::try_from(span.start), u32::try_from(span.end)) {
        context.with_sema(|sema| {
            let (region, _) = sema.region_and_arena_at(file, source, s0, None)?;
            region
                .use_by_span([s0, s1])
                .or_else(|| region.selection_by_span([s0, s1]).map(|s| s.obj))
        })
    } else {
        None
    }
    .or_else(|| {
        let (base, text) = value_initializer(context, offset)?;
        if text.len() > crate::analysis::budget::MAX_BYTES {
            return None;
        }
        let r0 = u32::try_from(span.start.checked_sub(base)?).ok()?;
        let r1 = u32::try_from(span.end.checked_sub(base)?).ok()?;
        context.with_sema(|sema| {
            sema.with_expression(file, text, |region, _, _| {
                region.use_by_span([r0, r1]).or_else(|| {
                    region
                        .selection_by_span([r0, r1])
                        .map(|selection| selection.obj)
                })
            })
        })
    });
    let Some(obj) = resolved else {
        return Ok(None);
    };
    match obj {
        crate::analysis::obj::ObjectRef::Func(decl)
        | crate::analysis::obj::ObjectRef::Method(crate::analysis::obj::MethodOrigin::Concrete(
            decl,
        ))
        | crate::analysis::obj::ObjectRef::Const(crate::analysis::obj::SymbolOrigin::Package(
            decl,
        ))
        | crate::analysis::obj::ObjectRef::Var {
            origin: crate::analysis::obj::SymbolOrigin::Package(decl),
            ..
        }
        | crate::analysis::obj::ObjectRef::TypeName {
            origin: crate::analysis::obj::SymbolOrigin::Package(decl),
            ..
        } => symbol::reference(context, decl.file, decl.symbol, range),
        crate::analysis::obj::ObjectRef::Field(member)
        | crate::analysis::obj::ObjectRef::Method(crate::analysis::obj::MethodOrigin::Interface(
            member,
        )) => {
            let Some(member_decl) = member.describe(workspace) else {
                return Ok(None);
            };
            Ok(Some(render::member(context, member_decl, span, range)))
        }
        _ => Ok(None),
    }
}

/// A promoted method selection: the `// through` suffix and its outer receiver
/// type, used for gopls' enclosing-receiver link.
struct PromotedMethod {
    suffix: String,
    method_name: String,
    outer_file: std::path::PathBuf,
    outer_start: usize,
}

impl PromotedMethod {
    /// Link gopls derives from the enclosing (outermost) receiver type when the
    /// declaring receiver is not exported.
    fn outer_link(&self, context: &Context<'_>) -> Option<HoverLink> {
        if render::gopls_test_view_link(context) || render::platform_test_variant(context) {
            return None;
        }
        let workspace = context.workspace();
        let (file, stub) = workspace.files.get_key_value(&self.outer_file)?;
        let symbol = stub
            .symbols
            .iter()
            .find(|symbol| symbol.start == self.outer_start)?;
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
    span: &Range<usize>,
    method_file: &Path,
    method: &crate::index::IndexedSymbol,
) -> Option<PromotedMethod> {
    let selected = [
        u32::try_from(span.start).ok()?,
        u32::try_from(span.end).ok()?,
    ];
    let promotion = context
        .with_sema(|sema| sema.selected_promotion(context.file(), context.source(), selected))?;
    let expected = crate::analysis::obj::ObjectRef::Method(
        crate::analysis::obj::MethodOrigin::Concrete(crate::analysis::obj::DeclId {
            file: method_file,
            symbol: method,
        }),
    );
    if promotion.object != expected || promotion.through.is_empty() {
        return None;
    }
    let outer = promotion.outer?;
    Some(PromotedMethod {
        suffix: format!(" // through {}", promotion.through.join(", ")),
        method_name: method.name.clone(),
        outer_file: outer.file.to_path_buf(),
        outer_start: outer.symbol.start,
    })
}

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
