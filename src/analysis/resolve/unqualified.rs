//! File-scoped dot imports participate in unqualified lookup, with ambiguity.
use crate::index::IndexedSymbol as Symbol;
use crate::workspace::Workspace;
use std::path::{Path, PathBuf};

impl Workspace {
    pub(crate) fn file_symbol_iter<'a>(
        &'a self,
        file: &Path,
    ) -> impl Iterator<Item = (&'a PathBuf, &'a Symbol)> + use<'a> {
        let file = self
            .files
            .get_key_value(file)
            .map_or(Path::new(""), |(file, _)| file.as_path());
        let imports = self
            .files
            .get(file)
            .map_or(&[][..], |stub| stub.imports.as_slice());
        let aliases: Vec<_> = imports
            .iter()
            .filter(|import| !import.is_dot() && !import.is_blank())
            .filter_map(|import| self.import_name(import))
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
            .flat_map(|(_, import)| self.symbols_for_import(file, &import.path, None));
        self.package_symbols(file)
            .filter(|(_, symbol)| symbol.receiver.is_none())
            .chain(dotted)
            // Named imports also occupy the file block. Invalid collisions must
            // not invent a global target while an editor buffer is incomplete.
            .filter(move |(_, symbol)| !aliases.contains(&symbol.name.as_str()))
    }

    pub(crate) fn unqualified_symbol(
        &self,
        file: &Path,
        name: &str,
    ) -> Option<(&PathBuf, &Symbol)> {
        let imports = self
            .files
            .get(file)
            .map_or(&[][..], |stub| stub.imports.as_slice());
        if imports.iter().any(|import| {
            !import.is_dot() && !import.is_blank() && self.import_name(import) == Some(name)
        }) {
            return None;
        }
        let probe = crate::bloom::Fingerprint::new(name.as_bytes());
        let package = self
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
            .flat_map(|(_, import)| self.symbols_for_import(file, &import.path, Some(probe)));
        let mut matches = package
            .chain(dotted)
            .filter(|(_, symbol)| symbol.name == name);
        let symbol = matches.next()?;
        matches.next().is_none().then_some(symbol)
    }
}
