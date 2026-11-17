//! Package identity and import-qualified lookup over declaration metadata.
mod candidates;
mod members;
mod names;
mod unqualified;
pub(crate) use candidates::trace_candidates;
pub(crate) use members::{MemberDescription, MemberId, MemberIds, member_id};
pub(crate) use names::NameProbe;
pub(crate) use unqualified::{UnqualifiedLookup, file_symbol_iter, unqualified_symbol};

use crate::index::IndexedSymbol as Symbol;
use crate::workspace;
use crate::workspace::Workspace;
use std::ops::Bound;
use std::path::{Path, PathBuf};

/// Metadata lookup in the immutable build context of one semantic request.
#[derive(Clone, Copy)]
pub(crate) struct View<'a> {
    pub(super) workspace: &'a Workspace,
    pub(super) build: crate::workspace::BuildView<'a>,
}

impl<'a> View<'a> {
    pub(crate) fn new(workspace: &'a Workspace, build: crate::workspace::BuildView<'a>) -> Self {
        Self { workspace, build }
    }
    pub(crate) fn for_file(workspace: &'a Workspace, file: &Path) -> Self {
        Self::new(workspace, workspace.build_view(Some(file)))
    }
    pub(crate) fn file_visible(self, file: &Path) -> bool {
        self.build.visible(self.workspace, file)
    }
    pub(crate) fn reference_candidates(
        self,
        file: &Path,
        symbol: &Symbol,
    ) -> Vec<(&'a PathBuf, &'a crate::index::IndexedFile)> {
        candidates::reference_candidates(self, file, symbol)
    }
    pub(crate) fn dependent_candidates(
        self,
        file: &Path,
    ) -> Vec<(&'a PathBuf, &'a crate::index::IndexedFile)> {
        candidates::dependent_candidates(self, file)
    }
    pub(crate) fn file_symbol_iter(
        self,
        file: &Path,
    ) -> impl Iterator<Item = (&'a PathBuf, &'a Symbol)> + use<'a> {
        unqualified::file_symbol_iter_in_view(self, file)
    }

    /// Candidate inventories include selected test variants; selector visibility
    /// is applied separately when a name denotes a source-level object.
    pub(crate) fn package_symbols(
        self,
        file: &Path,
    ) -> impl Iterator<Item = (&'a PathBuf, &'a Symbol)> + use<'a> {
        self.workspace
            .package_files_in_view(file, self.build)
            .flat_map(|(path, stub)| stub.symbols.iter().map(move |symbol| (path, symbol)))
    }

    /// Documentation names occupy package scope, independent of file imports.
    pub(crate) fn package_symbol(
        self,
        file: &Path,
        name: &str,
    ) -> Option<(&'a PathBuf, &'a Symbol)> {
        let probe = crate::bloom::Fingerprint::new(name.as_bytes());
        let mut candidates = self
            .workspace
            .package_files_in_view(file, self.build)
            .filter(|(path, _)| {
                self.workspace
                    .selector_file_visible_in_view(file, path, self.build)
            })
            .filter(|(_, stub)| stub.declarations.may_contain_fingerprint(probe))
            .flat_map(|(path, stub)| stub.symbols.iter().map(move |symbol| (path, symbol)))
            .filter(|(_, symbol)| symbol.receiver.is_none() && symbol.name == name);
        let selected = candidates.next()?;
        candidates.next().is_none().then_some(selected)
    }

    pub(crate) fn import_path_symbols(
        self,
        file: &Path,
        import: &'a str,
    ) -> impl Iterator<Item = (&'a PathBuf, &'a Symbol)> + use<'a> {
        symbols_for_import(self.workspace, file, import, None, self.build)
    }
    pub(crate) fn unqualified_lookup(self, file: &Path, name: &str) -> UnqualifiedLookup<'a> {
        unqualified::unqualified_lookup_in_view(self, file, name)
    }
    pub(crate) fn unqualified_symbol(
        self,
        file: &Path,
        name: &str,
    ) -> Option<(&'a PathBuf, &'a Symbol)> {
        match self.unqualified_lookup(file, name) {
            UnqualifiedLookup::Found(file, symbol) => Some((file, symbol)),
            UnqualifiedLookup::Ambiguous | UnqualifiedLookup::NotFound => None,
        }
    }
    pub(crate) fn declared_package_name(self, file: &Path, import: &str) -> Option<&'a str> {
        if !self.build.includes_detached()
            && let Some(name) = self.workspace.declared_package_name(import)
        {
            return Some(name);
        }
        let test_package = self.workspace.files.get(file).and_then(|stub| {
            (!workspace::production_file(file)
                && self.workspace.import_path(file).as_deref() == Some(import))
            .then(|| stub.package.strip_suffix("_test"))
            .flatten()
        });
        let directories = import_directories(self.workspace, import);
        let mut packages = directories
            .iter()
            .flat_map(|directory| {
                self.workspace
                    .files
                    .range::<Path, _>((Bound::Included(directory.as_path()), Bound::Unbounded))
                    .take_while(move |(path, _)| path.starts_with(directory))
                    .filter(move |(path, _)| path.parent() == Some(directory.as_path()))
            })
            .filter(|(path, stub)| {
                self.build.visible(self.workspace, path)
                    && !stub.package.is_empty()
                    && (workspace::production_file(path)
                        || test_package == Some(stub.package.as_str()))
            })
            .map(|(_, stub)| stub.package.as_str());
        packages
            .next()
            .filter(|first| packages.all(|package| package == *first))
    }
    pub(crate) fn import_name<'i>(
        self,
        file: &Path,
        import: &'i crate::syntax::Import,
    ) -> Option<&'i str>
    where
        'a: 'i,
    {
        import
            .explicit_name()
            .or_else(|| self.declared_package_name(file, &import.path))
            .or_else(|| import.path.rsplit('/').next())
    }
    pub(crate) fn imported_name(self, file: &Path, name: &str) -> bool {
        self.workspace.files.get(file).is_some_and(|stub| {
            stub.imports.iter().any(|import| {
                !import.is_dot()
                    && !import.is_blank()
                    && self.import_name(file, import) == Some(name)
            })
        })
    }
    fn test_import_scope(self, file: &Path, import: &str) -> Option<&'a Path> {
        let (file, stub) = self.workspace.files.get_key_value(file)?;
        (!workspace::production_file(file)
            && stub.package.strip_suffix("_test") == self.declared_package_name(file, import)
            && self.workspace.import_path(file).as_deref() == Some(import))
        .then(|| file.parent())
        .flatten()
    }
    pub(crate) fn imported_symbol_iter(
        self,
        file: &Path,
        alias: &str,
    ) -> impl Iterator<Item = (&'a PathBuf, &'a Symbol)> + use<'a> {
        self.imported_symbol_candidates(file, alias, None)
    }
    pub(crate) fn imported_symbol_candidates(
        self,
        file: &Path,
        alias: &str,
        probe: Option<crate::bloom::Fingerprint>,
    ) -> impl Iterator<Item = (&'a PathBuf, &'a Symbol)> + use<'a> {
        imported_symbol_candidates_in_view(self, file, alias, probe)
    }
    pub(crate) fn resolve_type_declaration(
        self,
        file: &Path,
        name: &str,
    ) -> Option<(&'a PathBuf, &'a Symbol)> {
        resolve_type_declaration_in_view(self, file, name)
    }
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
    View::for_file(workspace, file).imported_symbol_candidates(file, alias, probe)
}

fn imported_symbol_candidates_in_view<'a>(
    view: View<'a>,
    file: &Path,
    alias: &str,
    probe: Option<crate::bloom::Fingerprint>,
) -> impl Iterator<Item = (&'a PathBuf, &'a Symbol)> + use<'a> {
    let workspace = view.workspace;
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
                    && view.import_name(file, import) == Some(alias)
            })
        })
        .map(|import| import.path.as_str());
    import
        .into_iter()
        .flat_map(move |import| symbols_for_import(workspace, file, import, probe, view.build))
}

pub(super) fn symbols_for_import<'a>(
    workspace: &'a Workspace,
    file: &Path,
    import: &'a str,
    probe: Option<crate::bloom::Fingerprint>,
    build: crate::workspace::BuildView<'a>,
) -> impl Iterator<Item = (&'a PathBuf, &'a Symbol)> + use<'a> {
    let file = workspace
        .files
        .get_key_value(file)
        .map_or(Path::new(""), |(path, _)| path.as_path());
    let test_scope = View::new(workspace, build).test_import_scope(file, import);
    let directories = import_directories(workspace, import);
    let expected_pkg = View::new(workspace, build).declared_package_name(file, import);
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
        .filter(move |(path, _)| build.visible(workspace, path))
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
    View::for_file(workspace, file).resolve_type_declaration(file, name)
}

fn resolve_type_declaration_in_view<'a>(
    view: View<'a>,
    file: &Path,
    name: &str,
) -> Option<(&'a PathBuf, &'a Symbol)> {
    let name = crate::analysis::scope::named_root(name);
    if let Some((alias, member)) = name.split_once('.') {
        let mut matches = view
            .imported_symbol_candidates(
                file,
                alias,
                Some(crate::bloom::Fingerprint::new(member.as_bytes())),
            )
            .filter(|(_, symbol)| symbol.is_type() && symbol.name == member);
        let first = matches.next()?;
        matches.next().is_none().then_some(first)
    } else {
        view.unqualified_symbol(file, name)
            .filter(|(_, symbol)| symbol.is_type())
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

fn import_directories(workspace: &Workspace, import: &str) -> Vec<PathBuf> {
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
    directories
        .retain(|directory| workspace.directory_import_path(directory).as_deref() == Some(import));

    directories
}
