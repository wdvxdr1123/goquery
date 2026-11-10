//! Document symbols honour the negotiated hierarchical shape and gopls ranges.
use super::query::source_range;
use crate::analysis::query::Context;
use crate::analysis::query::symbols::{DocumentSymbol, DocumentSymbolKind, document_symbols};
use crate::workspace;
use serde_json::{Value, json};
use tower_lsp::lsp_types::SymbolKind as LspSymbolKind;

pub(super) fn execute(context: &Context<'_>, hierarchical: bool) -> Result<Value, (i32, String)> {
    let symbols = document_symbols(context)?;
    if hierarchical {
        return Ok(Value::Array(
            symbols.into_iter().map(encode_hierarchical).collect(),
        ));
    }
    // Legacy clients receive top-level SymbolInformation entries only, exactly
    // like gopls: nested children are dropped instead of being flattened.
    let uri = workspace::uri(context.file());
    Ok(Value::Array(
        symbols
            .into_iter()
            .map(|symbol| {
                json!({
                    "name": symbol.name,
                    "kind": kind(symbol.kind),
                    "location": {"uri": uri, "range": source_range(symbol.range)},
                })
            })
            .collect(),
    ))
}

fn encode_hierarchical(symbol: DocumentSymbol) -> Value {
    let mut value = json!({
        "name": symbol.name,
        "kind": kind(symbol.kind),
        "range": source_range(symbol.range),
        "selectionRange": source_range(symbol.selection),
    });
    if !symbol.detail.is_empty() {
        value["detail"] = symbol.detail.into();
    }
    if !symbol.children.is_empty() {
        value["children"] = Value::Array(
            symbol
                .children
                .into_iter()
                .map(encode_hierarchical)
                .collect(),
        );
    }
    value
}

// Protocol symbol kinds; semantic categories stay independent of wire numbers.
fn kind(kind: DocumentSymbolKind) -> LspSymbolKind {
    match kind {
        DocumentSymbolKind::Function => LspSymbolKind::FUNCTION,
        DocumentSymbolKind::Method => LspSymbolKind::METHOD,
        DocumentSymbolKind::Struct => LspSymbolKind::STRUCT,
        DocumentSymbolKind::Interface => LspSymbolKind::INTERFACE,
        DocumentSymbolKind::Class => LspSymbolKind::CLASS,
        DocumentSymbolKind::Field => LspSymbolKind::FIELD,
        DocumentSymbolKind::Variable => LspSymbolKind::VARIABLE,
        DocumentSymbolKind::Constant => LspSymbolKind::CONSTANT,
    }
}
