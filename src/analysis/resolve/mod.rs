//! Package identity and import-qualified lookup over declaration metadata.
mod candidates;
mod names;
mod unqualified;
pub(crate) use candidates::{dependent_candidates, reference_candidates, trace_candidates};
pub(crate) use names::NameIndex;
pub(crate) use unqualified::{
    UnqualifiedLookup, file_symbol_iter, unqualified_lookup, unqualified_symbol,
};

use crate::index::IndexedSymbol as Symbol;
use crate::workspace;
use crate::workspace::Workspace;
use std::ops::Bound;
use std::path::{Path, PathBuf};

pub(crate) fn imported_symbols<'a>(
    workspace: &'a Workspace,
    file: &Path,
    alias: &str,
) -> Vec<(&'a PathBuf, &'a Symbol)> {
    imported_symbol_iter(workspace, file, alias).collect()
}

pub(crate) fn imported_symbol_iter<'a>(
    workspace: &'a Workspace,
    file: &Path,
    alias: &str,
) -> impl Iterator<Item = (&'a PathBuf, &'a Symbol)> + use<'a> {
    imported_symbol_candidates(workspace, file, alias, None)
}

pub(crate) fn imported_symbol_candidates<'a>(
    workspace: &'a Workspace,
    file: &Path,
    alias: &str,
    probe: Option<crate::bloom::Fingerprint>,
) -> impl Iterator<Item = (&'a PathBuf, &'a Symbol)> + use<'a> {
    let file = workspace
        .files
        .get_key_value(file)
        .map_or(Path::new(""), |(file, _)| file.as_path());
    let import = workspace
        .files
        .get(file)
        .and_then(|stub| {
            stub.imports.iter().find(|import| {
                !import.is_dot()
                    && !import.is_blank()
                    && workspace.import_name(import) == Some(alias)
            })
        })
        .map(|import| import.path.as_str());
    import
        .into_iter()
        .flat_map(move |import| symbols_for_import(workspace, file, import, probe))
}

pub(super) fn symbols_for_import<'a>(
    workspace: &'a Workspace,
    file: &Path,
    import: &'a str,
    probe: Option<crate::bloom::Fingerprint>,
) -> impl Iterator<Item = (&'a PathBuf, &'a Symbol)> + use<'a> {
    let caller_detached = workspace.is_detached(file);
    let test_scope = workspace.test_import_scope(file, import);
    let expected_pkg = workspace.declared_package_name(import);
    let mut directories: Vec<_> = workspace
        .modules
        .iter()
        .filter_map(move |(root, module)| {
            let suffix = if module == "std" {
                Some(import)
            } else if import == module {
                Some("")
            } else {
                import
                    .strip_prefix(module.as_str())
                    .and_then(|rest| rest.strip_prefix('/'))
            };
            suffix.map(|suffix| root.join(suffix))
        })
        .collect();
    directories.sort();
    directories.dedup();
    directories.retain(|directory| {
        expected_pkg.is_some()
            && workspace.directory_import_path(directory).as_deref() == Some(import)
    });
    directories
        .into_iter()
        .flat_map(move |directory| {
            let file_depth = directory.components().count() + 1;
            workspace
                .files
                .range::<Path, _>((Bound::Included(directory.as_path()), Bound::Unbounded))
                .take_while(move |(path, _)| path.starts_with(&directory))
                .filter(move |(path, _)| path.components().count() == file_depth)
        })
        .filter(move |(_, stub)| {
            probe.is_none_or(|probe| stub.declarations.may_contain_fingerprint(probe))
        })
        .filter(move |(path, _)| caller_detached || !workspace.is_detached(path))
        .filter(move |(path, _)| {
            workspace::production_file(path)
                || test_scope.is_some_and(|scope| Some(scope) == path.parent())
        })
        .filter(move |(_, stub)| expected_pkg == Some(stub.package.as_str()))
        .flat_map(|(path, stub)| {
            stub.symbols
                .iter()
                .filter(|symbol| symbol.receiver.is_none() && exported(&symbol.name))
                .map(move |symbol| (path, symbol))
        })
}

pub(crate) fn resolve_type_declaration<'a>(
    workspace: &'a Workspace,
    file: &Path,
    name: &str,
) -> Option<(&'a PathBuf, &'a Symbol)> {
    let name = crate::analysis::scope::named_root(name);
    if let Some((alias, member)) = name.split_once('.') {
        let mut matches = imported_symbol_candidates(
            workspace,
            file,
            alias,
            Some(crate::bloom::Fingerprint::new(member.as_bytes())),
        )
        .filter(|(_, symbol)| symbol.is_type() && symbol.name == member);
        let first = matches.next()?;
        let Some(second) = matches.next() else {
            return Some(first);
        };
        let target = workspace.orphan_view_target(file)?;
        let mut filtered = [first, second]
            .into_iter()
            .chain(matches)
            .filter(|(path, _)| workspace.file_matches_target(path, &target));
        let winner = filtered.next()?;
        filtered.next().is_none().then_some(winner)
    } else {
        unqualified_symbol(workspace, file, name).filter(|(_, symbol)| symbol.is_type())
    }
}

pub(crate) fn exported(name: &str) -> bool {
    crate::syntax::is_exported_name(name)
}

pub(crate) fn requalified_type(
    workspace: &Workspace,
    file: &Path,
    origin: &Path,
    annotation: &str,
) -> Option<String> {
    if annotation.len() > crate::analysis::budget::MAX_BYTES {
        return None;
    }
    let origin = workspace.files.get_key_value(origin)?.0;
    let parsed = crate::syntax::parse_type(annotation);
    if !parsed.diagnostics.is_empty() {
        return None;
    }
    let mut substitutions = Vec::new();
    let mut substitution_bytes = 0usize;
    for reference in parsed.references {
        let start = reference
            .qualifier
            .as_ref()
            .map_or(reference.name.start, |qualifier| qualifier.start);
        let name = &annotation[reference.name.clone()];
        let spelling = reference.qualifier.as_ref().map_or_else(
            || name.to_owned(),
            |qualifier| format!("{}.{name}", &annotation[qualifier.clone()]),
        );
        let replacement = if resolve_type_declaration(workspace, origin, &spelling).is_some() {
            // Uninstantiated single reference only (`reference.name` + optional qualifier).
            result_type(workspace, file, origin, &spelling)?
        } else if origin == file
            || (crate::analysis::scope::universe::builtin_type(name)
                && reference.qualifier.is_none())
        {
            spelling
        } else {
            return None;
        };
        substitution_bytes = substitution_bytes.checked_add(replacement.len())?;
        if substitution_bytes > crate::analysis::budget::MAX_BYTES {
            return None;
        }
        substitutions.push((start..reference.name.end, replacement));
    }
    substitutions.sort_unstable_by_key(|(span, _)| span.start);
    let mut output = String::new();
    let mut end = 0;
    for (span, replacement) in substitutions {
        if span.start < end {
            return None;
        }
        if output
            .len()
            .saturating_add(span.start - end)
            .saturating_add(replacement.len())
            > crate::analysis::budget::MAX_BYTES
        {
            return None;
        }
        output.push_str(&annotation[end..span.start]);
        output.push_str(&replacement);
        end = span.end;
    }
    if output.len().saturating_add(annotation.len() - end) > crate::analysis::budget::MAX_BYTES {
        return None;
    }
    output.push_str(&annotation[end..]);
    Some(output)
}

pub(crate) fn result_type(
    workspace: &Workspace,
    file: &Path,
    origin: &Path,
    name: &str,
) -> Option<String> {
    if name != crate::analysis::scope::named_root(name) {
        return requalified_type(workspace, file, origin, name);
    }
    let (path, symbol) = resolve_type_declaration(workspace, origin, name)?;
    let same_package = path.parent() == file.parent()
        && workspace.files.get(path)?.package == workspace.files.get(file)?.package;
    let candidate = if same_package {
        symbol.name.clone()
    } else {
        let import = workspace.import_path(path)?;
        let imported = workspace
            .files
            .get(file)?
            .imports
            .iter()
            .find(|candidate| !candidate.is_blank() && candidate.path == import)?;
        let alias = workspace.import_name(imported)?;
        if imported.is_dot() {
            symbol.name.clone()
        } else {
            format!("{alias}.{}", symbol.name)
        }
    };
    let (resolved_path, resolved) = resolve_type_declaration(workspace, file, &candidate)?;
    (resolved_path == path && resolved.start == symbol.start).then_some(candidate)
}
