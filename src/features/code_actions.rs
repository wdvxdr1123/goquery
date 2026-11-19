//! Code actions adapt native diagnostics to quick fixes and organize imports.
use crate::analysis::query::Context;
use crate::analysis::query::diagnostics;
use crate::text::{self, SourceMap};
use crate::workspace;
use serde_json::{Value, json};

const QUICKFIX: &str = "quickfix";
const ORGANIZE_IMPORTS: &str = "source.organizeImports";

pub(super) fn execute(context: &Context<'_>, params: &Value) -> Result<Value, (i32, String)> {
    let organize = allows(params, ORGANIZE_IMPORTS);
    let quickfix = can_quickfix(params);
    if !organize && !quickfix {
        return Ok(Value::Null);
    }
    let source = context.source();
    let map = SourceMap::new(source);
    let mut actions = Vec::new();
    if quickfix {
        let requested = requested_range(source, &params["range"]);
        quickfixes(context, &map, requested, params, &mut actions);
    }
    if organize {
        organize_imports(context, &map, &mut actions);
    }
    if actions.is_empty() {
        return Ok(Value::Null);
    }
    Ok(Value::Array(actions))
}

pub(super) fn is_noop(params: &Value) -> bool {
    !allows(params, ORGANIZE_IMPORTS) && !can_quickfix(params)
}

fn can_quickfix(params: &Value) -> bool {
    allows(params, QUICKFIX)
        && params["context"]["diagnostics"]
            .as_array()
            .is_some_and(|diagnostics| {
                diagnostics
                    .iter()
                    .any(|item| unused_variable(item).is_some())
            })
}

/// LSP kind filters match themselves and every more specific kind.
fn allows(params: &Value, kind: &str) -> bool {
    let mut filters = params["context"]["only"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .peekable();
    filters.peek().is_none()
        || filters.any(|filter| {
            kind.strip_prefix(filter)
                .is_some_and(|suffix| suffix.is_empty() || suffix.starts_with('.'))
        })
}

fn unused_variable(diagnostic: &Value) -> Option<&str> {
    if diagnostic["source"].as_str() != Some("compiler")
        || diagnostic["code"].as_str() != Some("UnusedVar")
    {
        return None;
    }
    diagnostic["message"]
        .as_str()?
        .strip_prefix("declared and not used: ")
}

/// Requested byte range; an unparseable range disables position filtering.
fn requested_range(source: &str, range: &Value) -> Option<std::ops::Range<usize>> {
    let start = text::offset(source, &range["start"])?;
    let end = text::offset(source, &range["end"])?;
    Some(start..end)
}

fn overlaps(span: &std::ops::Range<usize>, requested: Option<&std::ops::Range<usize>>) -> bool {
    requested.is_none_or(|requested| span.start <= requested.end && requested.start <= span.end)
}

fn quickfixes(
    context: &Context<'_>,
    map: &SourceMap<'_>,
    requested: Option<std::ops::Range<usize>>,
    params: &Value,
    actions: &mut Vec<Value>,
) {
    let Some(diagnostics) = params["context"]["diagnostics"].as_array() else {
        return;
    };
    let source = context.source();
    for diagnostic in diagnostics {
        let Some(name) = unused_variable(diagnostic) else {
            continue;
        };
        let (Some(start), Some(end)) = (
            text::offset(source, &diagnostic["range"]["start"]),
            text::offset(source, &diagnostic["range"]["end"]),
        ) else {
            continue;
        };
        if !overlaps(&(start..end), requested.as_ref()) {
            continue;
        }
        let Some(removal) = diagnostics::variable_removal(context, &(start..end)) else {
            continue;
        };
        let edit = edit(context, &map.range(removal.start, removal.end));
        actions.push(json!({
            "title": format!("Remove variable {name}"),
            "kind": QUICKFIX,
            "diagnostics": [diagnostic],
            "edit": edit,
        }));
    }
}

fn organize_imports(context: &Context<'_>, map: &SourceMap<'_>, actions: &mut Vec<Value>) {
    let imports = diagnostics::unused_imports(context);
    if imports.len() != 1 {
        // gopls emits a minimized multi-edit for several removals; this action
        // covers the single-removal shape only.
        return;
    }
    let Some(removal) = imports[0].removal.as_ref() else {
        return;
    };
    actions.push(json!({
        "title": "Organize Imports",
        "kind": ORGANIZE_IMPORTS,
        "edit": edit(context, &map.range(removal.start, removal.end)),
    }));
}

fn edit(context: &Context<'_>, range: &tower_lsp::lsp_types::Range) -> Value {
    let document = workspace::uri(context.file());
    let text_document = match context.document_version() {
        Some(version) => json!({ "uri": document, "version": version }),
        None => json!({ "uri": document }),
    };
    json!({
        "documentChanges": [{
            "textDocument": text_document,
            "edits": [{ "range": range, "newText": "" }],
        }],
    })
}
