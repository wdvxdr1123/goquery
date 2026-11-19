//! Protocol locations adapt Context reference searches with bounded allocation.
use super::query::source_location;
use crate::analysis::query::{Context, ReferenceTarget, SourceLocation, SourceRange};
use crate::text::SourceMap;
use serde_json::Value;
use std::time::Instant;

pub(super) fn execute(
    context: Context<'_>,
    offset: usize,
    params: &Value,
) -> Result<Value, (i32, String)> {
    let file = context.file();
    let started = Instant::now();
    let include_declaration = params["context"]["includeDeclaration"].as_bool() == Some(true);
    let Some(index) = context.reference_probe(offset) else {
        return Ok(Value::Null);
    };
    let target = context.reference_target(&index, offset, include_declaration)?;
    let (source, query) = context.into_source_and_metadata();
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
            return Ok(Value::Array(
                rows.into_iter().map(source_location).collect(),
            ));
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
        ReferenceTarget::AnonymousField(field) => {
            drop(source);
            (
                query.inline_field_references(
                    &field.file,
                    &field.selection,
                    &field.name,
                    file,
                    include_declaration,
                )?,
                true,
            )
        }
    };
    let mut locations: Vec<_> = locations.into_iter().map(source_location).collect();
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
    // A resolved query returns an allocated list, including [] for no uses.
    // Unavailable targets return null before this path.
    Ok(Value::Array(locations))
}
