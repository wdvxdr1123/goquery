//! Protocol items adapt bounded Context call relations.
use crate::analysis::query::Context;
use crate::index::IndexedSymbol as Symbol;
use crate::workspace;
use serde_json::{Value, json};
use std::path::Path;

pub(super) fn item<View>(context: &Context<'_, View>, file: &Path, symbol: &Symbol) -> Value {
    let workspace = context.workspace();
    let uri = workspace::uri(file);
    let filename = file.file_name().unwrap_or_default().to_string_lossy();
    let detail = workspace.import_path(file).map_or_else(
        || filename.to_string(),
        |package| format!("{package} \u{2022} {filename}"),
    );
    let initializer = symbol.is_value();
    let mut range = symbol.selection;
    if initializer {
        for sibling in &workspace.files[file].symbols {
            if sibling.range == symbol.range {
                if (
                    sibling.selection.start.line,
                    sibling.selection.start.character,
                ) < (range.start.line, range.start.character)
                {
                    range.start = sibling.selection.start;
                }
                if (sibling.selection.end.line, sibling.selection.end.character)
                    > (range.end.line, range.end.character)
                {
                    range.end = sibling.selection.end;
                }
            }
        }
    }
    json!({"name":if initializer {"init"} else {&symbol.name},"kind":if initializer {tower_lsp::lsp_types::SymbolKind::VARIABLE} else {tower_lsp::lsp_types::SymbolKind::FUNCTION},"detail":detail,"uri":uri,
            "range":range,"selectionRange":range,
            "data":{"uri":uri,"start":symbol.start,"name":symbol.name}})
}

pub(super) fn execute(
    context: &Context<'_, ()>,
    params: &Value,
    incoming: bool,
) -> Result<Value, (i32, String)> {
    let data = &params["item"]["data"];
    let file = workspace::path(&data["uri"]).ok_or((
        crate::error::INVALID_PARAMS,
        "invalid call hierarchy item".into(),
    ))?;
    let start = data["start"].as_u64().ok_or((
        crate::error::INVALID_PARAMS,
        "missing symbol identity".into(),
    ))? as usize;
    let (file, target) = context.call_target(&file, start, data["name"].as_str())?;
    let groups = if incoming && target.is_value() {
        Vec::new()
    } else if incoming {
        context.incoming_calls(file, target)?
    } else {
        context.outgoing_calls(file, target)?
    };
    let mut groups: Vec<_> = groups
        .into_iter()
        .map(|group| {
            let key = (workspace::uri(group.file), group.symbol.start);
            let node = item(context, group.file, group.symbol);
            let ranges: Vec<_> = group
                .sites
                .into_iter()
                .map(super::query::source_range)
                .collect();
            let value = if incoming {
                json!({"from":node,"fromRanges":ranges})
            } else {
                json!({"to":node,"fromRanges":ranges})
            };
            (key, value)
        })
        .collect();
    groups.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(Value::Array(
        groups.into_iter().map(|(_, value)| value).collect(),
    ))
}
