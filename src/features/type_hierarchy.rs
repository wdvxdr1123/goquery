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
pub(super) fn item<View>(context: &Context<'_, View>, file: &Path, symbol: &Symbol) -> Value {
    let uri = workspace::uri(file);
    let kind = if context.type_is_interface(file, symbol) {
        tower_lsp::lsp_types::SymbolKind::INTERFACE
    } else {
        tower_lsp::lsp_types::SymbolKind::CLASS
    };
    json!({"name":symbol.name,"kind":kind,"detail":context.type_package(file),"uri":uri,"range":symbol.selection,"selectionRange":symbol.selection})
}

pub(super) fn execute(
    context: &Context<'_, ()>,
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
    let target = context.type_target(
        &file,
        requested["name"].as_str(),
        selection,
        requested["data"]["start"].as_u64(),
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
    let mut results: Vec<_> = results
        .into_iter()
        .map(|target| {
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
            (
                (
                    structural && detail != package,
                    detail,
                    name,
                    workspace::uri(target.file),
                    target.symbol.start,
                ),
                node,
            )
        })
        .collect();
    results.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(Value::Array(
        results.into_iter().map(|(_, item)| item).collect(),
    ))
}
