//! Unified semantic object identities replacing ad-hoc resolution structs.

use crate::analysis::budget::MAX_BYTES;
use crate::index::IndexedSymbol;
use crate::syntax::{AggregateMember, MemberKind, TokenKind};
use crate::text::SourceMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use tower_lsp::lsp_types::Range;

/// Compact lexical scope identifier within a `ScopeTree`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ScopeId(pub(crate) u32);

/// Compact local binding identifier within a `ScopeTree`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct LocalId(pub(crate) u32);

/// Canonical identity of a package-level or receiver-associated declaration.
/// Independent of import aliasing or consuming file spelling.
#[derive(Clone, Copy, Debug)]
pub(crate) struct DeclId<'a> {
    pub(crate) file: &'a Path,
    pub(crate) symbol: &'a IndexedSymbol,
}

impl PartialEq for DeclId<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.file == other.file && self.symbol.start == other.symbol.start
    }
}

impl Eq for DeclId<'_> {}

impl Hash for DeclId<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.file.hash(state);
        self.symbol.start.hash(state);
    }
}

/// Origin of a type, constant, or value declaration.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum SymbolOrigin<'a> {
    Package(DeclId<'a>),
    Local {
        file: &'a Path,
        id: LocalId,
        span: [u32; 2],
    },
}

/// Canonical identity of a struct field or interface method member.
#[derive(Clone, Copy, Debug)]
pub(crate) struct MemberDeclId<'a> {
    pub(crate) file: &'a Path,
    pub(crate) owner: Option<&'a IndexedSymbol>,
    pub(crate) selection: [u32; 2],
    pub(crate) embedded: bool,
    pub(crate) builtin: bool,
}

impl<'a> MemberDeclId<'a> {
    pub(crate) fn to_member_declaration(
        self,
        workspace: &'a crate::workspace::Workspace,
    ) -> Option<MemberDeclaration<'a>> {
        let [start, end] = self.selection;
        let (start, end) = (start as usize, end as usize);
        if start >= end || end.saturating_sub(start) > 1024 {
            return None;
        }
        let (file, stub) = workspace.files.get_key_value(self.file)?;
        let owner = self
            .owner
            .filter(|sym| {
                stub.symbols.iter().any(|s| std::ptr::eq(s, *sym))
                    && sym
                        .declaration()
                        .is_some_and(|decl| decl.start <= start && end <= decl.end)
            })
            .or_else(|| {
                stub.symbols.iter().find(|sym| {
                    sym.is_type()
                        && sym
                            .declaration()
                            .is_some_and(|decl| decl.start <= start && end <= decl.end)
                })
            })?;
        let info = owner.type_info.as_ref()?;
        let raw_name = workspace.source_slice(file, start..end)?;
        let target = raw_name.as_ref();
        let name: &'a str = if self.embedded {
            info.embedded.iter().find_map(|entry| {
                let head = entry.split('[').next().unwrap_or(entry.as_str());
                let bare = head.trim_start_matches('*').rsplit('.').next()?;
                (bare == target).then_some(bare)
            })?
        } else {
            info.members
                .iter()
                .find(|m| m.as_str() == target)
                .map(String::as_str)
                .or_else(|| {
                    info.search_members
                        .iter()
                        .find(|m| m.name == target)
                        .map(|m| m.name.as_str())
                })?
        };
        let kind = if info.is_interface() {
            MemberKind::Method
        } else {
            MemberKind::Field
        };
        Some(MemberDeclaration {
            file,
            owner,
            name,
            kind,
            embedded: self.embedded,
        })
    }
}

#[derive(Clone, Copy)]
pub(crate) struct MemberDeclaration<'a> {
    pub file: &'a PathBuf,
    pub owner: &'a IndexedSymbol,
    pub name: &'a str,
    pub kind: MemberKind,
    pub embedded: bool,
}

#[derive(Clone, Copy)]
pub(crate) enum ReceiverMember<'a> {
    Method(&'a PathBuf, &'a IndexedSymbol),
    Declaration(MemberDeclaration<'a>),
}

pub(crate) struct MemberData {
    pub selection: Range,
    pub annotation: Option<String>,
}

impl MemberDeclaration<'_> {
    pub(crate) fn selection(
        &self,
        declaration: &str,
        entry: &AggregateMember,
    ) -> Option<std::ops::Range<usize>> {
        if !self.embedded {
            return Some(entry.selection.clone());
        }
        let annotation = declaration.get(entry.annotation.clone())?;
        let token = crate::syntax::Lexer::new(annotation)
            .take_while(|token| token.kind != TokenKind::LBrack)
            .filter(|token| token.kind == TokenKind::Ident)
            .last()?;
        Some(entry.annotation.start + token.span.start..entry.annotation.start + token.span.end)
    }

    pub(crate) fn same(&self, other: &Self) -> bool {
        self.file == other.file && self.owner.start == other.owner.start && self.name == other.name
    }

    pub(crate) fn entry<'a>(
        &self,
        declaration: &str,
        entries: &'a [AggregateMember],
    ) -> Option<&'a AggregateMember> {
        let root = crate::syntax::aggregate_offset(declaration)?;
        entries.iter().find(|entry| {
            entry.container == root
                && if self.embedded {
                    entry.selection.start == entry.annotation.start
                        && crate::analysis::scope::named_root(
                            entry.symbol.name.trim_start_matches('*'),
                        )
                        .rsplit('.')
                        .next()
                            == Some(self.name)
                } else {
                    entry.symbol.name == self.name
                }
        })
    }

    pub(crate) fn data(self, workspace: &crate::workspace::Workspace) -> Option<MemberData> {
        let source = workspace.source(self.file)?;
        let map = SourceMap::new(&source);
        self.data_in_source(&source, &map)
    }

    pub(crate) fn data_in_source(self, source: &str, map: &SourceMap<'_>) -> Option<MemberData> {
        let base = self.owner.end;
        let declaration = source.get(base..map.offset(self.owner.range.end)?)?;
        let entries = crate::syntax::aggregate_members(declaration);
        let entry = self.entry(declaration, &entries)?;
        let selection = self.selection(declaration, entry)?;
        Some(MemberData {
            selection: map.range(base + selection.start, base + selection.end),
            annotation: (entry.annotation.len() <= MAX_BYTES)
                .then(|| declaration.get(entry.annotation.clone()).map(str::to_owned))
                .flatten(),
        })
    }
}

impl PartialEq for MemberDeclId<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.file == other.file
            && self.selection == other.selection
            && self.builtin == other.builtin
            && self.owner.map(|s| s.start) == other.owner.map(|s| s.start)
    }
}

impl Eq for MemberDeclId<'_> {}

impl Hash for MemberDeclId<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.file.hash(state);
        self.selection.hash(state);
        self.builtin.hash(state);
        self.owner.map(|s| s.start).hash(state);
    }
}

/// Origin of a resolved method object.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum MethodOrigin<'a> {
    Concrete(DeclId<'a>),
    Interface(MemberDeclId<'a>),
}

/// Classification of Go variable bindings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum VarKind {
    Package,
    Local,
    Param,
    ResultVar,
    Receiver,
}

/// Go universe predeclared built-in functions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum BuiltinFunction {
    Append,
    Cap,
    Clear,
    Close,
    Complex,
    Copy,
    Delete,
    Imag,
    Len,
    Make,
    Max,
    Min,
    New,
    Panic,
    Print,
    Println,
    Real,
    Recover,
}

impl BuiltinFunction {
    pub(crate) fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "append" => Self::Append,
            "cap" => Self::Cap,
            "clear" => Self::Clear,
            "close" => Self::Close,
            "complex" => Self::Complex,
            "copy" => Self::Copy,
            "delete" => Self::Delete,
            "imag" => Self::Imag,
            "len" => Self::Len,
            "make" => Self::Make,
            "max" => Self::Max,
            "min" => Self::Min,
            "new" => Self::New,
            "panic" => Self::Panic,
            "print" => Self::Print,
            "println" => Self::Println,
            "real" => Self::Real,
            "recover" => Self::Recover,
            _ => return None,
        })
    }

    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Append => "append",
            Self::Cap => "cap",
            Self::Clear => "clear",
            Self::Close => "close",
            Self::Complex => "complex",
            Self::Copy => "copy",
            Self::Delete => "delete",
            Self::Imag => "imag",
            Self::Len => "len",
            Self::Make => "make",
            Self::Max => "max",
            Self::Min => "min",
            Self::New => "new",
            Self::Panic => "panic",
            Self::Print => "print",
            Self::Println => "println",
            Self::Real => "real",
            Self::Recover => "recover",
        }
    }
}

/// Unified identity of a resolved Go semantic object (`go/types.Object` analog).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ObjectRef<'a> {
    PkgName {
        import_path_span: [u32; 2],
        target_dir: &'a Path,
    },
    Const(SymbolOrigin<'a>),
    TypeName {
        origin: SymbolOrigin<'a>,
        is_alias: bool,
    },
    TypeParam {
        owner: Option<DeclId<'a>>,
        index: u16,
        span: [u32; 2],
    },
    Var {
        origin: SymbolOrigin<'a>,
        kind: VarKind,
    },
    Field(MemberDeclId<'a>),
    Func(DeclId<'a>),
    Method(MethodOrigin<'a>),
    BuiltinFunction(BuiltinFunction),
    BuiltinType(&'static str),
    BuiltinConst(&'static str),
    Nil,
    Label {
        file: &'a Path,
        span: [u32; 2],
    },
}

#[cfg(target_pointer_width = "64")]
const _: () = assert!(std::mem::size_of::<ObjectRef<'static>>() <= 48);
