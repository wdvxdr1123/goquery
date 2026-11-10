//! Relative UTF-16 encoding emits single-line, nonempty semantic tokens.
//!
//! A range request installs a byte window; a token line segment is emitted only
//! when its half-open extent intersects the window, mirroring gopls'
//! `tokenVisitor.token` check (`start < end && end > start`). A zero-width
//! window therefore keeps exactly the tokens that strictly contain it.
use super::SemanticConfig;
use crate::text::SourceMap;
use std::ops::Range;

pub(super) struct Encoder<'a> {
    source: &'a str,
    map: SourceMap<'a>,
    previous: (u64, u64),
    config: &'a SemanticConfig,
    window: Option<Range<usize>>,
    pub data: Vec<u64>,
}

impl<'a> Encoder<'a> {
    pub fn new(source: &'a str, config: &'a SemanticConfig) -> Self {
        Self {
            source,
            map: SourceMap::new(source),
            previous: (0, 0),
            data: Vec::new(),
            config,
            window: None,
        }
    }

    /// Restricts encoding to tokens intersecting `window` (byte offsets).
    pub fn with_window(source: &'a str, config: &'a SemanticConfig, window: Range<usize>) -> Self {
        Self {
            window: Some(window),
            ..Self::new(source, config)
        }
    }

    pub fn push(&mut self, span: Range<usize>, kind: super::config::SemanticType, modifiers: u64) {
        let Some(kind) = self.config.type_index(kind) else {
            return;
        };
        let modifiers = self.config.modifier_bits(modifiers);
        let mut offset = span.start;
        for part in self.source[span].split_inclusive('\n') {
            let text = part.trim_end_matches(['\r', '\n']);
            let end = offset + text.len();
            if !text.is_empty()
                && self
                    .window
                    .as_ref()
                    .is_none_or(|window| offset < window.end && end > window.start)
            {
                let position = self.map.position(offset);
                let line = u64::from(position.line);
                let column = u64::from(position.character);
                self.data.extend([
                    line - self.previous.0,
                    if line == self.previous.0 {
                        column - self.previous.1
                    } else {
                        column
                    },
                    text.encode_utf16().count() as u64,
                    u64::from(kind),
                    modifiers,
                ]);
                self.previous = (line, column);
            }
            offset += part.len();
        }
    }
}
