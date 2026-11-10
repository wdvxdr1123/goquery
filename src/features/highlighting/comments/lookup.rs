//! Lookup of doc-link names in package scope and imports.
//!
//! Mirrors `tokenVisitor.comment`'s `lookupObjects`: an optional import
//! qualifier, then either a package-scope name or `Type.Method`. Unresolved or
//! unexported names produce no tokens, leaving the comment untouched.
use super::{exported, style};
use crate::workspace::Workspace;
use std::path::Path;

pub(super) struct Found {
    pub(super) name: String,
    pub(super) kind: super::super::config::SemanticType,
    pub(super) modifiers: u64,
}

pub(super) fn lookup(workspace: &Workspace, file: &Path, name: &str) -> Vec<Found> {
    let parts: Vec<&str> = name.split('.').collect();
    if parts.is_empty() || parts.len() > 3 {
        return Vec::new();
    }
    if parts.len() > 1 {
        if import_path(workspace, file, parts[0]).is_some() {
            return imported(workspace, file, parts[0], &parts[1..]);
        }
        // `T.M` resolves in the current package.
        return current_method(workspace, file, parts[0], parts[1], parts.len() == 3);
    }
    let Some((_, symbol)) = workspace
        .package_symbols(file)
        .find(|(_, symbol)| symbol.name == parts[0] && symbol.receiver.is_none())
    else {
        return Vec::new();
    };
    if !exported(&symbol.name) {
        return Vec::new();
    }
    let (kind, modifiers) = style(symbol);
    vec![Found {
        name: parts[0].to_owned(),
        kind,
        modifiers,
    }]
}

/// Resolves an import alias to its canonical import path.
fn import_path<'a>(workspace: &'a Workspace, file: &Path, alias: &str) -> Option<&'a str> {
    workspace.files.get(file).and_then(|stub| {
        stub.imports
            .iter()
            .find(|import| {
                !import.is_dot()
                    && !import.is_blank()
                    && workspace.import_name(import) == Some(alias)
            })
            .map(|import| import.path.as_str())
    })
}

fn imported(workspace: &Workspace, file: &Path, alias: &str, rest: &[&str]) -> Vec<Found> {
    let symbols: Vec<_> = workspace.imported_symbols(file, alias);
    let mut objects = vec![Found {
        name: alias.to_owned(),
        kind: super::super::config::SemanticType::Namespace,
        modifiers: 0,
    }];
    if rest.len() == 1 {
        let name = rest[0];
        let Some((_, symbol)) = symbols
            .iter()
            .find(|(_, symbol)| symbol.name == name && symbol.receiver.is_none())
        else {
            return Vec::new();
        };
        if !exported(name) {
            return Vec::new();
        }
        let (kind, modifiers) = style(symbol);
        objects.push(Found {
            name: name.to_owned(),
            kind,
            modifiers,
        });
        return objects;
    }
    if rest.len() != 2 {
        return Vec::new();
    }
    let (receiver_name, method_name) = (rest[0], rest[1]);
    let receiver = symbols.iter().find(|(_, symbol)| {
        symbol.name == receiver_name && symbol.receiver.is_none() && symbol.is_type()
    });
    let Some((_, receiver)) = receiver else {
        return Vec::new();
    };
    let method = symbols.iter().find(|(_, symbol)| {
        symbol.name == method_name && symbol.receiver.as_deref() == Some(receiver_name)
    });
    let Some((_, method)) = method else {
        return Vec::new();
    };
    let (kind, modifiers) = style(receiver);
    objects.push(Found {
        name: receiver_name.to_owned(),
        kind,
        modifiers,
    });
    let (kind, modifiers) = style(method);
    objects.push(Found {
        name: method_name.to_owned(),
        kind,
        modifiers,
    });
    objects
}

fn current_method(
    workspace: &Workspace,
    file: &Path,
    receiver_name: &str,
    method_name: &str,
    has_extra: bool,
) -> Vec<Found> {
    if has_extra {
        return Vec::new();
    }
    let receiver = workspace.package_symbols(file).find(|(_, symbol)| {
        symbol.name == receiver_name && symbol.receiver.is_none() && symbol.is_type()
    });
    let Some((_, receiver)) = receiver else {
        return Vec::new();
    };
    let method = workspace.package_symbols(file).find(|(_, symbol)| {
        symbol.name == method_name && symbol.receiver.as_deref() == Some(receiver_name)
    });
    let Some((_, method)) = method else {
        return Vec::new();
    };
    let (kind, modifiers) = style(receiver);
    let mut objects = vec![Found {
        name: receiver_name.to_owned(),
        kind,
        modifiers,
    }];
    let (kind, modifiers) = style(method);
    objects.push(Found {
        name: method_name.to_owned(),
        kind,
        modifiers,
    });
    objects
}
