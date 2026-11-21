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
    context: &mut Context<'_>,
    offset: usize,
) -> Result<Option<HoverContent>, (i32, String)> {
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
    let (span, range) = {
        let source = context.source();
        let Some(span) = identifier_at(source, offset) else {
            return Ok(None);
        };
        let map = SourceMap::new(source);
        let range = SourceRange::new(&map, span.clone());
        (span, range)
    };
    if let Some(content) = member::checked(context, &span, range) {
        return Ok(Some(content));
    }
    // A declaration name always takes precedence over a use of the same name.
    if let Some(symbol) = context.workspace().files.get(file).and_then(|stub| {
        stub.symbols
            .iter()
            .find(|symbol| symbol.start == span.start && symbol.end == span.end)
    }) {
        return symbol::declare(context, file, symbol, range);
    }
    if let (Ok(s0), Ok(s1)) = (u32::try_from(span.start), u32::try_from(span.end)) {
        let selected = [s0, s1];
        enum LocalMatch<'a> {
            Object(crate::analysis::sema::SemanticObject<'a>),
            Binding(crate::analysis::sema::LocalBindingView),
        }
        let local_match = {
            let (doc, sema) = context.split();
            sema.region_at(
                doc.file(),
                doc.source(),
                selected[0],
                crate::analysis::sema::RegionMode::Full,
            )
            .and_then(|view| {
                if let Some(site) = view
                    .symbol_at(selected[0])
                    .filter(|site| site.span == selected)
                {
                    return site.obj.is_local().then_some(LocalMatch::Object(site.obj));
                }
                view.local_binding_at_span(selected)
                    .filter(|b| {
                        !matches!(b.kind, crate::analysis::sema::LocalDeclKind::TypeSwitchVar)
                    })
                    .map(LocalMatch::Binding)
            })
        };
        match local_match {
            Some(LocalMatch::Object(obj)) => {
                if let Some(content) = render::local_object(context, obj, &span, range) {
                    return Ok(Some(content));
                }
            }
            Some(LocalMatch::Binding(b)) => {
                let is_param = matches!(b.kind, crate::analysis::sema::LocalDeclKind::Param);
                if let Some(content) =
                    render::local_by_parts(context, Some(b.id), b.kind, b.def_span, is_param, range)
                {
                    return Ok(Some(content));
                }
            }
            None => {}
        }
    }
    if is_package_clause_name(context.source(), span.start) {
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
                && let Some(instantiated) = generic::instantiated(context, &span, path, symbol)
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
        Target::Local(_) | Target::Unresolved | Target::Builtin(_) | Target::CrossFile(..) => {}
    }
    if let Some(hover) = package_initializer_target(context, offset, &span, range)? {
        return Ok(Some(hover));
    }
    if let Some(hover) = package::qualifier(context, &span, range) {
        return Ok(Some(hover));
    }
    let name_buf = context.source()[span.clone()].to_owned();
    if let Some(hover) = builtin::hover(context, &name_buf, range) {
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
    context: &mut Context<'_>,
    offset: usize,
    span: &Range<usize>,
    range: super::SourceRange,
) -> Result<Option<HoverContent>, (i32, String)> {
    let workspace = context.workspace();
    let file = context.file();
    let resolved = if let (Ok(s0), Ok(s1)) = (u32::try_from(span.start), u32::try_from(span.end)) {
        let (doc, sema) = context.split();
        sema.region_at(
            doc.file(),
            doc.source(),
            s0,
            crate::analysis::sema::RegionMode::Full,
        )
        .and_then(|view| view.use_or_selection([s0, s1]))
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
        let text = text.to_owned();
        context.with_sema(|sema| {
            sema.with_expression(file, &text, |view, _| view.use_or_selection([r0, r1]))
        })
    });
    let Some(obj) = resolved else {
        return Ok(None);
    };
    match obj {
        crate::analysis::sema::ObjectRef::Func(decl)
        | crate::analysis::sema::ObjectRef::Method(
            crate::analysis::sema::MethodOrigin::Concrete(decl),
        )
        | crate::analysis::sema::ObjectRef::Const(crate::analysis::sema::SymbolOrigin::Package(
            decl,
        ))
        | crate::analysis::sema::ObjectRef::Var {
            origin: crate::analysis::sema::SymbolOrigin::Package(decl),
            ..
        }
        | crate::analysis::sema::ObjectRef::TypeName {
            origin: crate::analysis::sema::SymbolOrigin::Package(decl),
            ..
        } => symbol::reference(context, decl.file, decl.symbol, range),
        crate::analysis::sema::ObjectRef::Field(member)
        | crate::analysis::sema::ObjectRef::Method(
            crate::analysis::sema::MethodOrigin::Interface(member),
        ) => {
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
    context: &mut Context<'_>,
    span: &Range<usize>,
    method_file: &Path,
    method: &crate::index::IndexedSymbol,
) -> Option<PromotedMethod> {
    let selected = [
        u32::try_from(span.start).ok()?,
        u32::try_from(span.end).ok()?,
    ];
    let (doc, sema) = context.split();
    let promotion = sema.selected_promotion(doc.file(), doc.source(), selected)?;
    let expected = crate::analysis::sema::ObjectRef::Method(
        crate::analysis::sema::MethodOrigin::Concrete(crate::analysis::sema::DeclId {
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
