use serde_json::Value;
use tower_lsp::lsp_types::{Position, Range};

/// Query-local line starts. Closed-file indexes retain only compact positions.
pub struct SourceMap<'source> {
    source: &'source str,
    lines: Vec<usize>,
}

impl<'source> SourceMap<'source> {
    pub fn new(source: &'source str) -> Self {
        let mut lines = vec![0];
        lines.extend(source.match_indices('\n').map(|(offset, _)| offset + 1));
        Self { source, lines }
    }

    pub fn position(&self, offset: usize) -> Position {
        let line = self.lines.partition_point(|start| *start <= offset) - 1;
        let character = self.source[self.lines[line]..offset].encode_utf16().count();
        Position {
            line: line as u32,
            character: character as u32,
        }
    }

    pub fn range(&self, start: usize, end: usize) -> Range {
        Range {
            start: self.position(start),
            end: self.position(end),
        }
    }

    /// Converts a UTF-16 position using this snapshot's line table.
    pub fn offset(&self, position: Position) -> Option<usize> {
        let start = *self.lines.get(position.line as usize)?;
        utf16_offset(self.source, start, u64::from(position.character))
    }
}

pub fn offset(source: &str, pos: &Value) -> Option<usize> {
    let line = pos["line"].as_u64()? as usize;
    let column = pos["character"].as_u64()?;
    let mut start = 0;
    for _ in 0..line {
        start += source.get(start..)?.find('\n')? + 1;
    }
    utf16_offset(source, start, column)
}

fn utf16_offset(source: &str, start: usize, column: u64) -> Option<usize> {
    let mut utf16 = 0;
    for (byte, ch) in source.get(start..)?.char_indices() {
        if utf16 == column {
            return Some(start + byte);
        }
        if ch == '\n' {
            return None;
        }
        utf16 += ch.len_utf16() as u64;
        if utf16 > column {
            return None;
        }
    }
    (utf16 == column).then_some(source.len())
}
