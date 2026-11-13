//! Promoted struct-field table for type declaration hover, following gopls'
//! depth-first inventory with shadowing and ambiguity filtering.
use super::render::declaration_source;
use crate::analysis::query::Context;
use crate::index::IndexedSymbol as Symbol;
use crate::syntax::TokenKind;
use crate::workspace::Workspace;
use std::path::{Path, PathBuf};

const MAX_DEPTH: usize = 8;
const MAX_ROWS: usize = 128;

struct Candidate {
    name: String,
    type_text: String,
    path: String,
    depth: usize,
}

struct Frame {
    name: String,
    annotation: String,
    embedded: bool,
}

/// gopls' `// through` suffix for a method promoted from an embedded field.
pub(super) fn promotion_path(
    context: &Context<'_>,
    receiver_file: &Path,
    receiver_text: &str,
    target_file: &Path,
    target: &Symbol,
) -> Option<Vec<String>> {
    let base = receiver_text.trim_start_matches('*');
    let base = base.trim();
    let base = base.split('[').next().unwrap_or(base);
    let workspace = context.workspace();
    let (file, symbol) = workspace.type_declaration(receiver_file, base)?;
    let mut visiting = vec![(file.clone(), symbol.start)];
    path_from(context, file, symbol, target_file, target, 0, &mut visiting)
}

fn path_from(
    context: &Context<'_>,
    file: &Path,
    symbol: &Symbol,
    target_file: &Path,
    target: &Symbol,
    depth: usize,
    visiting: &mut Vec<(PathBuf, usize)>,
) -> Option<Vec<String>> {
    if depth >= MAX_DEPTH || symbol.generic {
        return None;
    }
    let fields = super::size::struct_fields(context, file, symbol)?;
    let workspace = context.workspace();
    for field in fields {
        if !field.embedded {
            continue;
        }
        let Some((embedded_file, embedded)) = embedded_target(workspace, file, &field.annotation)
        else {
            continue;
        };
        if embedded.generic {
            continue;
        }
        let Some(text) = super::render::type_text(context, file, &field.annotation) else {
            continue;
        };
        if embedded_file == target_file && embedded.start == target.start {
            return Some(vec![text]);
        }
        if visiting
            .iter()
            .any(|(visited_file, start)| *visited_file == embedded_file && *start == embedded.start)
        {
            continue;
        }
        visiting.push((embedded_file.clone(), embedded.start));
        if let Some(mut path) = path_from(
            context,
            &embedded_file,
            embedded,
            target_file,
            target,
            depth + 1,
            visiting,
        ) {
            visiting.pop();
            path.insert(0, text);
            return Some(path);
        }
        visiting.pop();
    }
    None
}

/// The `// Embedded fields:` fenced block, rendered like gopls' tabwriter.
pub(super) fn promoted_fields(
    context: &Context<'_>,
    origin: &Path,
    symbol: &Symbol,
) -> Option<String> {
    let mut candidates = Vec::new();
    let mut visiting = vec![(origin.to_path_buf(), symbol.start)];
    collect(
        context,
        origin,
        symbol,
        "",
        0,
        &mut visiting,
        &mut candidates,
    )?;
    if candidates.is_empty() {
        return None;
    }
    // Resolve shadowing: only fields reachable at their minimal depth survive.
    let mut minimum: Vec<(String, usize, usize)> = Vec::new();
    for candidate in &candidates {
        match minimum
            .iter_mut()
            .find(|(name, _, _)| name == &candidate.name)
        {
            Some((_, depth, count)) if candidate.depth == *depth => *count += 1,
            Some((_, depth, count)) if candidate.depth < *depth => {
                *depth = candidate.depth;
                *count = 1;
            }
            Some(_) => {}
            None => minimum.push((candidate.name.clone(), candidate.depth, 1)),
        }
    }
    let mut rows = Vec::new();
    for candidate in candidates {
        if candidate.path.is_empty() {
            continue; // direct fields suppress promoted names but aren't table rows
        }
        let Some((_, depth, count)) = minimum.iter().find(|(name, _, _)| name == &candidate.name)
        else {
            continue;
        };
        if *depth != candidate.depth || *count != 1 {
            continue;
        }
        rows.push(candidate);
    }
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

fn collect(
    context: &Context<'_>,
    file: &Path,
    symbol: &Symbol,
    prefix: &str,
    depth: usize,
    visiting: &mut Vec<(PathBuf, usize)>,
    out: &mut Vec<Candidate>,
) -> Option<()> {
    if depth >= MAX_DEPTH || out.len() > MAX_ROWS {
        return None;
    }
    let frames = struct_frames(context, file, symbol)?;
    for frame in frames {
        let path = if prefix.is_empty() {
            frame.name.clone()
        } else {
            format!("{prefix}.{}", frame.name)
        };
        let candidate_path = prefix.to_owned();
        // An unexported embedding can still promote exported fields. Filter
        // leaf field visibility below rather than rejecting the embedding path.
        if frame.embedded
            && let Some((target_file, target)) =
                embedded_target(context.workspace(), file, &frame.annotation)
            && !target.generic
        {
            if visiting
                .iter()
                .any(|(visited_file, start)| *visited_file == target_file && *start == target.start)
            {
                continue;
            }
            visiting.push((target_file.clone(), target.start));
            collect(
                context,
                &target_file,
                target,
                &path,
                depth + 1,
                visiting,
                out,
            );
            visiting.pop();
        }
        if frame.name == "_" {
            continue;
        }
        if !crate::syntax::is_exported_name(&frame.name) {
            let consumer = context.file();
            let test_variant = consumer
                .file_name()
                .is_some_and(|name| name.to_string_lossy().ends_with("_test.go"));
            if test_variant || !super::same_package(context, consumer, file) {
                continue;
            }
        }
        let type_text = if prefix.is_empty() {
            String::new() // direct shadowing needs identity, not type rendering
        } else {
            let Some(text) = super::render::type_text(context, file, &frame.annotation) else {
                continue;
            };
            text
        };
        out.push(Candidate {
            name: frame.name,
            type_text,
            path: candidate_path,
            depth,
        });
        if out.len() > MAX_ROWS {
            return None;
        }
    }
    Some(())
}

fn embedded_target<'a>(
    workspace: &'a Workspace,
    file: &'a Path,
    annotation: &str,
) -> Option<(PathBuf, &'a Symbol)> {
    let text = annotation.trim().trim_start_matches('*').trim();
    let name = text.split('[').next()?.trim();
    let (path, symbol) = workspace.type_declaration(file, name)?;
    Some((path.clone(), symbol))
}

/// Struct field declarations of a named aggregate type in source order.
fn struct_frames(context: &Context<'_>, file: &Path, symbol: &Symbol) -> Option<Vec<Frame>> {
    let source_text = declaration_source(context, file)?;
    let source = source_text.text();
    let declaration = symbol.declaration()?;
    let rest_base = source.len() - source.get(symbol.end..declaration.end)?.len();
    let fields = super::size::struct_fields(context, file, symbol)?;
    let mut frames = Vec::new();
    let mut search = symbol.end;
    for field in fields {
        // Locate the field's name selection in declaration order.
        let mut start = None;
        for token in crate::syntax::Lexer::new(&source[search..declaration.end]) {
            if token.kind == TokenKind::Ident
                && field.name == source[search + token.span.start..search + token.span.end]
            {
                start = Some(search + token.span.start);
                break;
            }
        }
        let start = start?;
        search = start + field.name.len();
        let _ = rest_base;
        let _ = start;
        frames.push(Frame {
            name: field.name,
            annotation: field.annotation,
            embedded: field.embedded,
        });
    }
    Some(frames)
}
