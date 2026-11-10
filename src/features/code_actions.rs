//! Code actions adapt native diagnostics to quick fixes and organize imports.
use crate::analysis::query::Context;
use crate::analysis::query::diagnostics;
use crate::text::{self, SourceMap};
use crate::workspace;
use serde_json::{Value, json};

const QUICKFIX: &str = "quickfix";
const ORGANIZE_IMPORTS: &str = "source.organizeImports";

pub(super) fn execute(context: &Context<'_>, params: &Value) -> Result<Value, (i32, String)> {
    let only: Vec<&str> = params["context"]["only"]
        .as_array()
        .map(|kinds| kinds.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let source = context.source();
    let map = SourceMap::new(source);
    let requested = requested_range(source, &params["range"]);
    let mut actions = Vec::new();
    if allows(&only, QUICKFIX) {
        quickfixes(context, &map, requested, params, &mut actions);
    }
    if allows(&only, ORGANIZE_IMPORTS) {
        organize_imports(context, &map, &mut actions);
    }
    if actions.is_empty() {
        return Ok(Value::Null);
    }
    Ok(Value::Array(actions))
}

/// LSP kind filters match themselves and every more specific kind.
fn allows(filters: &[&str], kind: &str) -> bool {
    filters.is_empty()
        || filters
            .iter()
            .any(|filter| kind == *filter || kind.starts_with(&format!("{filter}.")))
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
        if diagnostic["source"].as_str() != Some("compiler")
            || diagnostic["code"].as_str() != Some("UnusedVar")
        {
            continue;
        }
        let Some(message) = diagnostic["message"].as_str() else {
            continue;
        };
        let Some(name) = message.strip_prefix("declared and not used: ") else {
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
    let text_document = match context.workspace().document_version(context.file()) {
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
