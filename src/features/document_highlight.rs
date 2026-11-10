//! Protocol items adapt Context identity highlights.
use crate::analysis::query::Context;
use crate::analysis::query::highlight::HighlightKind;
use serde_json::{Value, json};
use tower_lsp::lsp_types::DocumentHighlightKind as LspHighlightKind;

pub(super) fn execute(context: &Context<'_>, offset: usize) -> Result<Value, (i32, String)> {
    let Some(rows) = context.document_highlights(offset)? else {
        return Ok(Value::Null);
    };
    Ok(Value::Array(
        rows.into_iter()
            .map(|row| {
                json!({
                    "range": super::query::source_range(row.range),
                    "kind": kind(row.kind),
                })
            })
            .collect(),
    ))
}

fn kind(kind: HighlightKind) -> LspHighlightKind {
    match kind {
        HighlightKind::Text => LspHighlightKind::TEXT,
        HighlightKind::Read => LspHighlightKind::READ,
        HighlightKind::Write => LspHighlightKind::WRITE,
    }
}
