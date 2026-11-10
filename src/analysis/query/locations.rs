//! Bounded query results own coordinates, never candidate source/AST buffers.
use crate::text::SourceMap;
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
}
#[derive(PartialEq, Eq)]
pub(crate) struct SourceLocation<'a> {
    pub(crate) file: &'a Path,
    pub(crate) range: SourceRange,
}

pub(super) fn push<'a>(
    locations: &mut Vec<SourceLocation<'a>>,
    file: &'a Path,
    map: &SourceMap<'_>,
    span: Range<usize>,
) -> Result<(), (i32, String)> {
    if locations.len() == super::budget::MAX_REFERENCES {
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
