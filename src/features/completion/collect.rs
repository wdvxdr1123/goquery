//! Adapt completion domain results to client edits and protocol items.
use crate::analysis::query::Context;
use crate::features::completion::render::EditContext;
use crate::features::document;
use crate::text::SourceMap;
use serde_json::{Value, json};

pub(in crate::features) fn execute(
    context: &Context<'_>,
    offset: usize,
    config: &super::render::CompletionConfig,
) -> Result<Value, (i32, String)> {
    let source = context.source();
    let mut render_documentation = |text: &str| {
        if config.markdown {
            document::markdown(text, &[])
        } else {
            text.to_owned()
        }
    };
    let completion = context.completion(
        offset,
        config.snippets && config.calls,
        config.documentation.then_some(&mut render_documentation),
    )?;
    let map = SourceMap::new(source);
    let edit = EditContext {
        config,
        source,
        map: &map,
        span: completion.span,
        cursor: offset,
        calls: completion.calls,
    };
    let items: Vec<Value> = completion
        .items
        .into_iter()
        .enumerate()
        .map(|(rank, item)| {
            let mut value = super::render::completion_item(item, &edit);
            value["sortText"] = format!("{rank:05}").into();
            if rank == 0 {
                value["preselect"] = true.into();
            }
            value
        })
        .collect();
    Ok(json!({"isIncomplete": true, "items": items}))
}
