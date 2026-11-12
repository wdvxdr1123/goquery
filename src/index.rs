mod identifiers;

use crate::bloom::BloomFilter;
use crate::syntax::Import;
use crate::text::SourceMap;
use identifiers::IdentifierFilter;
use std::ops::Range;
use tower_lsp::lsp_types::Range as LspRange;

pub use crate::syntax::SymbolKind;

pub const MAX_FILE_BYTES: u64 = 15 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct IndexedSymbol {
    syntax: crate::syntax::Symbol,
    declaration: [u32; 2],
    pub start: usize,
    pub end: usize,
    pub selection: LspRange,
    pub range: LspRange,
    /// Header/content digest for this declaration (name, kind, receiver and
    /// written signature/type/initializer bytes). Used to diff which declaration
    /// names changed between two edits; no source text is retained.
    pub(crate) digest: u64,
}

impl std::ops::Deref for IndexedSymbol {
    type Target = crate::syntax::Symbol;

    fn deref(&self) -> &Self::Target {
        &self.syntax
    }
}

impl IndexedSymbol {
    /// Stub byte spans avoid rebuilding a whole-file UTF-16 map for type headers.
    /// Server source files are bounded below u32::MAX; oversized standalone stub
    /// inputs have no usable compact span rather than a wrapped byte offset.
    pub(crate) fn declaration(&self) -> Option<Range<usize>> {
        (self.declaration[1] != u32::MAX)
            .then_some(self.declaration[0] as usize..self.declaration[1] as usize)
    }

    pub fn is_type(&self) -> bool {
        self.kind == SymbolKind::Type
    }
    pub fn is_function(&self) -> bool {
        self.kind == SymbolKind::Function
    }
    pub fn is_method(&self) -> bool {
        self.kind == SymbolKind::Method
    }
    pub fn is_var(&self) -> bool {
        self.kind == SymbolKind::Variable
    }
    pub fn is_const(&self) -> bool {
        self.kind == SymbolKind::Constant
    }
    /// Package-level variables and constants.
    pub fn is_value(&self) -> bool {
        matches!(self.kind, SymbolKind::Variable | SymbolKind::Constant)
    }
    pub fn is_callable(&self) -> bool {
        matches!(self.kind, SymbolKind::Function | SymbolKind::Method)
    }
    /// Types, variables and constants participate in lexical name resolution;
    /// funcs/methods are matched separately by callability.
    pub fn is_nameable(&self) -> bool {
        matches!(
            self.kind,
            SymbolKind::Type | SymbolKind::Variable | SymbolKind::Constant
        )
    }
}

/// Cheap disk identity for the poller: modification time (Unix nanoseconds)
/// and length. `None` modification time falls back to comparing the length.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct DiskStamp {
    pub(crate) modified: Option<u64>,
    pub(crate) len: u64,
}

impl DiskStamp {
    pub(crate) fn of(path: &std::path::Path) -> Option<Self> {
        let metadata = std::fs::metadata(path).ok()?;
        Some(Self {
            modified: metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|duration| duration.as_nanos() as u64),
            len: metadata.len(),
        })
    }

    /// Current wall clock in the same units as [`Self::modified`].
    pub(crate) fn now_nanos() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos() as u64)
            .unwrap_or(0)
    }
}

#[derive(Default)]
pub struct IndexedFile {
    pub(crate) identifiers: IdentifierFilter,
    pub(crate) import_paths: BloomFilter<1>,
    pub(crate) declarations: BloomFilter<4>,
    pub package: String,
    pub imports: Vec<Import>,
    pub symbols: Vec<IndexedSymbol>,
    pub(crate) constant_groups: Vec<[u32; 2]>,
    /// Hash of every declaration-header byte range (signatures, type bodies,
    /// initializer expressions). Body-only edits leave it unchanged. It is
    /// fixed metadata for deciding when one file's edit can invalidate another
    /// file's diagnostics; no source text is retained.
    pub(crate) declaration_digest: u64,
    /// Disk identity recorded when this stub was indexed; the disk poller uses
    /// it to notice direct (agent/shell) edits without editor events.
    pub(crate) stamp: Option<DiskStamp>,
}

impl IndexedFile {
    pub(crate) fn rebuild_import_filter(&mut self) {
        self.import_paths = BloomFilter::new();
        for import in &self.imports {
            self.import_paths.insert(import.path.as_bytes());
        }
    }
}

pub fn index_source(source: &str) -> IndexedFile {
    let parsed = crate::syntax::stub(source);
    let declaration_digest = declaration_digest(source, &parsed.symbols);
    let map = SourceMap::new(source);
    let mut import_paths = BloomFilter::new();
    let mut declarations = BloomFilter::new();
    for symbol in &parsed.symbols {
        if symbol.receiver.is_none() {
            declarations.insert(symbol.name.as_bytes());
        }
    }
    for import in &parsed.imports {
        import_paths.insert(import.path.as_bytes());
    }
    IndexedFile {
        identifiers: IdentifierFilter::new(source),
        import_paths,
        declarations,
        package: parsed.package,
        imports: parsed.imports,
        constant_groups: parsed
            .constant_groups
            .into_iter()
            .filter_map(|span| {
                Some([
                    u32::try_from(span.start).ok()?,
                    u32::try_from(span.end).ok()?,
                ])
            })
            .collect(),
        declaration_digest,
        stamp: None,
        symbols: parsed
            .symbols
            .into_iter()
            .map(|syntax| {
                let declaration = [
                    u32::try_from(syntax.range.start).unwrap_or(u32::MAX),
                    u32::try_from(syntax.range.end).unwrap_or(u32::MAX),
                ];
                let start = syntax.selection.start;
                let end = syntax.selection.end;
                let selection = map.range(syntax.selection.start, syntax.selection.end);
                let range = map.range(syntax.range.start, syntax.range.end);
                let digest = symbol_digest(source, &syntax);
                IndexedSymbol {
                    syntax,
                    declaration,
                    start,
                    end,
                    selection,
                    range,
                    digest,
                }
            })
            .collect(),
    }
}

/// Digest one declaration's identity and written header. Function bodies are
/// excluded (their start is the `{`); all other declarations use their range,
/// so type bodies and initializer expressions participate.
fn symbol_digest(source: &str, symbol: &crate::syntax::Symbol) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    symbol.name.hash(&mut hasher);
    (symbol.kind as u8).hash(&mut hasher);
    symbol.generic.hash(&mut hasher);
    symbol.alias.hash(&mut hasher);
    symbol.leaf.hash(&mut hasher);
    (symbol.constructor as u8).hash(&mut hasher);
    symbol.receiver.hash(&mut hasher);
    let end = symbol
        .body
        .as_ref()
        .map_or(symbol.range.end, |body| body.start);
    if let Some(header) = source.get(symbol.range.start.min(end)..end) {
        header.hash(&mut hasher);
    }
    hasher.finish()
}

/// File-wide digest of declaration headers. Imports and bodies are excluded, so
/// an import-only or body-only edit does not look like a declaration change.
fn declaration_digest(source: &str, symbols: &[crate::syntax::Symbol]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for symbol in symbols {
        let end = symbol
            .body
            .as_ref()
            .map_or(symbol.range.end, |body| body.start);
        if let Some(header) = source.get(symbol.range.start.min(end)..end) {
            header.hash(&mut hasher);
        }
    }
    hasher.finish()
}
