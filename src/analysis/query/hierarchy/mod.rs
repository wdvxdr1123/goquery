//! Metadata identity and bounded type/embedding relation queries.
mod candidates;
mod embedding;
pub(crate) mod relations;
use super::{Context, SourceRange};
use crate::analysis::scope::Locals;
use crate::analysis::types::TypeForm;
use crate::index::IndexedSymbol as Symbol;
use std::path::Path;

pub(crate) struct SymbolTarget<'a> {
    pub(crate) file: &'a Path,
    pub(crate) symbol: &'a Symbol,
}
impl<'a, View> Context<'a, View> {
    pub(crate) fn type_target(
        &self,
        file: &Path,
        name: Option<&str>,
        selection: Option<SourceRange>,
        start: Option<u64>,
    ) -> Result<SymbolTarget<'a>, (i32, String)> {
        let (file, stub) = self
            .workspace
            .files
            .get_key_value(file)
            .ok_or((-32801, "type hierarchy file is unavailable".into()))?;
        let symbol = stub
            .symbols
            .iter()
            .find(|symbol| {
                let range = symbol.selection;
                symbol.is_type()
                    && name == Some(symbol.name.as_str())
                    && selection
                        == Some(SourceRange::coordinates(
                            (range.start.line, range.start.character),
                            (range.end.line, range.end.character),
                        ))
                    && start.is_none_or(|start| usize::try_from(start).ok() == Some(symbol.start))
            })
            .ok_or((-32801, "type hierarchy item is stale".into()))?;
        Ok(SymbolTarget { file, symbol })
    }
    pub(crate) fn type_is_interface(&self, file: &Path, symbol: &Symbol) -> bool {
        matches!(
            self.workspace
                .type_form(file, &symbol.name, &Locals::empty(), 0),
            Some(TypeForm::Interface)
        )
    }
    pub(crate) fn type_package(&self, file: &Path) -> String {
        let mut path = self.workspace.import_path(file).unwrap_or_default();
        if let Some(stub) = self.workspace.files.get(file)
            && file
                .file_name()
                .is_some_and(|name| name.to_string_lossy().ends_with("_test.go"))
            && stub.package.ends_with("_test")
            && self.workspace.declared_package_name(&path) != Some(stub.package.as_str())
        {
            path.push_str("_test");
        }
        path
    }
}
