//! Document links for import paths, comment and string URLs.
use crate::analysis::query::Context;
use crate::text::SourceMap;
use serde_json::{Value, json};

pub(super) fn execute(context: &Context<'_>) -> Result<Value, (i32, String)> {
    let links = context.document_links()?;
    if links.is_empty() {
        return Ok(Value::Null);
    }
    let source = context.source();
    let map = SourceMap::new(source);
    Ok(Value::Array(
        links
            .into_iter()
            .map(|link| {
                json!({
                    "range": map.range(link.span.start, link.span.end),
                    "target": link.target,
                })
            })
            .collect(),
    ))
}
