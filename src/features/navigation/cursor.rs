//! Protocol adapters for shared request-local navigation targets.
use crate::analysis::query::{Context, NavigationSymbolTarget as Target};
use crate::text::SourceMap;
use crate::workspace;
use serde_json::{Value, json};

/// Go has no separate declaration semantic: a declaration resolves to the same
/// target as the definition. `textDocument/declaration` is a ccls-compatibility
/// extension (gopls v0.24 leaves it unimplemented), so it shares this adapter
/// instead of growing a second navigation path.
pub(in crate::features) fn execute(
    context: &Context<'_>,
    offset: usize,
    method: &str,
) -> Result<Value, (i32, String)> {
    let file = context.file();
    let source = context.source();
    let navigation = context.navigation_target(offset)?;
    let map = SourceMap::new(source);
    let definition = matches!(
        method,
        "textDocument/definition" | "textDocument/declaration"
    );
    if !navigation.fallback {
        context.trace_candidates(method, 1);
    }
    // Cross-file targets that are not indexed symbols (package doc file).
    if let Some((path, span)) = &navigation.location {
        let item = definition
            .then(|| {
                context
                    .cross_file_range(path, span, &map)
                    .map(|range| json!({"uri": workspace::uri(path), "range": super::super::query::source_range(range)}))
            })
            .flatten();
        return Ok(locations(item));
    }
    // Predeclared identifiers project onto the configured SDK's builtin.go, the
    // same bounded projection type definition uses for builtin types.
    if let Some(name) = &navigation.builtin {
        let item = definition
            .then(|| {
                context
                    .builtin_symbol_location(name, false)
                    .map(|(file, range)| json!({"uri": workspace::uri(&file), "range": super::super::query::source_range(range)}))
            })
            .flatten();
        return Ok(locations(item));
    }
    let result = match navigation.target {
        // gopls answers nil for a cursor that does not resolve to an object; an
        // empty array would be a different wire value.
        Target::Unresolved => Value::Null,
        Target::Member(member) => {
            let item = definition
                .then(|| context.member_selection(member, &map))
                .flatten()
                .map(|range| json!({"uri": workspace::uri(member.file), "range": super::super::query::source_range(range)}));
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
                    context
                        .asm_symbol_location(path, symbol)
                        .map(|(asm_file, range)| {
                            json!({"uri": workspace::uri(&asm_file), "range": super::super::query::source_range(range)})
                        })
                        .or_else(|| {
                            Some(json!({"uri": workspace::uri(path), "range": symbol.selection}))
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
