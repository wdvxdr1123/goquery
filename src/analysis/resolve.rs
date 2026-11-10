//! Package identity and import-qualified lookup over declaration metadata.
mod calls;
mod candidates;
mod names;
pub(crate) use names::NameIndex;
mod unqualified;

use crate::index::IndexedSymbol as Symbol;
use crate::workspace;
use crate::workspace::Workspace;
use std::ops::Bound;
use std::path::{Path, PathBuf};

impl Workspace {
    pub(crate) fn import_path(&self, file: &Path) -> Option<String> {
        let directory = file.parent()?;
        let (root, module) = self
            .modules
            .iter()
            .filter(|(root, _)| directory.starts_with(root))
            .max_by_key(|(root, _)| root.components().count())?;
        let suffix = directory
            .strip_prefix(root)
            .ok()?
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        Some(if module == "std" {
            suffix
        } else if suffix.is_empty() {
            module.clone()
        } else {
            format!("{module}/{suffix}")
        })
    }

    pub(crate) fn imported_symbols(&self, file: &Path, alias: &str) -> Vec<(&PathBuf, &Symbol)> {
        self.imported_symbol_iter(file, alias).collect()
    }

    pub(crate) fn imported_symbol_iter<'a>(
        &'a self,
        file: &Path,
        alias: &str,
    ) -> impl Iterator<Item = (&'a PathBuf, &'a Symbol)> + use<'a> {
        self.imported_symbol_candidates(file, alias, None)
    }

    pub(crate) fn imported_symbol_candidates<'a>(
        &'a self,
        file: &Path,
        alias: &str,
        probe: Option<crate::bloom::Fingerprint>,
    ) -> impl Iterator<Item = (&'a PathBuf, &'a Symbol)> + use<'a> {
        let file = self
            .files
            .get_key_value(file)
            .map_or(Path::new(""), |(file, _)| file.as_path());
        let import = self
            .files
            .get(file)
            .and_then(|stub| {
                stub.imports.iter().find(|import| {
                    !import.is_dot()
                        && !import.is_blank()
                        && self.import_name(import) == Some(alias)
                })
            })
            .map(|import| import.path.as_str());
        import
            .into_iter()
            .flat_map(move |import| self.symbols_for_import(file, import, probe))
    }

    fn symbols_for_import<'a>(
        &'a self,
        file: &Path,
        import: &'a str,
        probe: Option<crate::bloom::Fingerprint>,
    ) -> impl Iterator<Item = (&'a PathBuf, &'a Symbol)> + use<'a> {
        let test_scope = self.test_import_scope(file, import);
        let mut directories: Vec<_> = self
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
            .into_iter()
            .flat_map(move |directory| {
                let file_depth = directory.components().count() + 1;
                self.files
                    .range::<Path, _>((Bound::Included(directory.as_path()), Bound::Unbounded))
                    .take_while(move |(path, _)| path.starts_with(&directory))
                    .filter(move |(path, _)| path.components().count() == file_depth)
            })
            .filter(move |(_, stub)| {
                probe.is_none_or(|probe| stub.declarations.may_contain_fingerprint(probe))
            })
            .filter(move |(path, _)| {
                workspace::production_file(path)
                    || test_scope.is_some_and(|scope| Some(scope) == path.parent())
            })
            .filter(move |(path, stub)| {
                self.import_path(path)
                    .is_some_and(|path| path == import && self.importable_package(&path, stub))
            })
            .flat_map(|(path, stub)| {
                stub.symbols
                    .iter()
                    .filter(|symbol| symbol.receiver.is_none() && exported(&symbol.name))
                    .map(move |symbol| (path, symbol))
            })
    }
}

pub(crate) fn exported(name: &str) -> bool {
    crate::syntax::is_exported_name(name)
}
