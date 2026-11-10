//! Protocol locations adapt bounded native declaration targets.
use crate::analysis::query::Context;
use crate::analysis::types::TypeDeclaration;
use crate::syntax::SymbolKind;
use crate::text::SourceMap;
use crate::workspace;
use serde_json::{Value, json};

pub(super) fn execute(context: &mut Context<'_>, offset: usize) -> Result<Value, (i32, String)> {
    let workspace = context.workspace();
    let targets = context.type_declarations(offset)?;
    let builtin = targets
        .iter()
        .any(|target| matches!(target, TypeDeclaration::Builtin(_)))
        .then(|| workspace.builtin_file())
        .flatten()
        .and_then(|file| workspace.source(&file).map(|source| (file, source)));
    let builtin_symbols = builtin
        .as_ref()
        .map(|(_, source)| crate::syntax::stub(source));
    let builtin_map = builtin.as_ref().map(|(_, source)| SourceMap::new(source));
    let mut locations = Vec::new();
    for target in targets {
        let location=match target {
            TypeDeclaration::Named(file,symbol)=>Some(json!({"uri":workspace::uri(file),"range":symbol.selection})),
            TypeDeclaration::Local(file,selection)=>Some(json!({"uri":workspace::uri(file),"range":SourceMap::new(context.source()).range(selection.start,selection.end)})),
            TypeDeclaration::Builtin(name)=>builtin.as_ref().and_then(|(file,_source)| {
                let symbol=builtin_symbols.as_ref()?.symbols.iter().find(|symbol|symbol.kind==SymbolKind::Type && symbol.name==name)?;
                Some(json!({"uri":workspace::uri(file),"range":builtin_map.as_ref()?.range(symbol.selection.start,symbol.selection.end)}))
            }),
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
