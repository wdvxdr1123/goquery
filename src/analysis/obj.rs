//! Unified semantic object identities replacing ad-hoc resolution structs.

use crate::index::IndexedSymbol;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

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
    /// Attach an indexed owner without resolving a source spelling. Anonymous
    /// literals remain outside the indexed-member search domain.
    pub(crate) fn indexed(self, workspace: &'a crate::workspace::Workspace) -> Option<Self> {
        if self.builtin || self.selection[0] >= self.selection[1] {
            return None;
        }
        let stub = workspace.files.get(self.file)?;
        let owner = self
            .owner
            .filter(|owner| {
                owner.type_info.is_some()
                    && stub
                        .symbols
                        .iter()
                        .any(|symbol| std::ptr::eq(symbol, *owner))
                    && owner.declaration().is_some_and(|span| {
                        span.start <= self.selection[0] as usize
                            && self.selection[1] as usize <= span.end
                    })
            })
            .or_else(|| {
                stub.symbols.iter().find(|symbol| {
                    symbol.type_info.is_some()
                        && symbol.declaration().is_some_and(|span| {
                            span.start <= self.selection[0] as usize
                                && self.selection[1] as usize <= span.end
                        })
                })
            })?;
        Some(Self {
            owner: Some(owner),
            ..self
        })
    }
}

/// A lookup label paired with its canonical object. The label is metadata, not
/// an alternative declaration identity or a proof of a member's type.
#[derive(Clone, Copy)]
pub(crate) struct NamedObject<'a> {
    pub(crate) name: &'a str,
    pub(crate) object: ObjectRef<'a>,
}

impl<'a> NamedObject<'a> {
    pub(crate) fn member_id(self) -> Option<MemberDeclId<'a>> {
        match self.object {
            ObjectRef::Field(identity) | ObjectRef::Method(MethodOrigin::Interface(identity)) => {
                Some(identity)
            }
            _ => None,
        }
    }

    pub(crate) fn file(self) -> Option<&'a Path> {
        match self.object {
            ObjectRef::Method(MethodOrigin::Concrete(decl)) => Some(decl.file),
            _ => self.member_id().map(|identity| identity.file),
        }
    }

    pub(crate) fn method(self) -> Option<DeclId<'a>> {
        match self.object {
            ObjectRef::Method(MethodOrigin::Concrete(declaration)) => Some(declaration),
            _ => None,
        }
    }

    pub(crate) fn method_source<'workspace>(
        self,
        workspace: &'workspace crate::workspace::Workspace,
    ) -> Option<(&'workspace PathBuf, &'a IndexedSymbol)> {
        let declaration = self.method()?;
        let (file, _) = workspace.files.get_key_value(declaration.file)?;
        Some((file, declaration.symbol))
    }
}

impl PartialEq for MemberDeclId<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.file == other.file
            && self.selection == other.selection
            && self.builtin == other.builtin
    }
}

impl Eq for MemberDeclId<'_> {}

impl Hash for MemberDeclId<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.file.hash(state);
        self.selection.hash(state);
        self.builtin.hash(state);
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

/// Non-empty byte span `[start, end)` of a type-switch `case` clause (`end > 0`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct CaseScope {
    pub(crate) start: u32,
    pub(crate) end: std::num::NonZeroU32,
}

impl CaseScope {
    pub(crate) fn from_span(span: [u32; 2]) -> Option<Self> {
        let end = std::num::NonZeroU32::new(span[1])?;
        (span[0] < span[1]).then_some(Self {
            start: span[0],
            end,
        })
    }

    pub(crate) fn span(self) -> [u32; 2] {
        [self.start, self.end.get()]
    }
}

/// Source declaration target resolved from a semantic [`Object`].
/// Transition alias while analysis modules adopt [`Object`].
pub(crate) type ObjectRef<'a> = Object<'a>;

/// Unified identity of a resolved Go semantic object (`go/types.Object` analog).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Object<'a> {
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
        /// Type-switch clause span (`case T:`) distinguishing per-clause bindings.
        case_scope: Option<CaseScope>,
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

impl<'a> Object<'a> {
    pub(crate) fn case_scope(self) -> Option<[u32; 2]> {
        match self {
            Self::Var {
                case_scope: Some(scope),
                ..
            } => Some(scope.span()),
            _ => None,
        }
    }

    pub(crate) fn is_local(self) -> bool {
        matches!(
            self,
            Self::Const(SymbolOrigin::Local { .. })
                | Self::TypeName {
                    origin: SymbolOrigin::Local { .. },
                    ..
                }
                | Self::Var {
                    origin: SymbolOrigin::Local { .. },
                    ..
                }
                | Self::TypeParam { .. }
                | Self::Label { .. }
        )
    }

    /// Reference identity equality. When `match_case_scope` is false (`references`
    /// and `rename`), distinct clauses of the same type-switch guard group under
    /// the guard declaration span; when true (`documentHighlight`), each case
    /// clause remains isolated by its clause span and `LocalId`.
    pub(crate) fn same_reference_target(self, other: Self, match_case_scope: bool) -> bool {
        if !match_case_scope
            && let (
                Self::Var {
                    origin:
                        SymbolOrigin::Local {
                            file: f1, span: s1, ..
                        },
                    kind: k1,
                    case_scope: Some(_),
                },
                Self::Var {
                    origin:
                        SymbolOrigin::Local {
                            file: f2, span: s2, ..
                        },
                    kind: k2,
                    case_scope: Some(_),
                },
            ) = (self, other)
        {
            return f1 == f2 && s1 == s2 && k1 == k2;
        }
        self == other
    }
}

pub(crate) fn decl_object(decl: DeclId<'_>) -> Object<'_> {
    match decl.symbol.kind {
        crate::syntax::SymbolKind::Constant => Object::Const(SymbolOrigin::Package(decl)),
        crate::syntax::SymbolKind::Type => Object::TypeName {
            origin: SymbolOrigin::Package(decl),
            is_alias: decl.symbol.alias,
        },
        crate::syntax::SymbolKind::Variable => Object::Var {
            origin: SymbolOrigin::Package(decl),
            kind: VarKind::Package,
            case_scope: None,
        },
        crate::syntax::SymbolKind::Function => Object::Func(decl),
        crate::syntax::SymbolKind::Method => Object::Method(MethodOrigin::Concrete(decl)),
    }
}

#[cfg(target_pointer_width = "64")]
const _: () = assert!(std::mem::size_of::<Object<'static>>() <= 48);
