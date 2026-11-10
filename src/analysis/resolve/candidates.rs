//! Query-local reverse import reachability, without loading candidate sources.
use crate::analysis::resolve;
use crate::bloom::Fingerprint;
use crate::index::{IndexedFile, IndexedSymbol as Symbol};
use crate::workspace::Workspace;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

type Package<'a> = (&'a Path, &'a str);

impl Workspace {
    pub(crate) fn trace_candidates(&self, query: &str, count: usize) {
        if self.trace.queries {
            eprintln!("{query}: {count}/{} candidate files", self.files.len());
        }
    }

    pub(crate) fn reference_candidates(
        &self,
        file: &Path,
        symbol: &Symbol,
    ) -> Vec<(&PathBuf, &IndexedFile)> {
        let Some((file, stub)) = self.files.get_key_value(file) else {
            return Vec::new();
        };
        let root = (
            file.parent().expect("source directory"),
            stub.package.as_str(),
        );
        // Orphaned open buffers only contribute to / observe their own orphan
        // package (self plus detached siblings); real packages never see them.
        let detached = self.is_detached(file);
        let exported = resolve::exported(&symbol.name);
        if detached {
            return self
                .files
                .iter()
                .filter(|(path, stub)| {
                    self.is_detached(path)
                        && (path.parent(), stub.package.as_str()) == (Some(root.0), root.1)
                })
                .collect();
        }
        if !exported || symbol.receiver.is_none() {
            let import = exported
                .then(|| self.import_path(file))
                .flatten()
                .map(|path| (Fingerprint::new(path.as_bytes()), path));
            return self
                .files
                .iter()
                .filter(|(path, stub)| {
                    !self.is_detached(path)
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

        self.dependent_candidates(file)
    }

    pub(crate) fn dependent_candidates(&self, file: &Path) -> Vec<(&PathBuf, &IndexedFile)> {
        let Some((file, stub)) = self.files.get_key_value(file) else {
            return Vec::new();
        };
        let root = (
            file.parent().expect("source directory"),
            stub.package.as_str(),
        );
        let detached = self.is_detached(file);
        if detached {
            return self
                .files
                .iter()
                .filter(|(path, stub)| {
                    self.is_detached(path)
                        && (path.parent(), stub.package.as_str()) == (Some(root.0), root.1)
                })
                .collect();
        }
        // A consumer may import a wrapper instead of the method's declaring
        // package. Include reverse import reachability at package granularity:
        // embedding and a use can live in separate files of the same package.
        // This deliberately overapproximates embedding paths; resolution below
        // still proves the identity of every reference. Rebuilding per query
        // incorporates unsaved imports, build selection and module boundaries.
        let mut paths = HashMap::new();
        let mut importers: HashMap<&str, HashSet<Package<'_>>> = HashMap::new();
        for (path, stub) in self
            .files
            .iter()
            .filter(|(path, _)| !self.is_detached(path))
        {
            let package = (
                path.parent().expect("source directory"),
                stub.package.as_str(),
            );
            paths.entry(package).or_insert_with(|| {
                self.import_path(path)
                    .filter(|import| self.importable_package(import, stub))
            });
            for import in &stub.imports {
                importers
                    .entry(import.path.as_str())
                    .or_default()
                    .insert(package);
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
        self.files
            .iter()
            .filter(|(path, stub)| {
                !self.is_detached(path)
                    && packages.contains(&(
                        path.parent().expect("source directory"),
                        stub.package.as_str(),
                    ))
            })
            .collect()
    }
}
