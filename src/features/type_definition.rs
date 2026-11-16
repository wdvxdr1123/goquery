//! Protocol locations adapt bounded native declaration targets.
use crate::analysis::query::{Context, TypeDeclaration};
use crate::text::SourceMap;
use crate::workspace;
use serde_json::{Value, json};

pub(super) fn execute(context: &mut Context<'_>, offset: usize) -> Result<Value, (i32, String)> {
    let targets = context.type_declarations(offset)?;
    let mut locations = Vec::new();
    for target in targets {
        let location = match target {
            TypeDeclaration::Named(file, symbol) => {
                Some(json!({"uri":workspace::uri(file),"range":symbol.selection}))
            }
            TypeDeclaration::Local(file, selection) => Some(
                json!({"uri":workspace::uri(file),"range":SourceMap::new(context.source()).range(selection.start,selection.end)}),
            ),
            TypeDeclaration::Builtin(name) => context
                .builtin_symbol_location(name, true)
                .map(|(file, range)| json!({"uri":workspace::uri(&file),"range":super::query::source_range(range)})),
        };
        if let Some(location) = location {
            locations.push(location);
        }
    }
    if locations.is_empty() {
        // gopls serializes an absent location list as null, not an empty array.
        return Ok(Value::Null);
    }
    Ok(Value::Array(locations))
}
