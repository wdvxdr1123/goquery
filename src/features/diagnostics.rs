//! File diagnostics adapt query-local compiler/syntax rows to protocol items.
use crate::analysis::query::Context;
use crate::analysis::query::diagnostics;
use crate::text::SourceMap;
use crate::workspace;
use serde_json::{Value, json};
use tower_lsp::lsp_types::Range;

/// go/types error codes link to the pinned `typesinternal` documentation.
const CODE_LINK: &str = "https://pkg.go.dev/golang.org/x/tools/internal/typesinternal#";
const SEVERITY_ERROR: u8 = 1;
/// `DiagnosticTag.Unnecessary`; both unused-variable and unused-import use it.
const UNNECESSARY: u8 = 1;

pub(super) fn execute(context: &Context<'_>) -> Result<Value, (i32, String)> {
    // Orphaned open buffers (outside the build/discovery scope) are not part of
    // any package; gopls reports no compiler diagnostics for them here.
    if context.workspace().is_detached(context.file()) {
        return Ok(json!([]));
    }
    let rows = diagnostics::diagnostics(context);
    let map = SourceMap::new(context.source());
    let uri = workspace::uri(context.file());
    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        let range: Range = map.range(row.span.start, row.span.end);
        let mut item = json!({ "range": range, "severity": SEVERITY_ERROR });
        match row.kind {
            diagnostics::Kind::Compiler(code) => {
                item["code"] = json!(code.name());
                item["codeDescription"] = json!({ "href": format!("{CODE_LINK}{}", code.name()) });
                item["source"] = json!("compiler");
                item["message"] = json!(row.message);
                if row.unnecessary {
                    item["tags"] = json!([UNNECESSARY]);
                }
                if !row.related.is_empty() {
                    let related: Vec<Value> = row
                        .related
                        .iter()
                        .map(|related| {
                            json!({
                                "location": {
                                    "uri": uri,
                                    "range": map.range(related.span.start, related.span.end),
                                },
                                "message": related.message,
                            })
                        })
                        .collect();
                    item["relatedInformation"] = Value::Array(related);
                }
            }
            diagnostics::Kind::Syntax => {
                item["source"] = json!("syntax");
                item["message"] = json!(row.message);
            }
        }
        items.push(item);
    }
    Ok(Value::Array(items))
}
