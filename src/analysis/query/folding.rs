//! Folding ranges follow gopls' syntax walk over the read-only syntax index.
//!
//! One bounded pass classifies comments, raw strings, bracket groups and
//! block/call/composite/clause nodes; the result holds protocol coordinates
//! only and is released with the request.
use super::{Context, SourceRange};
use crate::syntax::view::{FoldGroupKind, SyntaxIndex};
use crate::text::SourceMap;
use std::ops::Range;

/// Folding kinds gopls reports; syntax-only regions have no kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FoldKind {
    Comment,
    Imports,
}

#[derive(Clone, Copy)]
pub(crate) struct FoldRange {
    pub(crate) range: SourceRange,
    pub(crate) kind: Option<FoldKind>,
}

pub(crate) fn execute(context: &Context<'_>) -> Result<Vec<FoldRange>, (i32, String)> {
    let source = context.source();
    let map = SourceMap::new(source);
    let Some(index) = SyntaxIndex::parse(source) else {
        return Ok(Vec::new());
    };
    let mut spans: Vec<(Range<usize>, Option<FoldKind>)> = Vec::new();
    for raw in index.raw_strings() {
        spans.push((raw.clone(), None));
    }
    for group in index.comments() {
        let start = map.position(group.group.start);
        let end = map.position(group.group.end);
        if start.line == end.line {
            continue;
        }
        let first_start = map.position(group.first.start);
        let first_end = map.position(group.first.end);
        let start_offset = if first_start.line != first_end.line {
            // A multi-line first comment folds from the end of its first line;
            // the scanner's comment text has carriage returns removed.
            let line = first_line_length(source, group.first.start);
            group.first.start + line
        } else {
            group.first.end
        };
        spans.push((start_offset..group.group.end, Some(FoldKind::Comment)));
    }
    for group in index.groups() {
        // Adjacent brackets hold nothing to fold.
        if group.open.end == group.close.start {
            continue;
        }
        let kind = match group.kind {
            FoldGroupKind::Imports => Some(FoldKind::Imports),
            FoldGroupKind::FieldList | FoldGroupKind::Decl => None,
        };
        spans.push((group.open.start + 1..group.close.start, kind));
    }
    for node in index.nodes() {
        let span = node.open + 1..node.close;
        if span.is_empty() {
            continue;
        }
        spans.push((span, None));
    }
    let mut ranges: Vec<FoldRange> = spans
        .into_iter()
        .map(|(span, kind)| FoldRange {
            range: SourceRange::new(&map, span),
            kind,
        })
        .collect();
    // gopls reports one range per walked node, so coincident ranges such as a
    // single-argument CallExpr and its raw-string argument are both retained.
    ranges.sort_by_key(|range| {
        (
            range.range.start.line,
            range.range.start.utf16_column,
            range.range.end.line,
            range.range.end.utf16_column,
        )
    });
    Ok(ranges)
}

/// Byte length of the first line of a comment, excluding stripped carriage
/// returns, matching the scanner's `ast.Comment.Text` first line.
fn first_line_length(source: &str, start: usize) -> usize {
    let bytes = source.as_bytes();
    let mut index = start;
    let mut length = 0;
    while index < bytes.len() && bytes[index] != b'\n' {
        if bytes[index] != b'\r' {
            length += 1;
        }
        index += 1;
    }
    length
}
