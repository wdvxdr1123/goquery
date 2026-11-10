//! Package-name descriptions: imported package references and package clauses.
use super::content::HoverContent;
use crate::analysis::query::Context;
use crate::syntax::{Lexer, TokenKind};
use crate::workspace::{self, Workspace};
use std::ops::Range;
use std::path::{Path, PathBuf};

const MAX_PACKAGE_FILES: usize = 256;
const MAX_SOURCE_BYTES: usize = 4 * 1024 * 1024;

/// Hover over the package name in the package clause.
pub(super) fn clause(context: &Context<'_>, range: super::SourceRange) -> Option<HoverContent> {
    let workspace = context.workspace();
    let file = context.file();
    let name = workspace.files.get(file)?.package.clone();
    let mut content = HoverContent::new(range);
    content.code = Some(format!("package {name}"));
    if let Some(documents) = package_doc(workspace, file, &name) {
        content.doc = Some(documents.text);
        content.doc_links = documents.links;
    }
    content.footer = footer(context, file);
    Some(content)
}

/// Hover over an explicit import alias identifier (`str "strings"`).
pub(super) fn import_alias(
    context: &Context<'_>,
    span: &Range<usize>,
    range: super::SourceRange,
) -> Option<HoverContent> {
    let workspace = context.workspace();
    let file = context.file();
    let source = context.source();
    let name = source.get(span.clone())?;
    let import = workspace
        .files
        .get(file)?
        .imports
        .iter()
        .find(|import| import.explicit_name() == Some(name))?
        .clone();
    if import.is_blank() || import.is_dot() {
        return None;
    }
    // gopls reports the import path literal's range for import-spec hover.
    let path_span = alias_path_span(source, span.start, &import.path)?;
    let package = workspace
        .declared_package_name(&import.path)
        .map(str::to_owned)
        .or_else(|| import.path.rsplit('/').next().map(str::to_owned))?;
    let mut content = HoverContent::new(super::SourceRange::new(
        &crate::text::SourceMap::new(source),
        path_span,
    ));
    let _ = range;
    content.code = Some(format!("package {package}"));
    if let Some(documents) = imported_package_doc(workspace, file, &import.path, &package) {
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
    let mut tokens = Lexer::new(source).peekable();
    while let Some(token) = tokens.next() {
        if token.kind != TokenKind::Import {
            continue;
        }
        let grouped = tokens
            .peek()
            .is_some_and(|token| token.kind == TokenKind::LParen);
        if grouped {
            tokens.next();
        }
        loop {
            let first = tokens.next()?;
            if first.kind == TokenKind::RParen {
                break;
            }
            if first.kind == TokenKind::Semicolon {
                continue;
            }
            if first.kind == TokenKind::String {
                if !grouped {
                    break;
                }
                continue;
            }
            let Some(path) = tokens
                .next()
                .filter(|token| token.kind == TokenKind::String)
            else {
                break;
            };
            if first.span.start == offset
                && crate::syntax::decode_import_path(&source[path.span.clone()])
                    .is_some_and(|decoded| decoded == import_path)
            {
                return Some(path.span);
            }
            if !grouped {
                break;
            }
        }
    }
    None
}

/// Hover over an imported package's qualifier identifier.
pub(super) fn qualifier(
    context: &Context<'_>,
    span: &Range<usize>,
    range: super::SourceRange,
) -> Option<HoverContent> {
    let workspace = context.workspace();
    let file = context.file();
    let source = context.source();
    if !qualified(source, span.end) {
        return None;
    }
    let name = source.get(span.clone())?;
    let stub = workspace.files.get(file)?;
    let import = stub.imports.iter().find(|import| {
        !import.is_dot() && !import.is_blank() && workspace.import_name(import) == Some(name)
    })?;
    let import_path = import.path.clone();
    let package = workspace
        .declared_package_name(&import_path)
        .map(str::to_owned)
        .or_else(|| import_path.rsplit('/').next().map(str::to_owned))?;
    let mut content = HoverContent::new(range);
    content.code = Some(format!("package {package}"));
    if let Some(documents) = imported_package_doc(workspace, file, &import_path, &package) {
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
fn package_doc(workspace: &Workspace, file: &Path, package: &str) -> Option<super::doc::Documents> {
    let view = workspace.orphan_view_target(file);
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
        let Some(documents) = package_comment(workspace, candidate, &source, view.as_ref()) else {
            continue;
        };
        if !documents.text.is_empty() {
            return Some(documents);
        }
    }
    None
}

fn package_comment(
    workspace: &Workspace,
    file: &Path,
    source: &str,
    view: Option<&crate::buildconstraint::Target>,
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
    let context = Context::borrowed(workspace, file, source);
    super::doc::attached_in_view(
        &context,
        file,
        source,
        super::doc::Target {
            start: package.span.start,
            end: package.span.end,
            declaration: None,
            trailing: false,
        },
        view,
    )
}

/// Package documentation for an imported package, searched in file-path order.
fn imported_package_doc(
    workspace: &Workspace,
    from: &Path,
    import_path: &str,
    package: &str,
) -> Option<super::doc::Documents> {
    let directory = imported_directory(workspace, from, import_path)?;
    let view = workspace.orphan_view_target(from);
    let external = package.ends_with("_test");
    for (path, _stub) in workspace
        .files
        .range::<Path, _>((
            std::ops::Bound::Included(directory.as_path()),
            std::ops::Bound::Unbounded,
        ))
        .take_while(|(path, _)| path.starts_with(&directory))
        .filter(|(path, stub)| {
            path.parent() == Some(directory.as_path())
                && stub.package == package
                && (external || crate::workspace::production_file(path))
        })
        .take(MAX_PACKAGE_FILES)
    {
        let Some(source) = header_source(workspace, path) else {
            continue;
        };
        if let Some(documents) = package_comment(workspace, path, &source, view.as_ref())
            && !documents.text.is_empty()
        {
            return Some(documents);
        }
    }
    None
}

/// Bounded header read: package documentation lives before declarations.
fn header_source(workspace: &Workspace, file: &Path) -> Option<String> {
    let source = workspace
        .source(file)
        .filter(|text| text.len() <= MAX_SOURCE_BYTES)?;
    let end = source
        .char_indices()
        .take_while(|(offset, _)| *offset < 64 * 1024)
        .last()
        .map(|(offset, ch)| offset + ch.len_utf8())
        .unwrap_or(0);
    Some(source.get(..end).map(str::to_owned).unwrap_or(source))
}

fn imported_directory(workspace: &Workspace, _from: &Path, import_path: &str) -> Option<PathBuf> {
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
        if candidate.is_dir() {
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
    if let Some((root, _)) = module_root {
        let manifest = std::fs::read_to_string(root.join("go.mod"))
            .ok()
            .and_then(|source| crate::gomod::parse(&source).ok());
        if let Some(version) = manifest.and_then(|manifest| manifest.go)
            && let Some(language) = language_version(&version)
        {
            attrs.push(("Language version", language));
        }
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

fn language_version(version: &str) -> Option<String> {
    let mut parts = version.split('.');
    let major = parts.next()?;
    if major != "1" {
        return None;
    }
    let minor = parts.next()?;
    let digits: String = minor.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return None;
    }
    Some(format!("go1.{digits}"))
}
