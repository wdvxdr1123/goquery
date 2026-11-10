use crate::analysis::query::Context;
use crate::analysis::scope::Identity;
use crate::analysis::source::Role;
mod config;
mod encode;
pub(crate) use config::SemanticConfig;
mod comments;
mod format;
mod identity;
mod lexical;
use crate::analysis::check::shapes;
use encode::Encoder;
use serde_json::{Value, json};
use std::collections::HashMap;

/// gopls rejects semantic token requests whose byte span exceeds this size.
const MAX_RANGE_BYTES: usize = 100_000;

pub(super) fn execute(context: &Context<'_>) -> Result<Value, (i32, String)> {
    let mut encoder = Encoder::new(context.source(), &context.workspace().semantic_config);
    encode_tokens(context, &mut encoder)?;
    Ok(json!({"data": encoder.data}))
}

/// Range requests reuse the full tokenizer and keep the tokens intersecting the
/// requested range, exactly as gopls' `tokenVisitor.token` does.
pub(super) fn execute_range(context: &Context<'_>, params: &Value) -> Result<Value, (i32, String)> {
    let source = context.source();
    let lines = line_starts(source);
    let start = position_offset(source, &params["range"]["start"], &lines)?;
    let end = position_offset(source, &params["range"]["end"], &lines)?;
    if start > end {
        return Err((0, format!("start (offset {start}) > end (offset {end})")));
    }
    if end - start > MAX_RANGE_BYTES {
        return Err((
            0,
            format!(
                "semantic tokens: range {} too large ({} > {MAX_RANGE_BYTES})",
                context.file().display(),
                end - start
            ),
        ));
    }
    let mut encoder =
        Encoder::with_window(source, &context.workspace().semantic_config, start..end);
    encode_tokens(context, &mut encoder)?;
    Ok(json!({"data": encoder.data}))
}

/// Byte offsets of line starts, matching gopls' `protocol.Mapper` line table
/// (`[0]` plus the offset after every `\n`).
fn line_starts(source: &str) -> Vec<usize> {
    let mut starts = vec![0];
    starts.extend(source.match_indices('\n').map(|(offset, _)| offset + 1));
    starts
}

/// Converts an LSP UTF-16 position to a byte offset with gopls' validation
/// order and messages. The virtual line at `lines.len()` is the EOF position.
fn position_offset(
    source: &str,
    position: &Value,
    lines: &[usize],
) -> Result<usize, (i32, String)> {
    let line = position["line"].as_u64().ok_or((
        crate::error::INVALID_PARAMS,
        "expected range line".to_string(),
    ))? as usize;
    let character = position["character"].as_u64().ok_or((
        crate::error::INVALID_PARAMS,
        "expected range character".to_string(),
    ))? as usize;
    if line > lines.len() {
        return Err((
            0,
            format!("line number {line} out of range 0-{}", lines.len()),
        ));
    }
    if line == lines.len() {
        if character == 0 {
            return Ok(source.len());
        }
        return Err((0, "column is beyond end of file".to_string()));
    }
    let start = lines[line];
    let mut column = 0;
    let mut offset = start;
    let mut remaining = &source[start..];
    while column < character {
        let Some(ch) = remaining.chars().next() else {
            return Err((0, "column is beyond end of file".to_string()));
        };
        if ch == '\n' {
            return Err((0, "column is beyond end of line".to_string()));
        }
        remaining = &remaining[ch.len_utf8()..];
        let width = ch.len_utf16();
        if width == 2 && column + 1 == character {
            // Requested position falls inside a surrogate pair; gopls returns
            // the rune start without consuming it.
            break;
        }
        column += width;
        offset += ch.len_utf8();
    }
    Ok(offset)
}

fn encode_tokens(context: &Context<'_>, encoder: &mut Encoder<'_>) -> Result<(), (i32, String)> {
    let workspace = context.workspace();
    let source = context.source();
    let file = context.file();
    let trace = workspace.trace.queries.then(std::time::Instant::now);
    let analysis = context.semantic_analysis()?;
    let mut resolver = context.resolver();
    let analysis_time = trace.map(|phase| phase.elapsed());
    let phase = workspace.trace.queries.then(std::time::Instant::now);
    let builtin = workspace.builtin_file();
    let builtin = builtin.as_deref();
    let mut formatting: HashMap<std::ops::Range<usize>, Vec<std::ops::Range<usize>>> =
        HashMap::new();
    for literal in context.format_directives() {
        formatting.insert(literal.literal, literal.directives);
    }
    let mut lexical = lexical::tokens(source)
        .flat_map(|(span, kind)| match kind {
            config::SemanticType::Comment => comments::split(workspace, file, source, &span),
            config::SemanticType::String => match formatting.get(&span) {
                // gopls colors multiline strings as plain string tokens and
                // skips printf splitting for them entirely.
                Some(directives) if !source[span.clone()].contains('\n') => {
                    format::split(span, directives)
                }
                _ => vec![(span, kind, 0)],
            },
            _ => vec![(span, kind, 0)],
        })
        .filter(|(span, _, _)| {
            !matches!(&source[span.clone()], "=" | ":=")
                || !analysis.declaration_operator(span.start)
        })
        .peekable();
    let setup_time = phase.map(|phase| phase.elapsed());
    let mut shapes = context.shapes(&analysis);
    let mut resolve_time = std::time::Duration::ZERO;
    let mut style_time = std::time::Duration::ZERO;
    let mut shape_time = std::time::Duration::ZERO;
    let mut lexical_time = std::time::Duration::ZERO;
    let mut name_count = 0usize;
    let mut slowest: Vec<(std::time::Duration, String)> = Vec::new();
    let mut resolve_by_tag: HashMap<&'static str, (std::time::Duration, usize)> = HashMap::new();
    let mut style_by_tag: HashMap<&'static str, (std::time::Duration, usize)> = HashMap::new();
    let mut resolve_by_outcome: HashMap<
        (&'static str, &'static str),
        (std::time::Duration, usize),
    > = HashMap::new();
    let mut style_by_outcome: HashMap<(&'static str, &'static str), (std::time::Duration, usize)> =
        HashMap::new();
    for name in &analysis.names {
        name_count += 1;
        let phase = workspace.trace.queries.then(std::time::Instant::now);
        while lexical
            .peek()
            .is_some_and(|(span, _, _)| span.start < name.span.start)
        {
            let (span, kind, modifiers) = lexical.next().expect("peeked lexical token");
            encoder.push(span, kind, modifiers);
        }
        if let Some(phase) = phase {
            lexical_time += phase.elapsed();
        }
        let phase = workspace.trace.queries.then(std::time::Instant::now);
        let identity = resolver.resolve(&analysis, source, name);
        if let Some(phase) = phase {
            let elapsed = phase.elapsed();
            resolve_time += elapsed;
            let tag = role_tag(&name.role);
            let entry = resolve_by_tag.entry(tag).or_default();
            entry.0 += elapsed;
            entry.1 += 1;
            if matches!(name.role, Role::Member(Some(_)) | Role::ExpressionMember(_)) {
                let outcome = match &identity {
                    Some(Identity::Local(_)) => "local",
                    Some(Identity::Global(_, symbol)) => global_kind_name(symbol),
                    Some(Identity::Member(_)) => "member",
                    None => "none",
                };
                let entry = resolve_by_outcome.entry((tag, outcome)).or_default();
                entry.0 += elapsed;
                entry.1 += 1;
            }
            if elapsed > std::time::Duration::from_millis(1) {
                let line = source[..name.span.start].matches('\n').count() + 1;
                let role = match &name.role {
                    Role::Name => "Name",
                    Role::Type(_) => "Type",
                    Role::EmbeddedType(_) => "EmbeddedType",
                    Role::Member(_) => "Member",
                    Role::ExpressionMember(_) => "ExpressionMember",
                    Role::Field => "Field",
                    Role::FieldDeclaration(_) => "FieldDeclaration",
                    Role::LiteralKey(_) => "LiteralKey",
                    Role::LiteralElementKey(_) => "LiteralElementKey",
                    Role::BlankDeclaration { .. } => "BlankDeclaration",
                    Role::Parameter => "Parameter",
                    Role::Result => "Result",
                    Role::Namespace => "Namespace",
                    Role::Ignored => "Ignored",
                };
                slowest.push((
                    elapsed,
                    format!(
                        "{}:{line} {} {role} -> {}",
                        &source[name.span.clone()],
                        name.span.start,
                        match &identity {
                            Some(Identity::Local(_)) => "local",
                            Some(Identity::Global(_, symbol)) => global_kind_name(symbol),
                            Some(Identity::Member(_)) => "member",
                            None => "none",
                        }
                    ),
                ));
            }
        }
        if matches!(name.role, Role::Ignored)
            || matches!(name.role, Role::Namespace) && &source[name.span.clone()] == "_"
        {
            continue;
        }
        let phase = workspace.trace.queries.then(std::time::Instant::now);
        let (kind, modifiers) =
            identity::style(&analysis, &resolver, file, source, name, &identity, builtin);
        if let Some(phase) = phase {
            let elapsed = phase.elapsed();
            style_time += elapsed;
            let tag = role_tag(&name.role);
            let entry = style_by_tag.entry(tag).or_default();
            entry.0 += elapsed;
            entry.1 += 1;
            if matches!(name.role, Role::Member(Some(_)) | Role::ExpressionMember(_)) {
                let outcome = match &identity {
                    Some(Identity::Local(_)) => "local",
                    Some(Identity::Global(_, symbol)) => global_kind_name(symbol),
                    Some(Identity::Member(_)) => "member",
                    None => "none",
                };
                let entry = style_by_outcome.entry((tag, outcome)).or_default();
                entry.0 += elapsed;
                entry.1 += 1;
            }
            if elapsed > std::time::Duration::from_millis(5) {
                let line = source[..name.span.start].matches('\n').count() + 1;
                slowest.push((
                    elapsed,
                    format!(
                        "style {}:{line} {} {:?}",
                        &source[name.span.clone()],
                        name.span.start,
                        match &identity {
                            Some(Identity::Local(_)) => "local",
                            Some(Identity::Global(_, symbol)) => global_kind_name(symbol),
                            Some(Identity::Member(_)) => "member",
                            None => "none",
                        }
                    ),
                ));
            }
        }
        let phase = workspace.trace.queries.then(std::time::Instant::now);
        let shape = if workspace.semantic_config.type_shapes() {
            shape_modifiers(shapes.style(name, &identity))
        } else {
            0
        };
        if let Some(phase) = phase {
            shape_time += phase.elapsed();
        }
        encoder.push(name.span.clone(), kind, modifiers | shape);
    }
    let phase = workspace.trace.queries.then(std::time::Instant::now);
    for (span, kind, modifiers) in lexical {
        encoder.push(span, kind, modifiers);
    }
    if let Some(phase) = phase {
        lexical_time += phase.elapsed();
    }
    if let Some(start) = trace {
        slowest.sort_by_key(|entry| std::cmp::Reverse(entry.0));
        slowest.truncate(20);
        eprintln!(
            "semantic tokens: analysis {analysis_time:?}; setup {setup_time:?}; lexical {lexical_time:?}; resolve {resolve_time:?} ({name_count} names); style {style_time:?}; shapes {shape_time:?}; total {:?}",
            start.elapsed()
        );
        for (elapsed, label) in &slowest {
            eprintln!("semantic tokens slow: {elapsed:?} {label}");
        }
        for (tag, (elapsed, count)) in &resolve_by_tag {
            eprintln!("semantic tokens resolve {tag}: {elapsed:?} ({count})");
        }
        for (tag, (elapsed, count)) in &style_by_tag {
            eprintln!("semantic tokens style {tag}: {elapsed:?} ({count})");
        }
        for ((tag, outcome), (elapsed, count)) in &resolve_by_outcome {
            eprintln!("semantic tokens resolve {tag}/{outcome}: {elapsed:?} ({count})");
        }
        for ((tag, outcome), (elapsed, count)) in &style_by_outcome {
            eprintln!("semantic tokens style {tag}/{outcome}: {elapsed:?} ({count})");
        }
    }
    Ok(())
}

fn role_tag(role: &Role) -> &'static str {
    match role {
        Role::Name => "name",
        Role::Type(_) => "type",
        Role::EmbeddedType(_) => "embedded",
        Role::Member(Some(_)) => "member-qualified",
        Role::Member(None) => "member",
        Role::ExpressionMember(_) => "expression-member",
        Role::Field => "field",
        Role::FieldDeclaration(_) => "field-declaration",
        Role::LiteralKey(_) => "literal-key",
        Role::LiteralElementKey(_) => "literal-element-key",
        Role::BlankDeclaration { .. } => "blank",
        Role::Parameter => "parameter",
        Role::Result => "result",
        Role::Namespace => "namespace",
        Role::Ignored => "ignored",
    }
}

/// Trace-only label for a resolved global declaration kind.
fn global_kind_name(symbol: &crate::index::IndexedSymbol) -> &'static str {
    if symbol.is_type() {
        "global-type"
    } else if symbol.is_method() {
        "global-method"
    } else if symbol.is_function() {
        "global-func"
    } else if symbol.is_var() {
        "global-var"
    } else {
        "global-other"
    }
}

fn shape_modifiers(shape: shapes::ValueShape) -> u64 {
    let mut result = 0;
    if shape.forms & shapes::kinds::SIGNATURE != 0 {
        result |= config::SIGNATURE;
    }
    if shape.forms & shapes::kinds::NUMBER != 0 {
        result |= config::NUMBER;
    }
    if shape.forms & shapes::kinds::BOOL != 0 {
        result |= config::BOOL;
    }
    if shape.forms & shapes::kinds::STRING != 0 {
        result |= config::STRING;
    }
    if shape.forms & shapes::kinds::INTERFACE != 0 {
        result |= config::INTERFACE;
    }
    if shape.forms & shapes::kinds::STRUCT != 0 {
        result |= config::STRUCT;
    }
    if shape.forms & shapes::kinds::POINTER != 0 {
        result |= config::POINTER;
    }
    if shape.forms & shapes::kinds::ARRAY != 0 {
        result |= config::ARRAY;
    }
    if shape.forms & shapes::kinds::SLICE != 0 {
        result |= config::SLICE;
    }
    if shape.forms & shapes::kinds::MAP != 0 {
        result |= config::MAP;
    }
    if shape.forms & shapes::kinds::CHAN != 0 {
        result |= config::CHAN;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::{line_starts, position_offset};

    fn position(line: u64, character: u64) -> serde_json::Value {
        serde_json::json!({"line": line, "character": character})
    }

    #[test]
    fn line_starts_match_mapper_table() {
        assert_eq!(line_starts(""), vec![0]);
        assert_eq!(line_starts("a\nb\n"), vec![0, 2, 4]);
    }

    #[test]
    fn position_offset_reports_gopls_errors() {
        let source = "package p\n\nvar x = 1\n";
        let lines = line_starts(source);
        assert_eq!(position_offset(source, &position(0, 0), &lines), Ok(0));
        assert_eq!(
            position_offset(source, &position(4, 0), &lines),
            Ok(source.len())
        );
        assert_eq!(
            position_offset(source, &position(5, 0), &lines),
            Err((0, "line number 5 out of range 0-4".to_string()))
        );
        assert_eq!(
            position_offset(source, &position(0, 100), &lines),
            Err((0, "column is beyond end of line".to_string()))
        );
    }

    #[test]
    fn position_offset_handles_utf16_surrogate_pairs() {
        let source = "var s = \"\u{1f600}\"\n";
        let lines = line_starts(source);
        // The emoji starts at UTF-16 column 9 and occupies two units.
        assert_eq!(position_offset(source, &position(0, 9), &lines), Ok(9));
        assert_eq!(
            position_offset(source, &position(0, 10), &lines),
            Ok(9),
            "a position inside the surrogate pair maps to the rune start"
        );
        assert_eq!(position_offset(source, &position(0, 11), &lines), Ok(13));
    }
}
