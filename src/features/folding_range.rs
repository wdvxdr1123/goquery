//! Foldable syntactic regions adapted from gopls' folding-range walk.
use crate::analysis::query::Context;
use crate::analysis::query::folding::{self as folding_query, FoldKind};
use serde_json::{Value, json};

pub(super) fn execute(context: &Context<'_>) -> Result<Value, (i32, String)> {
    let ranges = folding_query::execute(context)?;
    Ok(Value::Array(
        ranges
            .into_iter()
            .map(|fold| {
                let protocol = super::query::source_range(fold.range);
                let mut item = json!({
                    "startLine": protocol.start.line,
                    "startCharacter": protocol.start.character,
                    "endLine": protocol.end.line,
                    "endCharacter": protocol.end.character,
                });
                if let Some(kind) = fold.kind {
                    item["kind"] = json!(match kind {
                        FoldKind::Comment => "comment",
                        FoldKind::Imports => "imports",
                    });
                }
                item
            })
            .collect(),
    ))
}
