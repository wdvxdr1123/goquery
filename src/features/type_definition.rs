//! Protocol encoding for type definition locations.
use super::query::source_locations;
use crate::analysis::query::Context;
use serde_json::Value;

pub(super) fn execute(context: &Context<'_>, offset: usize) -> Result<Value, (i32, String)> {
    Ok(source_locations(context.type_definition_locations(offset)?))
}
