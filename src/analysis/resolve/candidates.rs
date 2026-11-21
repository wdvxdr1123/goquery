//! Query-local reverse import reachability, without loading candidate sources.
use crate::analysis::engine::resolve;
use crate::bloom::Fingerprint;
use crate::index::{IndexedFile, IndexedSymbol as Symbol};
use crate::workspace::Workspace;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

type Package<'a> = (&'a Path, &'a str);

/// Reverse import reachability is a necessary condition for inheriting an
/// unexported method from another package.
pub(crate) struct DependentPackages<'a>(HashSet<Package<'a>>);

impl DependentPackages<'_> {
    pub(crate) fn contains(&self, file: &Path, stub: &IndexedFile) -> bool {
        file.parent()
            .is_some_and(|directory| self.0.contains(&(directory, stub.package.as_str())))
    }
}

pub(crate) fn trace_candidates(workspace: &Workspace, query: &str, count: usize) {
    if workspace.trace.queries {
        eprintln!("{query}: {count}/{} candidate files", workspace.files.len());
    }
}

pub(super) fn reference_candidates<'a>(
    view: resolve::View<'a>,
    file: &Path,
    symbol: &Symbol,
) -> Vec<(&'a PathBuf, &'a IndexedFile)> {
    let workspace = view.workspace;
    if !view.file_visible(file) {
        return Vec::new();
    }
    let Some((file, stub)) = workspace.files.get_key_value(file) else {
        return Vec::new();
    };
    let root = (
        file.parent().expect("source directory"),
        stub.package.as_str(),
    );
    let exported = resolve::exported(&symbol.name);
    if !exported || symbol.receiver.is_none() {
        let import = exported
            .then(|| workspace.import_path(file))
            .flatten()
            .map(|path| (Fingerprint::new(path.as_bytes()), path));
        return workspace
            .files
            .iter()
            .filter(|(path, stub)| {
                view.file_visible(path)
                    && ((path.parent(), stub.package.as_str()) == (Some(root.0), root.1)
                        || import.as_ref().is_some_and(|(fingerprint, import)| {
                            stub.import_paths.may_contain_fingerprint(*fingerprint)
                                && stub
                                    .imports
                                    .iter()
                                    .any(|candidate| candidate.path == *import)
                        }))
            })
            .collect();
    }

    dependent_candidates(view, file)
}

pub(super) fn dependent_candidates<'a>(
    view: resolve::View<'a>,
    file: &Path,
) -> Vec<(&'a PathBuf, &'a IndexedFile)> {
    if !view.file_visible(file) {
        return Vec::new();
    }
    let packages = dependent_packages(view, file);
    view.workspace
        .files
        .iter()
        .filter(|(path, stub)| {
            view.file_visible(path)
                && packages
                    .as_ref()
                    .is_none_or(|packages| packages.contains(path, stub))
        })
        .collect()
}

pub(super) fn dependent_packages<'a>(
    view: resolve::View<'a>,
    file: &Path,
) -> Option<DependentPackages<'a>> {
    const MAX_PACKAGES: usize = 4096;
    const MAX_IMPORT_EDGES: usize = 32_768;
    let workspace = view.workspace;
    if !view.file_visible(file) {
        return None;
    }
    let (file, stub) = workspace.files.get_key_value(file)?;
    let root = (file.parent()?, stub.package.as_str());
    // A consumer may import a wrapper instead of the method's declaring
    // package. Include reverse import reachability at package granularity:
    // embedding and a use can live in separate files of the same package.
    // This deliberately overapproximates embedding paths; resolution below
    // still proves the identity of every reference. Rebuilding per query
    // incorporates unsaved imports, build selection and module boundaries.
    let mut paths = HashMap::new();
    let mut importers: HashMap<&str, HashSet<Package<'_>>> = HashMap::new();
    let mut edges = 0;
    for (path, stub) in workspace
        .files
        .iter()
        .filter(|(path, _)| view.file_visible(path))
    {
        let package = (
            path.parent().expect("source directory"),
            stub.package.as_str(),
        );
        paths.entry(package).or_insert_with(|| {
            workspace.import_path(path).filter(|import| {
                view.declared_package_name(path, import) == Some(stub.package.as_str())
            })
        });
        if paths.len() > MAX_PACKAGES {
            return None;
        }
        for import in &stub.imports {
            if importers
                .entry(import.path.as_str())
                .or_default()
                .insert(package)
            {
                edges += 1;
                if edges > MAX_IMPORT_EDGES {
                    return None;
                }
            }
        }
    }
    let mut packages = HashSet::from([root]);
    let mut pending = vec![root];
    while let Some(package) = pending.pop() {
        let Some(Some(path)) = paths.get(&package) else {
            continue;
        };
        if let Some(importers) = importers.get(path.as_str()) {
            for &importer in importers {
                if packages.insert(importer) {
                    pending.push(importer);
                }
            }
        }
    }
    Some(DependentPackages(packages))
}
