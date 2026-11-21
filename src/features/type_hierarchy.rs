//! Wire items and validation adapt Context structural/embedding relations.
use crate::analysis::query::{Context, SourceRange};
use crate::index::IndexedSymbol as Symbol;
use crate::workspace;
use serde_json::{Value, json};
use std::path::Path;

pub(super) enum Relation {
    Structural,
    Embedding,
}
pub(super) fn item<View>(context: &mut Context<'_, View>, file: &Path, symbol: &Symbol) -> Value {
    let uri = workspace::uri(file);
    let kind = if context.type_is_interface(file, symbol) {
        tower_lsp::lsp_types::SymbolKind::INTERFACE
    } else {
        tower_lsp::lsp_types::SymbolKind::CLASS
    };
    let mut item = json!({"name":symbol.name,"kind":kind,"detail":context.type_package(file),"uri":uri,"range":symbol.selection,"selectionRange":symbol.selection});
    if let Some(origin) = context.hierarchy_view_origin(file) {
        item["data"] = json!({"viewUri":workspace::uri(origin)});
    }
    item
}

pub(super) fn execute(
    context: &mut Context<'_, ()>,
    params: &Value,
    supers: bool,
    relation: Relation,
) -> Result<Value, (i32, String)> {
    let requested = &params["item"];
    let file = workspace::path(&requested["uri"]).ok_or((
        crate::error::INVALID_PARAMS,
        "invalid type hierarchy URI".into(),
    ))?;
    let selection =
        serde_json::from_value::<tower_lsp::lsp_types::Range>(requested["selectionRange"].clone())
            .ok()
            .filter(|range| {
                serde_json::to_value(range).ok().as_ref() == Some(&requested["selectionRange"])
            })
            .map(|range| {
                SourceRange::coordinates(
                    (range.start.line, range.start.character),
                    (range.end.line, range.end.character),
                )
            });
    let origin = requested["data"]
        .get("viewUri")
        .map(|value| {
            workspace::path(value).ok_or((
                crate::error::INVALID_PARAMS,
                "invalid type hierarchy view".into(),
            ))
        })
        .transpose()?;
    let target = context.type_target(
        &file,
        requested["name"].as_str(),
        selection,
        requested["data"]["start"].as_u64(),
        origin.as_deref(),
    )?;
    let structural = matches!(relation, Relation::Structural);
    let package = if structural {
        context.type_package(target.file)
    } else {
        String::new()
    };
    let results = match relation {
        Relation::Structural => context.type_relations(target.file, target.symbol, supers)?,
        Relation::Embedding => context.embedding_relations(target.file, target.symbol, supers)?,
    };
    let mut encoded = Vec::with_capacity(results.len());
    for target in results {
        let node = item(context, target.file, target.symbol);
        let detail = if structural {
            node["detail"].as_str().unwrap_or_default().to_owned()
        } else {
            String::new()
        };
        let name = if structural {
            target.symbol.name.as_str()
        } else {
            ""
        };
        encoded.push((
            (
                structural && detail != package,
                detail,
                name,
                workspace::uri(target.file),
                target.symbol.start,
            ),
            node,
        ));
    }
    encoded.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(Value::Array(
        encoded.into_iter().map(|(_, item)| item).collect(),
    ))
}
