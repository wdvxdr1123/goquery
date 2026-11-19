//! Bounded query results own coordinates, never candidate source/AST buffers.
use crate::text::SourceMap;
use std::borrow::Cow;
use std::ops::Range;
use std::path::Path;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct SourcePosition {
    pub(crate) line: u32,
    pub(crate) utf16_column: u32,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct SourceRange {
    pub(crate) start: SourcePosition,
    pub(crate) end: SourcePosition,
}
impl SourceRange {
    pub(crate) fn coordinates(start: (u32, u32), end: (u32, u32)) -> Self {
        Self {
            start: SourcePosition {
                line: start.0,
                utf16_column: start.1,
            },
            end: SourcePosition {
                line: end.0,
                utf16_column: end.1,
            },
        }
    }

    pub(crate) fn new(map: &SourceMap<'_>, span: Range<usize>) -> Self {
        let start = map.position(span.start);
        let end = map.position(span.end);
        Self {
            start: SourcePosition {
                line: start.line,
                utf16_column: start.character,
            },
            end: SourcePosition {
                line: end.line,
                utf16_column: end.character,
            },
        }
    }

    pub(super) fn indexed_selection(range: &tower_lsp::lsp_types::Range) -> Self {
        Self::coordinates(
            (range.start.line, range.start.character),
            (range.end.line, range.end.character),
        )
    }
}
#[derive(PartialEq, Eq)]
pub(crate) struct Location<File> {
    pub(crate) file: File,
    pub(crate) range: SourceRange,
}

pub(crate) type SourceLocation<'a> = Location<&'a Path>;
pub(crate) type NavigationLocation<'a> = Location<Cow<'a, Path>>;

pub(super) fn push<'a>(
    locations: &mut Vec<SourceLocation<'a>>,
    file: &'a Path,
    map: &SourceMap<'_>,
    span: Range<usize>,
) -> Result<(), (i32, String)> {
    if locations.len() == crate::analysis::budget::MAX_REFERENCES {
        return Err((
            crate::error::REQUEST_FAILED,
            "reference result exceeds 4096 locations".into(),
        ));
    }
    locations.push(SourceLocation {
        file,
        range: SourceRange::new(map, span),
    });
    Ok(())
}
