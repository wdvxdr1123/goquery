//! Bounded Go documentation extraction: comment attachment, `CommentGroup.Text`
//! semantics and resolution of Go doc links to editor locations.
use super::Context;
use crate::analysis::query::documentation;
use crate::syntax::{Lexer, TokenKind};
use crate::workspace::{self, Workspace};
pub(super) use documentation::Target;
use std::ops::Range;
use std::path::{Path, PathBuf};

const MAX_DOC_LINKS: usize = 128;

pub(super) struct Documents {
    pub(super) text: String,
    pub(super) links: Vec<(String, String)>,
}

/// Extract the attached documentation group and resolve doc-link targets.
pub(super) fn attached(
    context: &Context<'_>,
    file: &Path,
    source: &str,
    target: Target,
) -> Option<Documents> {
    attached_in_view(context, file, source, target, None)
}

/// Same as [`attached`], resolving links in a detached document's build view.
pub(super) fn attached_in_view(
    context: &Context<'_>,
    file: &Path,
    source: &str,
    target: Target,
    view: Option<&crate::buildconstraint::Target>,
) -> Option<Documents> {
    let spans = documentation::spans(source, target)?;
    let text = documentation::text(source, &spans)?;
    let links = doc_links(context, file, source, &spans, view);
    Some(Documents { text, links })
}

/// Resolve the document links of gopls' `formatDocComment`: every symbol path
/// segment maps the whole bracketed text to the file location of the target.
fn doc_links(
    context: &Context<'_>,
    file: &Path,
    source: &str,
    spans: &[Range<usize>],
    view: Option<&crate::buildconstraint::Target>,
) -> Vec<(String, String)> {
    let mut links: Vec<(String, String)> = Vec::new();
    for span in spans {
        let Some(comment) = source.get(span.clone()) else {
            continue;
        };
        for (bracket, inner) in bracket_links(comment) {
            let name = inner.trim_start_matches('*');
            let parts: Vec<&str> = name.split('.').collect();
            if parts.len() > 3 || parts.iter().any(|part| !identifier(part)) {
                continue;
            }
            let mut resolved = None;
            let mut progressive = String::new();
            for part in &parts {
                if !progressive.is_empty() {
                    progressive.push('.');
                }
                progressive.push_str(part);
                if let Some((path, position)) = doc_link_symbol(context, file, &progressive, view) {
                    resolved = Some((path, position));
                }
            }
            if let Some((path, (line, character))) = resolved {
                if links.len() == MAX_DOC_LINKS {
                    return links;
                }
                let url = format!(
                    "{}#{},{}",
                    workspace::uri(&path),
                    line as u64 + 1,
                    character as u64 + 1
                );
                let key = bracket.to_owned();
                links.retain(|(existing, _)| existing != &key);
                links.push((key, url));
            }
        }
    }
    links
}

fn identifier(text: &str) -> bool {
    let mut chars = text.chars();
    chars
        .next()
        .is_some_and(|first| first.is_alphabetic() || first == '_')
        && chars.all(|ch| ch.is_alphanumeric() || ch == '_')
}

/// Bracket ranges in one raw comment token, skipping URL link definitions.
fn bracket_links(comment: &str) -> Vec<(String, String)> {
    let bytes = comment.as_bytes();
    let mut result = Vec::new();
    let mut start = None;
    for index in 0..bytes.len() {
        match bytes[index] {
            b'[' => start = Some(index),
            b']' => {
                if let Some(begin) = start.take() {
                    let inner = &comment[begin + 1..index];
                    if !inner.is_empty() && !url_definition(&comment[index + 1..]) {
                        result.push((inner.to_owned(), inner.to_owned()));
                    }
                }
            }
            _ => {}
        }
    }
    result
}

fn url_definition(rest: &str) -> bool {
    let Some(rest) = rest.strip_prefix(':') else {
        return false;
    };
    let rest = rest.trim_start();
    let Some((scheme, _)) = rest.split_once("://") else {
        return false;
    };
    ["file", "ftp", "gopher", "http", "https", "mailto", "nntp"].contains(&scheme)
}

/// Resolve a doc-link symbol path in the declaring file the way gopls does:
/// imported package prefix, then package-level symbol, then a member.
fn doc_link_symbol(
    context: &Context<'_>,
    file: &Path,
    name: &str,
    view: Option<&crate::buildconstraint::Target>,
) -> Option<(PathBuf, (u32, u32))> {
    let workspace = context.workspace();
    if let Some((prefix, rest)) = name.split_once('.') {
        if let Some(import) = import_named(workspace, file, prefix) {
            if let Some((recv, member)) = rest.split_once('.') {
                return member_location(workspace, file, &format!("{prefix}.{recv}"), member);
            }
            if rest.is_empty() {
                return import_position(workspace, file, &import)
                    .map(|position| (position.0, position.1));
            }
            let resolved = workspace
                .imported_symbol_iter(file, prefix)
                .find(|(_, symbol)| symbol.name == rest && !symbol.name.is_empty())
                .map(|(path, symbol)| {
                    (
                        path.clone(),
                        (
                            symbol.selection.start.line,
                            symbol.selection.start.character,
                        ),
                    )
                });
            if let (Some(view), Some((path, _))) = (view, resolved.as_ref())
                && !workspace.file_matches_target(path, view)
            {
                let package = workspace
                    .files
                    .get(path)
                    .map(|stub| stub.package.clone())
                    .or_else(|| workspace.declared_package_name(&import).map(str::to_owned));
                if let (Some(package), Some(directory)) = (package, path.parent())
                    && let Some(location) =
                        view_location(workspace, view, directory, &package, rest)
                {
                    return Some(location);
                }
            }
            return resolved;
        }
        if let Some((_, symbol)) = workspace
            .unqualified_symbol(file, prefix)
            .filter(|(_, symbol)| symbol.is_type())
        {
            return member_location(workspace, file, &symbol.name, rest);
        }
        return None;
    }
    let package = workspace
        .files
        .get(file)
        .map(|stub| stub.package.clone())
        .unwrap_or_default();
    let resolved = workspace
        .unqualified_symbol(file, name)
        .map(|(path, symbol)| {
            let start = symbol.selection.start;
            (path.clone(), (start.line, start.character))
        });
    if let Some(view) = view {
        if let Some((path, _)) = resolved.as_ref()
            && workspace.file_matches_target(path, view)
        {
            return resolved;
        }
        if let Some(directory) = file.parent()
            && let Some(location) = view_location(workspace, view, directory, &package, name)
        {
            return Some(location);
        }
    }
    resolved.or_else(|| {
        if workspace.builtin_file().as_deref() == Some(file) {
            None
        } else {
            super::builtin::location(context, name)
        }
    })
}

fn view_location(
    workspace: &Workspace,
    target: &crate::buildconstraint::Target,
    directory: &Path,
    package: &str,
    name: &str,
) -> Option<(PathBuf, (u32, u32))> {
    let location =
        super::view::package_symbol(workspace, Some(target), directory, package, name, false)?;
    Some((location.path, (location.line, location.character)))
}

fn member_location(
    workspace: &Workspace,
    file: &Path,
    receiver: &str,
    member: &str,
) -> Option<(PathBuf, (u32, u32))> {
    let member = workspace.scoped_receiver_member(file, file, receiver, member)?;
    match member {
        crate::analysis::method_set::ReceiverMember::Method(path, symbol) => {
            let start = symbol.selection.start;
            Some((path.clone(), (start.line, start.character)))
        }
        crate::analysis::method_set::ReceiverMember::Declaration(declaration) => {
            let data = workspace.member_data(declaration)?;
            Some((
                declaration.file.clone(),
                (data.selection.start.line, data.selection.start.character),
            ))
        }
    }
}

fn import_named(workspace: &Workspace, file: &Path, name: &str) -> Option<String> {
    let stub = workspace.files.get(file)?;
    stub.imports
        .iter()
        .find(|import| {
            !import.is_dot() && !import.is_blank() && workspace.import_name(import) == Some(name)
        })
        .map(|import| import.path.clone())
        .or_else(|| {
            stub.imports
                .iter()
                .find(|import| {
                    !import.is_blank()
                        && workspace.declared_package_name(&import.path) == Some(name)
                })
                .map(|import| import.path.clone())
        })
}

/// Location of a package import's `PkgName` object: the alias if written,
/// otherwise the import path literal.
fn import_position(
    workspace: &Workspace,
    file: &Path,
    import: &str,
) -> Option<(PathBuf, (u32, u32))> {
    let source = workspace.source(file)?;
    let expected_alias = workspace
        .files
        .get(file)?
        .imports
        .iter()
        .find(|candidate| candidate.path == import)
        .and_then(|candidate| candidate.explicit_name().map(str::to_owned));
    let mut tokens = Lexer::new(&source).peekable();
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
            let mut alias = None;
            let first = tokens.next()?;
            if first.kind == TokenKind::RParen {
                break;
            }
            let path_token = if first.kind == TokenKind::String {
                Some(first)
            } else {
                alias = Some(first.clone());
                tokens.next()
            };
            if let Some(path) = path_token.filter(|token| token.kind == TokenKind::String) {
                let path_text = crate::syntax::decode_import_path(&source[path.span.clone()])?;
                if path_text == import {
                    let offset = alias
                        .as_ref()
                        .map_or(path.span.start, |alias| alias.span.start);
                    let map = crate::text::SourceMap::new(&source);
                    let position = map.position(offset);
                    return Some((
                        workspace.files.get_key_value(file)?.0.clone(),
                        (position.line, position.character),
                    ));
                }
            }
            if !grouped {
                break;
            }
        }
    }
    let _ = expected_alias;
    None
}
