//! Request-local lookup for one spelling, built only from declaration metadata.
use crate::analysis::resolve;
use crate::bloom::Fingerprint;
use crate::index::IndexedSymbol as Symbol;
use crate::workspace;
use crate::workspace::Workspace;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

type Target<'a> = (&'a PathBuf, &'a Symbol);
type Package<'a> = (&'a Path, &'a str);

/// Sparse package/name and import-path/name lookups; no source or body is cached.
pub(crate) struct NameIndex<'a> {
    workspace: &'a Workspace,
    name: Box<str>,
    packages: HashMap<Package<'a>, Option<Target<'a>>>,
    imports: HashMap<String, Option<Target<'a>>>,
    test_imports: HashMap<&'a Path, Option<Target<'a>>>,
}

impl<'a> NameIndex<'a> {
    pub(crate) fn new(workspace: &'a Workspace, name: &str) -> Self {
        let mut result = Self {
            workspace,
            name: name.into(),
            packages: HashMap::new(),
            imports: HashMap::new(),
            test_imports: HashMap::new(),
        };
        let fingerprint = Fingerprint::new(name.as_bytes());
        for (path, stub) in &workspace.files {
            if !stub.identifiers.may_contain_fingerprint(fingerprint) {
                continue;
            }
            for symbol in stub
                .symbols
                .iter()
                .filter(|symbol| symbol.receiver.is_none() && symbol.name == name)
            {
                let target = (path, symbol);
                let package = (
                    path.parent().expect("source directory"),
                    stub.package.as_str(),
                );
                result
                    .packages
                    .entry(package)
                    .and_modify(|entry| *entry = None)
                    .or_insert(Some(target));
                if resolve::exported(name)
                    && let Some(import) = workspace.import_path(path)
                    && workspace.importable_package(&import, stub)
                {
                    if workspace::production_file(path) {
                        result
                            .imports
                            .entry(import)
                            .and_modify(|entry| *entry = None)
                            .or_insert(Some(target));
                    } else {
                        result
                            .test_imports
                            .entry(path.parent().expect("source directory"))
                            .and_modify(|entry| *entry = None)
                            .or_insert(Some(target));
                    }
                }
            }
        }
        result
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn unqualified(&self, file: &Path) -> Option<Target<'a>> {
        let stub = self.workspace.files.get(file)?;
        if stub.imports.iter().any(|import| {
            !import.is_dot()
                && !import.is_blank()
                && self.workspace.import_name(import) == Some(self.name.as_ref())
        }) {
            return None;
        }
        let mut selected = match self.packages.get(&(file.parent()?, stub.package.as_str())) {
            Some(None) => return None,
            Some(Some(target)) => Some(*target),
            None => None,
        };
        let mut previous_imports = HashSet::new();
        for import in stub.imports.iter().filter(|import| import.is_dot()) {
            let path = &import.path;
            if !previous_imports.insert(path) {
                continue;
            }
            if let Some(target) = self.imported_path(file, path) {
                if selected.is_some() || target.is_none() {
                    return None;
                }
                selected = target;
            }
        }
        selected
    }

    pub(crate) fn imported(&self, file: &Path, alias: &str) -> Option<Target<'a>> {
        let import = self
            .workspace
            .files
            .get(file)?
            .imports
            .iter()
            .find(|import| {
                !import.is_dot()
                    && !import.is_blank()
                    && self.workspace.import_name(import) == Some(alias)
            })?;
        self.imported_path(file, &import.path).flatten()
    }

    fn imported_path(&self, file: &Path, import: &str) -> Option<Option<Target<'a>>> {
        let production = self.imports.get(import).copied();
        if self.test_imports.is_empty() {
            return production;
        }
        let tests = self
            .workspace
            .test_import_scope(file, import)
            .and_then(|directory| self.test_imports.get(directory).copied());
        match (production, tests) {
            (None, target) | (target, None) => target,
            (Some(_), Some(_)) => Some(None),
        }
    }
}
