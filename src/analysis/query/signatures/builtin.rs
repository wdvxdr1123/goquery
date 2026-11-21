//! Builtin signatures describe SDK type classes.
use super::HelpInformation;
use super::label::signature_label;
use crate::analysis::query::{DocumentView, builtin, documentation};
use crate::syntax::SymbolKind;

pub(super) fn for_name(doc: DocumentView<'_, '_>, name: &str) -> Option<HelpInformation> {
    let declaration = builtin::declaration_in(doc.workspace(), name)?;
    if declaration.symbol.kind != SymbolKind::Function {
        return None;
    }
    let range = declaration.symbol.range;
    let header = declaration.source.get(range.clone())?;
    if header.len() > crate::analysis::budget::MAX_BYTES {
        return None;
    }
    let parsed = crate::syntax::parse_signature(header);
    if !parsed.diagnostics.is_empty() {
        return None;
    }
    let information = signature_label(header, &parsed, name, |text| {
        Some(builtin::replace_placeholder_types(text))
    })?;
    let documentation = documentation::attached(
        &declaration.source,
        documentation::Target {
            start: range.start,
            end: range.end,
            declaration: None,
            trailing: false,
        },
    );
    Some(HelpInformation {
        callable: information,
        documentation,
    })
}
