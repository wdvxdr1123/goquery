//! Protocol encoding for navigation results.
use crate::analysis::query::Context;
use serde_json::Value;

/// Go declarations and definitions use the same target.
pub(in crate::features) fn definition(
    context: &Context<'_>,
    offset: usize,
    method: &str,
) -> Result<Value, (i32, String)> {
    Ok(super::super::query::source_locations(
        context.definition_location(offset, method)?,
    ))
}

pub(in crate::features) fn prepare_hierarchy(
    context: &Context<'_>,
    offset: usize,
    method: &str,
) -> Result<Value, (i32, String)> {
    let Some(target) = context.hierarchy_symbol(offset, method)? else {
        return Ok(Value::Null);
    };
    let (path, symbol) = (target.file, target.symbol);
    let item = match method {
        "textDocument/prepareCallHierarchy" if symbol.is_callable() => {
            super::super::call_hierarchy::item(context, path, symbol)
        }
        "textDocument/prepareTypeHierarchy" if symbol.is_type() => {
            super::super::type_hierarchy::item(context, path, symbol)
        }
        _ => return Ok(Value::Null),
    };
    Ok(Value::Array(vec![item]))
}
