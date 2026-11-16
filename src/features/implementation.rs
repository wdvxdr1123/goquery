//! Protocol locations adapt Context implementation relations.
use crate::analysis::query::Context;
use crate::analysis::query::implementation::ImplementationRow;
use crate::workspace;
use serde_json::{Value, json};

const MAX_RESULTS: usize = 1024;

pub(super) fn execute(context: &Context<'_>, offset: usize) -> Result<Value, (i32, String)> {
    let Some(rows) = context.implementation_locations(offset)? else {
        return Ok(Value::Null);
    };
    let mut encoded = Vec::with_capacity(rows.len());
    for row in rows {
        match row {
            ImplementationRow::Location(location) => encoded.push(json!({
                "uri": workspace::uri(location.file),
                "range": super::query::source_range(location.range),
            })),
            ImplementationRow::Builtin(name) => {
                let (file, range) = context
                    .builtin_symbol_location(name, true)
                    .ok_or_else(|| (0, format!("missing builtin declaration for {name}")))?;
                encoded.push(json!({
                    "uri": workspace::uri(&file),
                    "range": super::query::source_range(range),
                }));
            }
        }
    }
    encoded.sort_by_key(location_key);
    encoded.dedup();
    encoded.truncate(MAX_RESULTS);
    Ok(Value::Array(encoded))
}

fn location_key(location: &Value) -> (String, u64, u64, u64, u64) {
    let uri = location["uri"].as_str().unwrap_or_default().to_owned();
    let range = &location["range"];
    let number = |value: &Value| value.as_u64().unwrap_or_default();
    (
        uri,
        number(&range["start"]["line"]),
        number(&range["start"]["character"]),
        number(&range["end"]["line"]),
        number(&range["end"]["character"]),
    )
}
