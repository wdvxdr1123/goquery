//! Protocol locations adapt Context reference searches with bounded allocation.
use crate::analysis::query::{Context, ReferenceTarget, SourceLocation, SourceRange};
use crate::text::SourceMap;
use crate::workspace;
use serde_json::{Value, json};
use std::time::Instant;

pub(super) fn execute(
    context: Context<'_>,
    offset: usize,
    params: &Value,
) -> Result<Value, (i32, String)> {
    let workspace = context.workspace();
    let file = context.file();
    let started = Instant::now();
    let include_declaration = params["context"]["includeDeclaration"].as_bool() == Some(true);
    let Some(index) = context.reference_index(offset) else {
        return Ok(Value::Null);
    };
    let target = context.reference_target(&index, offset, include_declaration)?;
    let source = context.into_source();
    let query = Context::metadata(workspace);
    let (locations, dedup) = match target {
        ReferenceTarget::Unresolved => return Ok(Value::Null),
        ReferenceTarget::Local(spans) => {
            let map = SourceMap::new(&source);
            let rows: Vec<_> = spans
                .into_iter()
                .map(|span| SourceLocation {
                    file,
                    range: SourceRange::new(&map, span),
                })
                .collect();
            if rows.is_empty() {
                // gopls serializes an absent location list as null.
                return Ok(Value::Null);
            }
            return Ok(Value::Array(rows.into_iter().map(location).collect()));
        }
        ReferenceTarget::Global(target_file, symbol) => {
            drop(source);
            (
                query.global_references(
                    file,
                    &index,
                    target_file,
                    symbol,
                    include_declaration,
                    started,
                )?,
                false,
            )
        }
        ReferenceTarget::Member(member) => {
            drop(source);
            (
                query.member_references(member, &index, include_declaration, file)?,
                true,
            )
        }
    };
    let mut locations: Vec<_> = locations.into_iter().map(location).collect();
    locations.sort_by(|a, b| {
        a["uri"]
            .as_str()
            .cmp(&b["uri"].as_str())
            .then_with(|| {
                a["range"]["start"]["line"]
                    .as_u64()
                    .cmp(&b["range"]["start"]["line"].as_u64())
            })
            .then_with(|| {
                a["range"]["start"]["character"]
                    .as_u64()
                    .cmp(&b["range"]["start"]["character"].as_u64())
            })
    });
    if dedup {
        locations.dedup();
    }
    if locations.is_empty() {
        // gopls serializes an absent location list as null.
        return Ok(Value::Null);
    }
    Ok(Value::Array(locations))
}
fn location(location: SourceLocation<'_>) -> Value {
    json!({"uri":workspace::uri(location.file),"range":super::query::source_range(location.range)})
}
