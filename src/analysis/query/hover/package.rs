//! Package-name descriptions: imported package references and package clauses.
use super::content::HoverContent;
use crate::analysis::query::Context;
use crate::analysis::sema::Object;
use crate::syntax::{Lexer, TokenKind};
use crate::workspace::{self, Workspace};
use std::ops::Range;
use std::path::{Path, PathBuf};

const MAX_PACKAGE_FILES: usize = 256;

/// Hover over the package name in the package clause.
pub(super) fn clause(context: &mut Context<'_>, range: super::SourceRange) -> Option<HoverContent> {
    let workspace = context.workspace();
    let file = context.file();
    let name = workspace.files.get(file)?.package.clone();
    let mut content = HoverContent::new(range);
    content.code = Some(format!("package {name}"));
    if let Some(documents) = package_doc(context, file, &name) {
        content.doc = Some(documents.text);
        content.doc_links = documents.links;
    }
    content.footer = footer(context, file);
    Some(content)
}

/// Hover over an explicit import alias identifier (`str "strings"`).
pub(super) fn import_alias(
    context: &mut Context<'_>,
    span: &Range<usize>,
    range: super::SourceRange,
) -> Option<HoverContent> {
    let workspace = context.workspace();
    let file = context.file();
    let (name, import_path, path_span) = {
        let source = context.source();
        let name = source.get(span.clone())?.to_owned();
        let import = workspace
            .files
            .get(file)?
            .imports
            .iter()
            .find(|import| import.explicit_name() == Some(&name))?;
        if import.is_blank() || import.is_dot() {
            return None;
        }
        // gopls reports the import path literal's range for import-spec hover.
        let path_span = alias_path_span(source, span.start, &import.path)?;
        let range = super::SourceRange::new(&crate::text::SourceMap::new(source), path_span);
        (name, import.path.clone(), range)
    };
    let package = context
        .resolver()
        .declared_package_name(file, &import_path)
        .map(str::to_owned)
        .or_else(|| import_path.rsplit('/').next().map(str::to_owned))?;
    let mut content = HoverContent::new(path_span);
    let _ = range;
    content.code = Some(format!("package {package}"));
    if let Some(documents) = imported_package_doc(context, file, &import_path, &package, None) {
        content.doc = Some(documents.text);
        content.doc_links = documents.links;
    }
    // Import-spec hover does not carry the pkg.go.dev link gopls adds to
    // package-name objects.
    let _ = name;
    Some(content)
}

/// Source span of the import path literal whose alias starts at `offset`.
fn alias_path_span(source: &str, offset: usize, import_path: &str) -> Option<Range<usize>> {
    let mut found = None;
    crate::syntax::visit_import_spans(source, |spec| {
        if found.is_none()
            && spec.alias.is_some_and(|alias| alias.start == offset)
            && source
                .get(spec.path.clone())
                .and_then(crate::syntax::decode_import_path)
                .is_some_and(|decoded| decoded == import_path)
        {
            found = Some(spec.path);
        }
    });
    found
}

/// Hover over an imported package's qualifier identifier.
pub(super) fn qualifier(
    context: &mut Context<'_>,
    span: &Range<usize>,
    range: super::SourceRange,
) -> Option<HoverContent> {
    let file = context.file();
    let (name, import_path, target_dir) = {
        let sel = u32::try_from(span.start)
            .ok()
            .zip(u32::try_from(span.end).ok())
            .map(|(s, e)| [s, e]);
        let (doc, sema) = context.split();
        let source = doc.source();
        if !qualified(source, span.end) {
            return None;
        }
        let name = source.get(span.clone())?.to_owned();
        let resolved_pkg = sel
            .and_then(|sel| sema.resolve_navigation_offset(doc.file(), source, sel))
            .and_then(|obj| match obj {
                Object::PkgName {
                    import_path_span,
                    target_dir,
                } => {
                    let decoded = (import_path_span[0] < import_path_span[1])
                        .then(|| {
                            source
                                .get(import_path_span[0] as usize..import_path_span[1] as usize)
                                .and_then(crate::syntax::decode_import_path)
                        })
                        .flatten();
                    let dir =
                        (!target_dir.as_os_str().is_empty()).then(|| target_dir.to_path_buf());
                    Some((decoded, dir))
                }
                _ => None,
            });
        match resolved_pkg {
            Some((Some(import_path), dir)) => (name, import_path, dir),
            Some((None, dir)) => {
                let stub = doc.workspace().files.get(file)?;
                let import = stub.imports.iter().find(|import| {
                    !import.is_dot()
                        && !import.is_blank()
                        && doc.resolver().import_name(file, import) == Some(&name)
                })?;
                (name, import.path.clone(), dir)
            }
            None => {
                let stub = doc.workspace().files.get(file)?;
                let import = stub.imports.iter().find(|import| {
                    !import.is_dot()
                        && !import.is_blank()
                        && doc.resolver().import_name(file, import) == Some(&name)
                })?;
                (name, import.path.clone(), None)
            }
        }
    };
    let workspace = context.workspace();
    let package = context
        .resolver()
        .declared_package_name(file, &import_path)
        .map(str::to_owned)
        .or_else(|| import_path.rsplit('/').next().map(str::to_owned))?;
    let mut content = HoverContent::new(range);
    content.code = Some(format!("package {package}"));
    if let Some(documents) =
        imported_package_doc(context, file, &import_path, &package, target_dir.as_deref())
    {
        content.doc = Some(documents.text);
        content.doc_links = documents.links;
    }
    if !super::render::gopls_test_view_link(context)
        && !super::render::platform_test_variant(context)
    {
        content.link = Some(super::HoverLink {
            symbol: name.to_owned(),
            path: versioned_import(workspace, &import_path),
            anchor: String::new(),
        });
    }
    Some(content)
}

/// Dependency imports in pkg.go.dev links carry their module version.
fn versioned_import(workspace: &Workspace, import_path: &str) -> String {
    for (root, module) in &workspace.modules {
        let Some(rest) = import_path.strip_prefix(module.as_str()) else {
            continue;
        };
        if !rest.is_empty() && !rest.starts_with('/') {
            continue;
        }
        let Some((_, version)) = root
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.rsplit_once('@'))
        else {
            return import_path.to_owned();
        };
        return format!("{module}@{version}{rest}");
    }
    import_path.to_owned()
}

fn qualified(source: &str, end: usize) -> bool {
    source
        .get(end..)
        .is_some_and(|rest| rest.trim_start().starts_with('.'))
}

/// First production file with a package documentation comment.
fn package_doc(
    context: &mut Context<'_>,
    file: &Path,
    package: &str,
) -> Option<super::doc::Documents> {
    let workspace = context.workspace();
    let directory = file.parent()?;
    let external = package.ends_with("_test");
    let internal_test = file
        .file_name()
        .is_some_and(|name| name.to_string_lossy().ends_with("_test.go"));
    let mut files: Vec<&PathBuf> = workspace
        .files
        .range::<Path, _>((
            std::ops::Bound::Included(directory),
            std::ops::Bound::Unbounded,
        ))
        .take_while(|(path, _)| path.starts_with(directory))
        .filter(|(path, stub)| {
            path.parent() == Some(directory)
                && stub.package == package
                && context.resolver().file_visible(path)
                && (external
                    || crate::workspace::production_file(path)
                    || (internal_test
                        && path
                            .file_name()
                            .is_some_and(|name| name.to_string_lossy().ends_with("_test.go"))))
        })
        .map(|(path, _)| path)
        .take(MAX_PACKAGE_FILES)
        .collect();
    files.sort();
    for candidate in files {
        let Some(source) = header_source(workspace, candidate) else {
            continue;
        };
        let Some(documents) = package_comment(context, candidate, &source) else {
            continue;
        };
        if !documents.text.is_empty() {
            return Some(documents);
        }
    }
    None
}

fn package_comment(
    context: &mut Context<'_>,
    file: &Path,
    source: &str,
) -> Option<super::doc::Documents> {
    let mut tokens = Lexer::new(source);
    let package = tokens
        .next()
        .filter(|token| token.kind == TokenKind::Package)?;
    let _ = package;
    let name = tokens
        .next()
        .filter(|token| token.kind == TokenKind::Ident)?;
    let _ = name;
    super::doc::attached(
        context,
        file,
        source,
        super::doc::Target {
            start: package.span.start,
            end: package.span.end,
            declaration: None,
            trailing: false,
        },
    )
}

/// Package documentation for an imported package, searched in file-path order.
fn imported_package_doc(
    context: &mut Context<'_>,
    _from: &Path,
    import_path: &str,
    package: &str,
    target_dir: Option<&Path>,
) -> Option<super::doc::Documents> {
    let workspace = context.workspace();
    let directory = target_dir
        .map(Path::to_path_buf)
        .or_else(|| imported_directory(context, import_path))?;
    let external = package.ends_with("_test");
    let files: Vec<PathBuf> = workspace
        .files
        .range::<Path, _>((
            std::ops::Bound::Included(directory.as_path()),
            std::ops::Bound::Unbounded,
        ))
        .take_while(|(path, _)| path.starts_with(&directory))
        .filter(|(path, stub)| {
            path.parent() == Some(directory.as_path())
                && stub.package == package
                && context.resolver().file_visible(path)
                && (external || crate::workspace::production_file(path))
        })
        .map(|(path, _)| path.clone())
        .take(MAX_PACKAGE_FILES)
        .collect();
    for path in &files {
        let Some(source) = header_source(context.workspace(), path) else {
            continue;
        };
        if let Some(documents) = package_comment(context, path, &source)
            && !documents.text.is_empty()
        {
            return Some(documents);
        }
    }
    None
}

/// Bounded header read: package documentation lives before declarations.
fn header_source(workspace: &Workspace, file: &Path) -> Option<String> {
    workspace.source_prefix(file, 64 * 1024)
}

fn imported_directory(context: &Context<'_>, import_path: &str) -> Option<PathBuf> {
    let workspace = context.workspace();
    // Module roots map import prefixes to on-disk directories.
    let mut directories = Vec::new();
    for (root, module) in &workspace.modules {
        let suffix = if module == "std" {
            Some(import_path.to_owned())
        } else if import_path == module {
            Some(String::new())
        } else {
            import_path
                .strip_prefix(module.as_str())
                .and_then(|rest| rest.strip_prefix('/'))
                .map(str::to_owned)
        };
        let Some(suffix) = suffix else {
            continue;
        };
        let candidate = if suffix.is_empty() {
            root.clone()
        } else {
            root.join(&suffix)
        };
        if workspace
            .files
            .range::<Path, _>((
                std::ops::Bound::Included(candidate.as_path()),
                std::ops::Bound::Unbounded,
            ))
            .take_while(|(path, _)| path.starts_with(&candidate))
            .any(|(path, _)| {
                path.parent() == Some(candidate.as_path()) && context.resolver().file_visible(path)
            })
        {
            directories.push(candidate);
        }
    }
    directories.sort();
    directories.into_iter().next()
}

/// Package attributes footer: path, module and effective language version.
fn footer(context: &Context<'_>, file: &Path) -> Option<String> {
    let workspace = context.workspace();
    let mut attrs = Vec::new();
    let package = workspace.files.get(file)?.package.clone();
    let external_test = file
        .file_name()
        .is_some_and(|name| name.to_string_lossy().ends_with("_test.go"))
        && package.ends_with("_test");
    if external_test {
        // go/packages names an external test package `<importpath>_test`.
        if let Some(path) = workspace.import_path(file) {
            attrs.push(("Package path", format!("{path}_test")));
        } else {
            attrs.push(("Package path", package.clone()));
        }
    } else if let Some(path) = workspace.import_path(file) {
        attrs.push(("Package path", path));
    }
    let module_root = workspace
        .modules
        .iter()
        .filter(|(root, _)| file.starts_with(root))
        .max_by_key(|(root, _)| root.components().count());
    // gopls' GOROOT and Go command views carry no module metadata.
    if let Some((_root, module)) = module_root
        && module != "std"
        && module != "cmd"
    {
        attrs.push(("Module", module.clone()));
    }
    if let Some(version) = workspace.package_go_version(file) {
        attrs.push(("Language version", format!("go1.{version}")));
    }
    let _ = workspace::uri;
    (!attrs.is_empty()).then(|| {
        attrs
            .into_iter()
            .map(|(title, value)| format!(" - {title}: {value}"))
            .collect::<Vec<_>>()
            .join("\n")
    })
}
