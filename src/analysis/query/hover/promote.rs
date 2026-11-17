//! Promoted struct-field table for type declaration hover, following gopls'
//! depth-first inventory with shadowing and ambiguity filtering.
use crate::analysis::query::Context;
use crate::index::IndexedSymbol as Symbol;
use std::path::Path;

/// The `// Embedded fields:` fenced block, rendered like gopls' tabwriter.
pub(super) fn promoted_fields(
    context: &Context<'_>,
    origin: &Path,
    symbol: &Symbol,
) -> Option<String> {
    let rows = context.with_sema(|sema| {
        sema.promoted_fields(
            crate::analysis::obj::DeclId {
                file: origin,
                symbol,
            },
            context.file(),
        )
    })?;
    if rows.is_empty() {
        return None;
    }
    let name_width = rows.iter().map(|row| row.name.len()).max()? + 1;
    let type_width = rows.iter().map(|row| row.type_text.len()).max()? + 1;
    let comment_width = rows
        .iter()
        .map(|row| row.path.len() + "// through ".len())
        .max()?
        + 1;
    let mut text = String::from("// Embedded fields:\n");
    for row in rows {
        let comment = format!("// through {}", row.path);
        let mut line = format!(
            "{:name_width$}{:type_width$}{}",
            row.name, row.type_text, comment
        );
        line.push_str(&" ".repeat(comment_width.saturating_sub(comment.len())));
        text.push_str(&line);
        text.push('\n');
    }
    while text.ends_with("\n\n") {
        text.pop();
    }
    text.pop();
    Some(text)
}
