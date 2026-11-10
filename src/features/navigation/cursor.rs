//! Protocol adapters for shared request-local navigation targets.
use crate::analysis::check::navigation::Target;
use crate::analysis::query::Context;
use crate::text::SourceMap;
use crate::workspace::{self, Workspace};
use serde_json::{Value, json};
use std::ops::Range;
use std::path::Path;

/// Go has no separate declaration semantic: a declaration resolves to the same
/// target as the definition. `textDocument/declaration` is a ccls-compatibility
/// extension (gopls v0.24 leaves it unimplemented), so it shares this adapter
/// instead of growing a second navigation path.
pub(in crate::features) fn execute(
    context: &Context<'_>,
    offset: usize,
    method: &str,
) -> Result<Value, (i32, String)> {
    let workspace = context.workspace();
    let file = context.file();
    let source = context.source();
    let navigation = context.navigation_target(offset)?;
    let map = SourceMap::new(source);
    let definition = matches!(
        method,
        "textDocument/definition" | "textDocument/declaration"
    );
    if !navigation.fallback {
        workspace.trace_candidates(method, 1);
    }
    // Cross-file targets that are not indexed symbols (package doc file).
    if let Some((path, span)) = &navigation.location {
        let item = definition
            .then(|| cross_file_location(workspace, file, path, span, &map))
            .flatten();
        return Ok(locations(item));
    }
    // Predeclared identifiers project onto the configured SDK's builtin.go, the
    // same bounded projection type definition uses for builtin types.
    if let Some(name) = &navigation.builtin {
        let item = definition
            .then(|| builtin_location(workspace, name))
            .flatten();
        return Ok(locations(item));
    }
    let result = match navigation.target {
        Target::Opaque => unreachable!("Context resolves opaque fallback"),
        // gopls answers nil for a cursor that does not resolve to an object; an
        // empty array would be a different wire value.
        Target::Unresolved => Value::Null,
        Target::Member(member) => {
            let item = definition
                .then(|| {
                    if member.file == file {
                        Workspace::member_data_in_source(member, source, &map)
                    } else {
                        workspace.member_data(member)
                    }
                })
                .flatten()
                .map(|data| json!({"uri":workspace::uri(member.file),"range":data.selection}));
            locations(item)
        }
        Target::Local(selection) if definition => {
            json!([{"uri": workspace::uri(file), "range": map.range(selection.start, selection.end)}])
        }
        Target::Local(_) => Value::Null,
        Target::Global(path, symbol) => {
            let item = match method {
                "textDocument/definition" | "textDocument/declaration" => {
                    // Body-less Go functions may be implemented in assembler;
                    // gopls links the `TEXT` symbol declaration there.
                    asm_location(workspace, path, symbol).or_else(|| {
                        Some(json!({"uri":workspace::uri(path),"range":symbol.selection}))
                    })
                }
                "textDocument/prepareCallHierarchy" if symbol.is_callable() => {
                    Some(super::super::call_hierarchy::item(context, path, symbol))
                }
                "textDocument/prepareTypeHierarchy" if symbol.is_type() => {
                    Some(super::super::type_hierarchy::item(context, path, symbol))
                }
                _ => None,
            };
            locations(item)
        }
    };
    Ok(result)
}

/// gopls serializes absent location lists as `null`, not `[]`.
fn locations(item: Option<Value>) -> Value {
    item.map_or(Value::Null, |item| Value::Array(vec![item]))
}

fn cross_file_location(
    workspace: &Workspace,
    current: &Path,
    path: &Path,
    span: &Range<usize>,
    map: &SourceMap<'_>,
) -> Option<Value> {
    let range = if path == current {
        map.range(span.start, span.end)
    } else {
        let source = workspace.source(path)?;
        SourceMap::new(&source).range(span.start, span.end)
    };
    Some(json!({"uri": workspace::uri(path), "range": range}))
}

fn builtin_location(workspace: &Workspace, name: &str) -> Option<Value> {
    let file = workspace.builtin_file()?;
    let source = workspace.source(&file)?;
    let map = SourceMap::new(&source);
    let stub = crate::syntax::stub(&source);
    let symbol = stub.symbols.iter().find(|symbol| symbol.name == name)?;
    Some(
        json!({"uri":workspace::uri(&file),"range":map.range(symbol.selection.start, symbol.selection.end)}),
    )
}

/// gopls links a body-less Go function to its assembler `TEXT` declaration.
fn asm_location(
    workspace: &Workspace,
    go_file: &Path,
    symbol: &crate::index::IndexedSymbol,
) -> Option<Value> {
    if symbol.body.is_some() || !symbol.is_function() {
        return None;
    }
    let asm_file = go_file.with_extension("s");
    if asm_file == go_file {
        return None;
    }
    let source = workspace.source(&asm_file)?;
    let map = SourceMap::new(&source);
    let mut offset = 0;
    while let Some(found) = source[offset..].find("TEXT") {
        let start = offset + found;
        let after = source[start + 4..].trim_start();
        let token_start = source.len() - after.len();
        let token_end = token_start
            + after
                .find(|character: char| {
                    character.is_whitespace() || character == '(' || character == ','
                })
                .unwrap_or(after.len());
        let token = &source[token_start..token_end];
        if let Some(separator) = token.rfind('·') {
            let name_start = token_start + separator + '·'.len_utf8();
            let name = &source[name_start..token_end];
            if name == symbol.name {
                return Some(json!({
                    "uri": workspace::uri(&asm_file),
                    "range": map.range(name_start, name_start + name.len()),
                }));
            }
        } else if token == symbol.name {
            return Some(json!({
                "uri": workspace::uri(&asm_file),
                "range": map.range(token_start, token_end),
            }));
        }
        offset = start + 4;
    }
    None
}
