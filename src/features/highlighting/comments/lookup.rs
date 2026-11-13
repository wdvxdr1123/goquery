//! Lookup of doc-link names in package scope and imports.
//!
//! Mirrors `tokenVisitor.comment`'s `lookupObjects`: an optional import
//! qualifier, then either a package-scope name or `Type.Member`. Unresolved or
//! unexported names produce no tokens, leaving the comment untouched.
use super::{exported, style};
use crate::analysis::method_set::ReceiverMember;
use crate::index::IndexedSymbol;
use crate::syntax::MemberKind;
use crate::workspace::Workspace;
use std::path::{Path, PathBuf};

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
        return current_member(workspace, file, parts[0], parts[1], parts.len() == 3);
    }
    let Some((_, symbol)) = package_object(workspace, file, parts[0]) else {
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

/// Doc links use package scope, not names brought into a file by dot imports.
fn package_object<'a>(
    workspace: &'a Workspace,
    file: &Path,
    name: &str,
) -> Option<(&'a PathBuf, &'a IndexedSymbol)> {
    let mut candidates = workspace
        .package_symbol_candidates(file, crate::bloom::Fingerprint::new(name.as_bytes()))
        .filter(|(origin, symbol)| {
            symbol.receiver.is_none()
                && symbol.name == name
                && workspace.selector_file_visible(file, origin)
        });
    let selected = candidates.next()?;
    candidates.next().is_none().then_some(selected)
}

fn imported(workspace: &Workspace, file: &Path, alias: &str, rest: &[&str]) -> Vec<Found> {
    let mut objects = vec![Found {
        name: alias.to_owned(),
        kind: super::super::config::SemanticType::Namespace,
        modifiers: 0,
    }];
    if rest.len() == 1 {
        let name = rest[0];
        let Some((_, symbol)) = workspace
            .imported_symbol_iter(file, alias)
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
    let (receiver_name, member_name) = (rest[0], rest[1]);
    if !exported(receiver_name) || !exported(member_name) {
        return Vec::new();
    }
    let Some((origin, receiver)) = workspace
        .type_declaration(file, &format!("{alias}.{receiver_name}"))
        .filter(|(origin, _)| workspace.selector_file_visible(file, origin))
    else {
        return Vec::new();
    };
    let Some(member) = workspace.scoped_receiver_member(file, origin, &receiver.name, member_name)
    else {
        return Vec::new();
    };
    let (kind, modifiers) = style(receiver);
    objects.push(Found {
        name: receiver_name.to_owned(),
        kind,
        modifiers,
    });
    let Some((kind, modifiers)) = member_style(member) else {
        return Vec::new();
    };
    objects.push(Found {
        name: member_name.to_owned(),
        kind,
        modifiers,
    });
    objects
}

fn current_member(
    workspace: &Workspace,
    file: &Path,
    receiver_name: &str,
    member_name: &str,
    has_extra: bool,
) -> Vec<Found> {
    if has_extra || !exported(receiver_name) || !exported(member_name) {
        return Vec::new();
    }
    let Some((origin, receiver)) =
        package_object(workspace, file, receiver_name).filter(|(_, symbol)| symbol.is_type())
    else {
        return Vec::new();
    };
    let Some(member) = workspace.scoped_receiver_member(file, origin, &receiver.name, member_name)
    else {
        return Vec::new();
    };
    let (kind, modifiers) = style(receiver);
    let mut objects = vec![Found {
        name: receiver_name.to_owned(),
        kind,
        modifiers,
    }];
    let Some((kind, modifiers)) = member_style(member) else {
        return Vec::new();
    };
    objects.push(Found {
        name: member_name.to_owned(),
        kind,
        modifiers,
    });
    objects
}

fn member_style(member: ReceiverMember<'_>) -> Option<(super::super::config::SemanticType, u64)> {
    use super::super::config::{SIGNATURE, SemanticType};
    match member {
        ReceiverMember::Method(_, method) => Some(style(method)),
        ReceiverMember::Declaration(member) => match member.kind {
            MemberKind::Method => Some((SemanticType::Method, SIGNATURE)),
            MemberKind::Field => Some((SemanticType::Property, 0)),
            MemberKind::Interface => None,
        },
    }
}
