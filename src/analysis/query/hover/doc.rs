//! Bounded Go documentation extraction: comment attachment, `CommentGroup.Text`
//! semantics and resolution of Go doc links to editor locations.
use super::Context;
use crate::analysis::obj::{DeclId, MethodOrigin, ObjectRef};
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
    let spans = documentation::spans(source, target)?;
    let text = documentation::text(source, &spans)?;
    let links = doc_links(context, file, source, &spans);
    Some(Documents { text, links })
}

/// Resolve the document links of gopls' `formatDocComment`: every symbol path
/// segment maps the whole bracketed text to the file location of the target.
fn doc_links(
    context: &Context<'_>,
    file: &Path,
    source: &str,
    spans: &[Range<usize>],
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
                if let Some((path, position)) = doc_link_symbol(context, file, &progressive) {
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
) -> Option<(PathBuf, (u32, u32))> {
    let workspace = context.workspace();
    let resolver = context.resolver();
    if let Some((prefix, rest)) = name.split_once('.') {
        if let Some(import) = import_named(context, file, prefix) {
            let (receiver, member) = rest
                .split_once('.')
                .map_or((rest, None), |(receiver, member)| (receiver, Some(member)));
            if receiver.is_empty() {
                return import_position(workspace, file, import);
            }
            let mut candidates = resolver
                .import_path_symbols(file, import)
                .filter(|(_, symbol)| symbol.name == receiver);
            let (origin, symbol) = candidates.next()?;
            if candidates.next().is_some() {
                return None;
            }
            if let Some(member) = member {
                return symbol
                    .is_type()
                    .then(|| {
                        member_location(
                            context,
                            file,
                            DeclId {
                                file: origin,
                                symbol,
                            },
                            member,
                        )
                    })
                    .flatten();
            }
            let start = symbol.selection.start;
            return Some((origin.clone(), (start.line, start.character)));
        }
        let (origin, symbol) = resolver
            .package_symbol(file, prefix)
            .filter(|(_, symbol)| symbol.is_type())?;
        return member_location(
            context,
            file,
            DeclId {
                file: origin,
                symbol,
            },
            rest,
        );
    }
    resolver
        .package_symbol(file, name)
        .map(|(path, symbol)| {
            let start = symbol.selection.start;
            (path.clone(), (start.line, start.character))
        })
        .or_else(|| {
            (workspace.builtin_file().as_deref() != Some(file))
                .then(|| super::builtin::location(context, name))
                .flatten()
        })
}

fn member_location(
    context: &Context<'_>,
    file: &Path,
    receiver: DeclId<'_>,
    name: &str,
) -> Option<(PathBuf, (u32, u32))> {
    let (origin, stub) = context.workspace().files.get_key_value(receiver.file)?;
    let symbol = stub
        .symbols
        .iter()
        .find(|symbol| symbol.start == receiver.symbol.start)?;
    let member = context.with_sema(|sema| {
        sema.arena_mut().declaration_member(
            DeclId {
                file: origin,
                symbol,
            },
            file,
            name,
        )
    })?;
    match member {
        ObjectRef::Method(MethodOrigin::Concrete(declaration)) => {
            let start = declaration.symbol.selection.start;
            Some((
                declaration.file.to_path_buf(),
                (start.line, start.character),
            ))
        }
        ObjectRef::Field(identity) | ObjectRef::Method(MethodOrigin::Interface(identity))
            if !identity.builtin =>
        {
            let owner = identity.indexed(context.workspace())?.owner?;
            let source = context
                .workspace()
                .source_slice(identity.file, owner.end..owner.declaration()?.end)?;
            let start = (identity.selection[0] as usize).checked_sub(owner.end)?;
            let end = (identity.selection[1] as usize).checked_sub(owner.end)?;
            source.get(start..end)?;
            let relative = crate::text::SourceMap::new(&source).position(start);
            let line = owner.selection.end.line.checked_add(relative.line)?;
            let character = if relative.line == 0 {
                owner
                    .selection
                    .end
                    .character
                    .checked_add(relative.character)?
            } else {
                relative.character
            };
            Some((identity.file.to_path_buf(), (line, character)))
        }
        _ => None,
    }
}

fn import_named<'a>(context: &Context<'a>, file: &Path, name: &str) -> Option<&'a str> {
    let resolver = context.resolver();
    let stub = context.workspace().files.get(file)?;
    stub.imports
        .iter()
        .find(|import| {
            !import.is_dot()
                && !import.is_blank()
                && resolver.import_name(file, import) == Some(name)
        })
        .or_else(|| {
            stub.imports.iter().find(|import| {
                !import.is_blank()
                    && resolver.declared_package_name(file, &import.path) == Some(name)
            })
        })
        .map(|import| import.path.as_str())
}

fn import_position(
    workspace: &Workspace,
    file: &Path,
    import: &str,
) -> Option<(PathBuf, (u32, u32))> {
    let source = workspace.source(file)?;
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
    None
}
