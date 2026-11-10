//! Code lenses expose gopls' default generate and cgo commands.
//!
//! gopls enables only the `generate` and `regenerate_cgo` sources by default;
//! test/benchmark lenses stay disabled until a client opts in. The lens set is
//! derived from the same read-only syntax index as folding.
use crate::analysis::query::Context;
use crate::syntax::view::SyntaxIndex;
use crate::text::SourceMap;
use crate::workspace;
use serde_json::{Value, json};
use tower_lsp::lsp_types::Range;

const GENERATE_DIRECTIVE: &str = "//go:generate";
const COMMAND_METADATA: &str = "codelens";

pub(super) fn execute(context: &Context<'_>) -> Result<Value, (i32, String)> {
    let source = context.source();
    let Some(index) = SyntaxIndex::parse(source) else {
        return Ok(Value::Null);
    };
    let map = SourceMap::new(source);
    let file = context.file();
    let mut lenses = Vec::new();
    // `goGenerateCodeLens` reports only the first directive in a file.
    if let Some(comment) = index
        .comment_spans()
        .iter()
        .find(|span| source[(*span).clone()].starts_with(GENERATE_DIRECTIVE))
        && let Some(dir) = file.parent()
    {
        let range = map.range(comment.start, comment.start + GENERATE_DIRECTIVE.len());
        let dir = workspace::uri(dir);
        lenses.push(generate_lens(range, &dir, true));
        lenses.push(generate_lens(range, &dir, false));
    }
    if let Some(import) = index.imports().iter().find(|import| import.path == "C") {
        lenses.push(regenerate_cgo_lens(
            map.range(import.span.start, import.span.end),
            &workspace::uri(file),
        ));
    }
    if lenses.is_empty() {
        return Ok(Value::Null);
    }
    lenses.sort_by_key(|lens| {
        let range = &lens["range"];
        (
            range["start"]["line"].as_u64().unwrap_or_default(),
            range["start"]["character"].as_u64().unwrap_or_default(),
            lens["command"]["title"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
        )
    });
    Ok(Value::Array(lenses))
}

fn generate_lens(range: Range, dir: &str, recursive: bool) -> Value {
    let title = if recursive {
        "run go generate ./..."
    } else {
        "run go generate"
    };
    json!({
        "range": range,
        "command": {
            "title": title,
            "command": "gopls.generate",
            "arguments": [
                {"Dir": dir, "Recursive": recursive},
                {"source": COMMAND_METADATA},
            ],
        },
    })
}

fn regenerate_cgo_lens(range: Range, file: &str) -> Value {
    json!({
        "range": range,
        "command": {
            "title": "regenerate cgo definitions",
            "command": "gopls.regenerate_cgo",
            "arguments": [
                {"URI": file},
                {"source": COMMAND_METADATA},
            ],
        },
    })
}
