//! Prepare-rename and rename adapters: protocol encoding of bounded edit rows.
use crate::analysis::query::Context;
use crate::analysis::query::rename::{RenameEdit, RenameText};
use crate::workspace;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub(super) fn prepare(context: &Context<'_>, offset: usize) -> Result<Value, (i32, String)> {
    let Some(item) = context.prepare_rename(offset)? else {
        return Ok(Value::Null);
    };
    Ok(json!({
        "range": super::query::source_range(item.range),
        "placeholder": item.placeholder,
    }))
}

pub(super) fn execute(
    context: &mut Context<'_>,
    offset: usize,
    params: &Value,
) -> Result<Value, (i32, String)> {
    let new_name = params["newName"]
        .as_str()
        .ok_or((crate::error::INVALID_PARAMS, "expected newName".to_string()))?;
    let edits = context.rename_edits(offset, new_name)?;
    let workspace = context.workspace();
    let mut documents: BTreeMap<PathBuf, Vec<Value>> = BTreeMap::new();
    for edit in edits.edits {
        let encoded = encode_edit(&edit, new_name);
        documents.entry(edit.file).or_default().push(encoded);
    }
    let document_changes: Vec<Value> = documents
        .into_iter()
        .map(|(file, edits)| {
            json!({
                "textDocument": {
                    "uri": workspace::uri(&file),
                    "version": workspace.document_version(&file).unwrap_or(0),
                },
                "edits": edits,
            })
        })
        .collect();
    Ok(json!({ "documentChanges": document_changes }))
}

fn encode_edit(edit: &RenameEdit, new_name: &str) -> Value {
    let text = match &edit.text {
        RenameText::Name => new_name.to_owned(),
        RenameText::ImportPrefix { imported } => {
            if imported == new_name {
                String::new()
            } else {
                format!("{new_name} ")
            }
        }
    };
    json!({
        "range": super::query::source_range(edit.range),
        "newText": text,
    })
}
