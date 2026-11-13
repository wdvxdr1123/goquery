use std::ops::Range;

/// Go declaration categories, independent of editor protocol numeric values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SymbolKind {
    Type,
    Constant,
    Variable,
    Function,
    Method,
}

/// Written underlying-constructor category of a named type declaration. Only a
/// declaration-header fact: `Named`/`Unknown` never prove a negative relation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum TypeConstructor {
    Unknown,
    Named,
    Struct,
    Interface,
    Map,
    Chan,
    Func,
    Pointer,
    Array,
    Slice,
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Symbol<Span = Range<usize>, Body = Option<Range<usize>>> {
    pub name: String,
    pub kind: SymbolKind,
    /// Declaration-header facts, extracted once with the syntax stub.
    pub generic: bool,
    pub alias: bool,
    pub leaf: bool,
    pub constructor: TypeConstructor,
    pub selection: Span,
    pub range: Span,
    pub receiver: Option<Box<str>>,
    /// Opaque body span; consumers can parse this region on demand.
    pub body: Body,
    pub type_info: Option<Box<TypeInfo>>,
}

impl<Span, Body> Symbol<Span, Body> {
    /// Indexed declarations keep positions separately and can elide these
    /// syntax spans without duplicating the shared declaration metadata.
    pub(crate) fn map_spans<Mapped, MappedBody>(
        self,
        mut map: impl FnMut(Span) -> Mapped,
        map_body: impl FnOnce(Body) -> MappedBody,
    ) -> Symbol<Mapped, MappedBody> {
        Symbol {
            name: self.name,
            kind: self.kind,
            generic: self.generic,
            alias: self.alias,
            leaf: self.leaf,
            constructor: self.constructor,
            selection: map(self.selection),
            range: map(self.range),
            receiver: self.receiver,
            body: map_body(self.body),
            type_info: self.type_info,
        }
    }
}

/// Declaration metadata only; aggregate members are not retained as syntax trees.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct TypeInfo {
    pub constructor: TypeConstructor,
    /// Conservative proof for metadata-only negative embedding searches.
    /// False means the inventory may omit opaque or bounded member syntax.
    pub embeddings_complete: bool,
    /// Named alias or defined underlying type, without a retained type tree.
    pub target: Option<TypeTarget>,
    /// Shallow named embeddings; search entries add no edges or substitutions.
    pub embedded: Vec<String>,
    /// Explicit member names that can shadow promoted selectors.
    pub members: Vec<String>,
    /// Additional search names. These do not assert selector/type relationships.
    pub search_members: Vec<MemberName>,
}

impl TypeInfo {
    pub fn is_interface(&self) -> bool {
        self.constructor == TypeConstructor::Interface
    }
}

/// Compact named type indirection. Generic arguments remain syntax spellings.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct TypeTarget {
    /// Target spelling, bounded to 16 KiB.
    pub name: String,
    pub alias: bool,
    pub generic: bool,
}

/// Aggregate member categories, independent of protocol numeric values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum MemberKind {
    Field,
    Method,
    Interface,
}

/// Compact search metadata without source positions or a syntax tree.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct MemberName {
    pub name: String,
    pub kind: MemberKind,
}

/// Query-local member name and source selection, relative to the type expression.
#[derive(Debug)]
#[non_exhaustive]
pub struct AggregateMember {
    pub symbol: MemberName,
    pub selection: Range<usize>,
    /// Field type or interface method parameter/result signature.
    pub annotation: Range<usize>,
    /// Start of the containing struct/interface type expression.
    pub container: usize,
}
/// The local name written by an import specification.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImportName {
    /// No written name; package resolution supplies the declared package name.
    Package,
    Alias(String),
    Dot,
    Blank,
}

/// An import retains its written name; implicit names belong to package resolution.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Import {
    pub name: ImportName,
    /// Decoded UTF-8 import path.
    pub path: String,
}

impl Import {
    /// Explicit source-level name, including `.` and `_`; `None` means no alias.
    pub fn explicit_name(&self) -> Option<&str> {
        match &self.name {
            ImportName::Package => None,
            ImportName::Alias(name) => Some(name),
            ImportName::Dot => Some("."),
            ImportName::Blank => Some("_"),
        }
    }

    pub fn is_dot(&self) -> bool {
        self.name == ImportName::Dot
    }

    pub fn is_blank(&self) -> bool {
        self.name == ImportName::Blank
    }
}

#[derive(Default)]
#[non_exhaustive]
pub struct Stub {
    pub package: String,
    pub imports: Vec<Import>,
    pub symbols: Vec<Symbol>,
    /// Sparse source-only context for grouped constant inheritance and iota.
    pub constant_groups: Vec<Range<usize>>,
}
