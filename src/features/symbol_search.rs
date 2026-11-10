//! Protocol symbols adapt bounded Context ranking and member locations.
use crate::analysis::query::Context;
use crate::analysis::query::symbols::SearchKind;
use crate::workspace;
use serde_json::{Value, json};
use tower_lsp::lsp_types::SymbolKind as LspSymbolKind;

pub(super) fn execute(context: &Context<'_, ()>, params: &Value) -> Result<Value, (i32, String)> {
    let text = params["query"].as_str().ok_or((
        crate::error::INVALID_PARAMS,
        "query must be a string".into(),
    ))?;
    let Some(symbols) = context.workspace_symbols(text)? else {
        return Ok(Value::Null);
    };
    Ok(Value::Array(symbols.into_iter().map(|row| {
        let kind = match row.kind {
            SearchKind::Declaration => lsp_symbol_kind(row.symbol.kind),
            SearchKind::Method => LspSymbolKind::METHOD,
            SearchKind::Interface => LspSymbolKind::INTERFACE,
            SearchKind::Field => LspSymbolKind::FIELD,
            SearchKind::Struct => LspSymbolKind::STRUCT,
        };
        json!({"name":row.name,"kind":kind,"containerName":row.container,
            "location":{"uri":workspace::uri(row.location.file),"range":super::query::source_range(row.location.range)}})
    }).collect()))
}

/// LSP `SymbolKind` for an index declaration kind.
fn lsp_symbol_kind(kind: crate::index::SymbolKind) -> LspSymbolKind {
    match kind {
        crate::index::SymbolKind::Type => LspSymbolKind::CLASS,
        crate::index::SymbolKind::Method => LspSymbolKind::METHOD,
        crate::index::SymbolKind::Function => LspSymbolKind::FUNCTION,
        crate::index::SymbolKind::Variable => LspSymbolKind::VARIABLE,
        crate::index::SymbolKind::Constant => LspSymbolKind::CONSTANT,
    }
}
