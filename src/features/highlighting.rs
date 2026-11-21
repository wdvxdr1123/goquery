use crate::analysis::query::Context;
use serde_json::{Value, json};

/// gopls rejects semantic token requests whose byte span exceeds this size.
const MAX_RANGE_BYTES: usize = 100_000;

pub(super) fn execute(
    context: &mut Context<'_>,
    config: &SemanticConfig,
) -> Result<Value, (i32, String)> {
    let tokens = context.semantic_tokens(None, config.type_shapes())?;
    let mut encoder = Encoder::new(context.source(), config);
    for token in tokens {
        encoder.push(token.span, token.kind, token.modifiers);
    }
    Ok(json!({"data": encoder.data}))
}

/// Range requests reuse the full tokenizer and keep the tokens intersecting the
/// requested range, exactly as gopls' `tokenVisitor.token` does.
pub(super) fn execute_range(
    context: &mut Context<'_>,
    params: &Value,
    config: &SemanticConfig,
) -> Result<Value, (i32, String)> {
    let (start, end) = {
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
        (start, end)
    };
    let tokens = context.semantic_tokens(Some(start..end), config.type_shapes())?;
    let mut encoder = Encoder::with_window(context.source(), config, start..end);
    for token in tokens {
        encoder.push(token.span, token.kind, token.modifiers);
    }
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

// Maps supported semantic categories to the client's negotiated legend.
use crate::analysis::query::semantic_tokens::{SemanticTokenKind, modifiers::DEFINITION};
use tower_lsp::lsp_types::{SemanticTokenModifier, SemanticTokenType, SemanticTokensLegend};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SemanticModifier {
    Declaration,
    Definition,
    Readonly,
    DefaultLibrary,
    Static,
    Signature,
    Number,
    Bool,
    String,
    Interface,
    Struct,
    Pointer,
    Array,
    Slice,
    Map,
    Chan,
    Shadowing,
    Format,
}

impl SemanticModifier {
    const ALL: [Self; 18] = [
        Self::Declaration,
        Self::Definition,
        Self::Readonly,
        Self::DefaultLibrary,
        Self::Static,
        Self::Signature,
        Self::Number,
        Self::Bool,
        Self::String,
        Self::Interface,
        Self::Struct,
        Self::Pointer,
        Self::Array,
        Self::Slice,
        Self::Map,
        Self::Chan,
        Self::Shadowing,
        Self::Format,
    ];
    const SHAPES: [Self; 11] = [
        Self::Signature,
        Self::Number,
        Self::Bool,
        Self::String,
        Self::Interface,
        Self::Struct,
        Self::Pointer,
        Self::Array,
        Self::Slice,
        Self::Map,
        Self::Chan,
    ];

    const fn name(self) -> &'static str {
        match self {
            Self::Declaration => "declaration",
            Self::Definition => "definition",
            Self::Readonly => "readonly",
            Self::DefaultLibrary => "defaultLibrary",
            Self::Static => "static",
            Self::Signature => "signature",
            Self::Number => "number",
            Self::Bool => "bool",
            Self::String => "string",
            Self::Interface => "interface",
            Self::Struct => "struct",
            Self::Pointer => "pointer",
            Self::Array => "array",
            Self::Slice => "slice",
            Self::Map => "map",
            Self::Chan => "chan",
            Self::Shadowing => "shadowing",
            Self::Format => "format",
        }
    }

    const fn bit(self) -> u64 {
        1 << self as usize
    }
}

#[derive(Clone, Copy)]
pub(crate) struct SemanticConfig {
    pub(crate) enabled: bool,
    types: [Option<u8>; SemanticTokenKind::ALL.len()],
    modifiers: [Option<u8>; SemanticModifier::ALL.len()],
}

impl Default for SemanticConfig {
    fn default() -> Self {
        Self::from_initialize(&Value::Null, true)
    }
}

impl SemanticConfig {
    pub(super) fn type_shapes(&self) -> bool {
        SemanticModifier::SHAPES
            .iter()
            .any(|modifier| self.modifiers[*modifier as usize].is_some())
    }

    pub(crate) fn from_initialize(params: &Value, enabled: bool) -> Self {
        let capabilities = &params["capabilities"]["textDocument"]["semanticTokens"];
        let requested = capabilities["tokenTypes"].as_array();
        let mut next = 0;
        let types = SemanticTokenKind::ALL.map(|kind| {
            requested
                .is_none_or(|types| {
                    types
                        .iter()
                        .any(|value| value.as_str() == Some(kind.name()))
                })
                .then(|| {
                    let id = next;
                    next += 1;
                    id
                })
        });
        let requested = capabilities["tokenModifiers"].as_array();
        let mut next = 0;
        let modifiers = SemanticModifier::ALL.map(|modifier| {
            requested
                .is_none_or(|values| {
                    values
                        .iter()
                        .any(|value| value.as_str() == Some(modifier.name()))
                })
                .then(|| {
                    let id = next;
                    next += 1;
                    id
                })
        });
        Self {
            enabled,
            types,
            modifiers,
        }
    }

    pub(super) fn type_index(&self, kind: SemanticTokenKind) -> Option<u8> {
        self.types[kind as usize].or_else(|| {
            (kind == SemanticTokenKind::TypeParameter)
                .then_some(self.types[SemanticTokenKind::Type as usize])
                .flatten()
        })
    }

    pub(super) fn modifier_bits(&self, mut flags: u64) -> u64 {
        if flags & DEFINITION != 0
            && self.modifiers[SemanticModifier::Definition as usize].is_none()
            && self.modifiers[SemanticModifier::Declaration as usize].is_some()
        {
            // Older clients only understand declaration; definitions retain
            // that presentation when the more precise modifier is unavailable.
            flags |= SemanticModifier::Declaration.bit();
        }
        SemanticModifier::ALL
            .iter()
            .zip(self.modifiers)
            .fold(0, |bits, (modifier, id)| {
                bits | id
                    .filter(|_| flags & modifier.bit() != 0)
                    .map_or(0, |id| 1 << id)
            })
    }

    pub(crate) fn legend(&self) -> SemanticTokensLegend {
        SemanticTokensLegend {
            token_types: SemanticTokenKind::ALL
                .iter()
                .zip(self.types)
                .filter_map(|(kind, id)| {
                    id.map(|_| SemanticTokenType::from(kind.name().to_owned()))
                })
                .collect(),
            token_modifiers: SemanticModifier::ALL
                .iter()
                .zip(self.modifiers)
                .filter_map(|(modifier, id)| {
                    id.map(|_| SemanticTokenModifier::from(modifier.name().to_owned()))
                })
                .collect(),
        }
    }
}
// Relative UTF-16 encoding emits single-line, nonempty semantic tokens.
//
// A range request installs a byte window; a token line segment is emitted only
// when its half-open extent intersects the window, mirroring gopls'
// `tokenVisitor.token` check (`start < end && end > start`). A zero-width
// window therefore keeps exactly the tokens that strictly contain it.
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

    pub fn push(&mut self, span: Range<usize>, kind: SemanticTokenKind, modifiers: u64) {
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
