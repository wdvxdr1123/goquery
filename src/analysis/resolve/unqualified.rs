//! File-scoped dot imports participate in unqualified lookup, with ambiguity.
use crate::index::IndexedSymbol as Symbol;
use crate::workspace::Workspace;
use std::path::{Path, PathBuf};

pub(crate) fn file_symbol_iter<'a>(
    workspace: &'a Workspace,
    file: &Path,
) -> impl Iterator<Item = (&'a PathBuf, &'a Symbol)> + use<'a> {
    let file = workspace
        .files
        .get_key_value(file)
        .map_or(Path::new(""), |(file, _)| file.as_path());
    let imports = workspace
        .files
        .get(file)
        .map_or(&[][..], |stub| stub.imports.as_slice());
    let aliases: Vec<_> = imports
        .iter()
        .filter(|import| !import.is_dot() && !import.is_blank())
        .filter_map(|import| workspace.import_name(import))
        .collect();
    let dotted = imports
        .iter()
        .enumerate()
        .filter(|(index, import)| {
            import.is_dot()
                && !imports[..*index]
                    .iter()
                    .any(|other| other.is_dot() && other.path == import.path)
        })
        .flat_map(move |(_, import)| {
            super::symbols_for_import(workspace, file, &import.path, None)
        });
    workspace
        .package_symbols(file)
        .filter(|(_, symbol)| symbol.receiver.is_none())
        .chain(dotted)
        // Named imports also occupy the file block. Invalid collisions must
        // not invent a global target while an editor buffer is incomplete.
        .filter(move |(_, symbol)| !aliases.contains(&symbol.name.as_str()))
}

#[derive(Clone, Copy)]
pub(crate) enum UnqualifiedLookup<'a> {
    Found(&'a PathBuf, &'a Symbol),
    Ambiguous,
    NotFound,
}

pub(crate) fn unqualified_lookup<'a>(
    workspace: &'a Workspace,
    file: &Path,
    name: &str,
) -> UnqualifiedLookup<'a> {
    let imports = workspace
        .files
        .get(file)
        .map_or(&[][..], |stub| stub.imports.as_slice());
    if imports.iter().any(|import| {
        !import.is_dot() && !import.is_blank() && workspace.import_name(import) == Some(name)
    }) {
        return UnqualifiedLookup::NotFound;
    }
    let probe = crate::bloom::Fingerprint::new(name.as_bytes());
    let package = workspace
        .package_symbol_candidates(file, probe)
        .filter(|(_, symbol)| symbol.receiver.is_none());
    let dotted = imports
        .iter()
        .enumerate()
        .filter(|(index, import)| {
            import.is_dot()
                && !imports[..*index]
                    .iter()
                    .any(|other| other.is_dot() && other.path == import.path)
        })
        .flat_map(|(_, import)| {
            super::symbols_for_import(workspace, file, &import.path, Some(probe))
        });
    let mut matches = package
        .chain(dotted)
        .filter(|(_, symbol)| symbol.name == name);
    let Some((path, symbol)) = matches.next() else {
        return UnqualifiedLookup::NotFound;
    };
    if matches.next().is_some() {
        UnqualifiedLookup::Ambiguous
    } else {
        UnqualifiedLookup::Found(path, symbol)
    }
}

pub(crate) fn unqualified_symbol<'a>(
    workspace: &'a Workspace,
    file: &Path,
    name: &str,
) -> Option<(&'a PathBuf, &'a Symbol)> {
    match unqualified_lookup(workspace, file, name) {
        UnqualifiedLookup::Found(path, symbol) => Some((path, symbol)),
        UnqualifiedLookup::Ambiguous | UnqualifiedLookup::NotFound => None,
    }
}
