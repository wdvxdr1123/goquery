//! Query-local SDK builtin declarations shared by hover and signature help.
use super::Context;
use crate::syntax::Symbol;
use std::path::PathBuf;

const MAX_BYTES: usize = 256 * 1024;

pub(super) struct Declaration {
    pub(super) file: PathBuf,
    pub(super) source: String,
    pub(super) symbol: Symbol,
}

impl<View> Context<'_, View> {
    pub(crate) fn builtin_symbol_location(
        &self,
        name: &str,
        types_only: bool,
    ) -> Option<(PathBuf, super::SourceRange)> {
        let file = self.workspace.builtin_file()?;
        let source = self.workspace.source(&file)?;
        let map = crate::text::SourceMap::new(&source);
        let stub = crate::syntax::stub(&source);
        let symbol = stub.symbols.iter().find(|symbol| {
            symbol.name == name && (!types_only || symbol.kind == crate::syntax::SymbolKind::Type)
        })?;
        Some((
            file,
            super::SourceRange::new(&map, symbol.selection.clone()),
        ))
    }
}

pub(super) fn declaration(context: &Context<'_>, name: &str) -> Option<Declaration> {
    let workspace = context.workspace();
    let file = workspace.builtin_file()?;
    let source = workspace
        .source(&file)
        .filter(|source| source.len() <= MAX_BYTES)?;
    let symbol = crate::syntax::stub(&source)
        .symbols
        .into_iter()
        .find(|symbol| symbol.name == name)?;
    Some(Declaration {
        file,
        source,
        symbol,
    })
}

/// gopls presents these SDK documentation type classes using common types.
pub(super) fn replace_placeholder_types(text: &str) -> String {
    text.replace("ComplexType", "complex128")
        .replace("FloatType", "float64")
        .replace("IntegerType", "int")
}
