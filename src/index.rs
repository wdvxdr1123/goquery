mod identifiers;

use crate::bloom::BloomFilter;
use crate::syntax::{Import, TypeConstructor, TypeInfo};
use crate::text::SourceMap;
use identifiers::IdentifierFilter;
use std::ops::Range;
use tower_lsp::lsp_types::Range as LspRange;

pub use crate::syntax::SymbolKind;

pub const MAX_FILE_BYTES: u64 = 15 * 1024 * 1024;

/// Shared declaration facts with elided syntax spans and compact body offsets.
type CompactSymbol = crate::syntax::Symbol<(), [u32; 2]>;

#[derive(Clone, Debug)]
pub struct IndexedSymbol {
    syntax: CompactSymbol,
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
    type Target = CompactSymbol;

    fn deref(&self) -> &Self::Target {
        &self.syntax
    }
}

impl IndexedSymbol {
    /// A 15 MiB source fits in compact offsets. Reconstruct function-body spans
    /// only when queried; the sentinel represents an absent/unavailable body.
    pub(crate) fn body(&self) -> Option<Range<usize>> {
        let [start, end] = self.syntax.body;
        (end != u32::MAX).then_some(start as usize..end as usize)
    }

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
        Some(Self::from_metadata(&metadata))
    }

    pub(crate) fn from_metadata(metadata: &std::fs::Metadata) -> Self {
        Self {
            modified: metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|duration| duration.as_nanos() as u64),
            len: metadata.len(),
        }
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
    /// Workspace-selected host/preferred-port memberships; no source is retained.
    pub(crate) build_targets: u64,
    /// Minimum release implied by the leading build expression, without text.
    pub(crate) build_go_version: Option<u16>,
    /// True when any type declaration in this file may contain interface methods
    /// not covered by `declared_methods` (unparsed/parenthesized/incomplete
    /// interfaces or unknown type constructors). Packed into existing padding.
    pub(crate) methods_opaque: bool,
    /// True when any type declaration in this file shadows a predeclared
    /// universe type name (`int`, `error`, `any`, etc.). Packed into padding.
    pub(crate) has_universe_shadow: bool,
    /// True when any type declaration in this file is a type alias (`type A = B`).
    /// Packed into existing padding.
    pub(crate) has_type_alias: bool,
    /// 64-bit method-name bitmask (`TypeInfo::method_name_bit`) of all receiver
    /// methods (`func (R) M`) and direct interface methods declared in this file.
    /// Used strictly as a sound necessary-condition negative filter (+8 bytes/file).
    pub(crate) declared_methods: u64,
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

#[cfg(target_pointer_width = "64")]
const _: () = assert!(std::mem::size_of::<IndexedFile>() <= 456);

impl IndexedFile {
    pub(crate) fn rebuild_import_filter(&mut self) {
        self.import_paths = BloomFilter::new();
        for import in &self.imports {
            self.import_paths.insert(import.path.as_bytes());
        }
    }
}

pub fn index_source(source: &str) -> IndexedFile {
    let mut parsed = crate::syntax::stub(source);
    // These lists become immutable index metadata. Parser growth capacity is
    // useful during extraction but need not remain live for every source file.
    parsed.imports.shrink_to_fit();
    for symbol in &mut parsed.symbols {
        if let Some(info) = &mut symbol.type_info {
            info.embedded.shrink_to_fit();
            info.members.shrink_to_fit();
            info.search_members.shrink_to_fit();
        }
    }
    let declaration_digest = declaration_digest(source, &parsed.symbols);
    let map = SourceMap::new(source);
    let mut import_paths = BloomFilter::new();
    let mut declarations = BloomFilter::new();
    let mut declared_methods = 0u64;
    let mut methods_opaque = false;
    let mut has_universe_shadow = false;
    let mut has_type_alias = false;
    for symbol in &parsed.symbols {
        if symbol.receiver.is_none() {
            declarations.insert(symbol.name.as_bytes());
        }
        if symbol.kind == SymbolKind::Method {
            if symbol.name != "_" {
                declared_methods |= TypeInfo::method_name_bit(&symbol.name);
            }
        } else if symbol.kind == SymbolKind::Type {
            if symbol.alias {
                has_type_alias = true;
            }
            if is_universe_type_shadow(&symbol.name) {
                has_universe_shadow = true;
            }
            if let Some(info) = &symbol.type_info {
                declared_methods |= info.direct_methods_mask;
            }
            if symbol.constructor == TypeConstructor::Unknown
                || (symbol.constructor == TypeConstructor::Interface
                    && symbol
                        .type_info
                        .as_ref()
                        .is_none_or(|info| !info.embeddings_complete))
            {
                methods_opaque = true;
            }
        }
    }
    for import in &parsed.imports {
        import_paths.insert(import.path.as_bytes());
    }
    IndexedFile {
        build_targets: 0,
        build_go_version: crate::buildconstraint::minimum_go_version(source),
        methods_opaque,
        has_universe_shadow,
        has_type_alias,
        declared_methods,
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
                    syntax: syntax.map_spans(
                        |_| (),
                        |body| {
                            body.and_then(|span| {
                                Some([
                                    u32::try_from(span.start).ok()?,
                                    u32::try_from(span.end).ok()?,
                                ])
                            })
                            .unwrap_or([u32::MAX; 2])
                        },
                    ),
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

fn is_universe_type_shadow(name: &str) -> bool {
    matches!(
        name,
        "bool"
            | "byte"
            | "rune"
            | "string"
            | "int"
            | "int8"
            | "int16"
            | "int32"
            | "int64"
            | "uint"
            | "uint8"
            | "uint16"
            | "uint32"
            | "uint64"
            | "uintptr"
            | "float32"
            | "float64"
            | "complex64"
            | "complex128"
            | "any"
            | "error"
            | "comparable"
    )
}

#[cfg(test)]
mod tests {
    use super::{IndexedFile, index_source};
    use crate::syntax::TypeInfo;

    #[test]
    fn method_bitmasks_and_index_flags_match_declaration_semantics() {
        for name in [
            "Read", "Write", "Close", "Error", "String", "Len", "Less", "Swap",
        ] {
            assert_eq!(TypeInfo::method_name_bit(name).count_ones(), 1);
        }

        let stub = index_source(
            "package p\n\
             type Plain interface {\n\
                 Read([]byte) (int, error)\n\
                 Write([]byte) (int, error)\n\
             }\n\
             type StructWithField struct {\n\
                 Read int\n\
             }\n\
             type Box[T any] struct { v T }\n\
             func (b *Box[T]) Get() T { return b.v }\n\
             func (StructWithField) _() {}\n\
             type Alias = StructWithField\n",
        );
        assert!(!stub.methods_opaque);
        assert!(!stub.has_universe_shadow);
        assert!(stub.has_type_alias);

        let plain = stub.symbols.iter().find(|s| s.name == "Plain").unwrap();
        let plain_mask = plain.type_info.as_ref().unwrap().direct_methods_mask;
        assert_eq!(
            plain_mask,
            TypeInfo::method_name_bit("Read") | TypeInfo::method_name_bit("Write")
        );

        let st = stub
            .symbols
            .iter()
            .find(|s| s.name == "StructWithField")
            .unwrap();
        assert_eq!(st.type_info.as_ref().unwrap().direct_methods_mask, 0);

        // File-level mask captures interface methods and receiver methods, skipping `_`.
        let expected_file_mask = TypeInfo::method_name_bit("Read")
            | TypeInfo::method_name_bit("Write")
            | TypeInfo::method_name_bit("Get");
        assert_eq!(stub.declared_methods, expected_file_mask);

        // Interfaces with union / generic type elements still record direct
        // explicit methods without mistaking union terms for methods, and mark
        // the file opaque for conservative fallback.
        let constraint = index_source(
            "package p\n\
             type Constraint interface {\n\
                 ~int | string\n\
                 Other[int]\n\
                 Direct() int\n\
             }\n\
             type int struct{}\n",
        );
        assert!(constraint.methods_opaque);
        assert!(constraint.has_universe_shadow);
        let c_sym = constraint
            .symbols
            .iter()
            .find(|s| s.name == "Constraint")
            .unwrap();
        let c_info = c_sym.type_info.as_ref().unwrap();
        assert_eq!(
            c_info.direct_methods_mask,
            TypeInfo::method_name_bit("Direct")
        );

        // Unclosed malformed interfaces must leave direct_methods_mask = 0 and mark opaque.
        let broken = index_source("package p\ntype Broken interface {\n\tFoo(\n");
        assert!(broken.methods_opaque);
        let b_sym = broken.symbols.iter().find(|s| s.name == "Broken").unwrap();
        assert_eq!(b_sym.type_info.as_ref().unwrap().direct_methods_mask, 0);

        // Parenthesized interfaces have no TypeInfo and mark the file opaque.
        let paren = index_source("package p\ntype Paren (interface { Magic() })\n");
        assert!(paren.methods_opaque);

        // Permanent memory footprint stays strictly bounded.
        #[cfg(target_pointer_width = "64")]
        {
            assert!(std::mem::size_of::<TypeInfo>() <= 128);
            assert!(std::mem::size_of::<IndexedFile>() <= 456);
        }
    }
}
