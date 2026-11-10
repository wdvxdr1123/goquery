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
#[derive(Default)]
pub struct IndexedFile {
    pub(crate) identifiers: IdentifierFilter,
    pub(crate) import_paths: BloomFilter<1>,
    pub(crate) declarations: BloomFilter<4>,
    pub package: String,
    pub imports: Vec<Import>,
    pub symbols: Vec<IndexedSymbol>,
    pub(crate) constant_groups: Vec<[u32; 2]>,
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
                IndexedSymbol {
                    syntax,
                    declaration,
                    start,
                    end,
                    selection,
                    range,
                }
            })
            .collect(),
    }
}
