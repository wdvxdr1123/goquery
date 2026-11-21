use super::BasicKind;
use crate::analysis::engine::budget::RELATION_STEPS;
use crate::analysis::engine::obj::DeclId;
use std::ops::Range;
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UnknownReason {
    BudgetExceeded,
    MissingMetadata,
    SyntaxError,
    Unsupported,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CheckOutcome<T, E = ()> {
    Ok(T),
    Invalid(E),
    Unknown(UnknownReason),
}

impl<T, E> CheckOutcome<T, E> {
    pub(crate) fn ok(self) -> Option<T> {
        match self {
            Self::Ok(value) => Some(value),
            Self::Invalid(_) | Self::Unknown(_) => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CheckMode {
    Concrete,
    Symbolic,
}

impl CheckMode {
    pub(crate) const fn is_symbolic(self) -> bool {
        matches!(self, Self::Symbolic)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AllocTier {
    Decl,
    Local,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct StepBudget {
    remaining: usize,
}

impl StepBudget {
    pub(crate) const fn new(limit: usize) -> Self {
        Self { remaining: limit }
    }

    pub(crate) fn reset(&mut self) {
        self.remaining = RELATION_STEPS;
    }

    pub(crate) fn step(&mut self) -> Option<()> {
        self.remaining = self.remaining.checked_sub(1)?;
        Some(())
    }
}

const TIER_MASK: u32 = 0xC000_0000;
const TIER_BUILTIN: u32 = 0x0000_0000;
const TIER_DECL: u32 = 0x4000_0000;
const TIER_LOCAL: u32 = 0x8000_0000;
const EPOCH_MASK: u16 = 0x7FFF;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum TypeTier {
    Invalid,
    Builtin,
    Decl,
    Local { epoch: u16 },
}

/// 32-bit tiered TypeId isolating permanent declaration types from per-region locals.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct TypeId(pub(super) u32);

impl TypeId {
    /// Permanent first node in every store, also used by non-value operands.
    pub(crate) const INVALID: Self = Self(0);

    pub(crate) const fn basic(kind: BasicKind) -> Self {
        Self(TIER_BUILTIN | ((kind as u32) + 1))
    }

    pub(super) const fn as_basic(self) -> Option<BasicKind> {
        if (self.0 & TIER_MASK) != TIER_BUILTIN || self.0 == 0 {
            return None;
        }
        BasicKind::from_index((self.0 - 1) as usize)
    }

    pub(super) const fn decl(index: u16) -> Self {
        Self(TIER_DECL | (index as u32))
    }

    pub(super) const fn local(epoch: u16, index: u16) -> Self {
        Self(TIER_LOCAL | (((epoch & EPOCH_MASK) as u32) << 16) | (index as u32))
    }

    pub(crate) const fn tier(self) -> TypeTier {
        match self.0 & TIER_MASK {
            TIER_BUILTIN if self.0 == 0 => TypeTier::Invalid,
            TIER_BUILTIN => TypeTier::Builtin,
            TIER_DECL => TypeTier::Decl,
            TIER_LOCAL => TypeTier::Local {
                epoch: ((self.0 >> 16) as u16) & EPOCH_MASK,
            },
            _ => TypeTier::Invalid,
        }
    }

    pub(super) const fn index(self) -> usize {
        (self.0 & 0xFFFF) as usize
    }

    pub(crate) const fn is_local(self) -> bool {
        (self.0 & TIER_MASK) == TIER_LOCAL
    }

    pub(crate) const fn is_decl(self) -> bool {
        (self.0 & TIER_MASK) == TIER_DECL
    }

    pub(crate) const fn is_decl_safe(self) -> bool {
        !self.is_local()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(super) struct SliceRef {
    pub(super) start: u32,
    pub(super) len: u16,
}

impl SliceRef {
    pub(super) fn from_range(range: Range<usize>) -> Option<Self> {
        Some(Self {
            start: u32::try_from(range.start).ok()?,
            len: u16::try_from(range.len()).ok()?,
        })
    }

    pub(super) fn range(self) -> Range<usize> {
        let start = self.start as usize;
        start..start + usize::from(self.len)
    }

    pub(super) const fn len(self) -> usize {
        self.len as usize
    }

    pub(super) const fn is_empty(self) -> bool {
        self.len == 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct OriginId(pub(super) u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct StructId(pub(super) u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct InterfaceId(pub(super) u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct SignatureId(pub(super) u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct TypeParamId(pub(super) u32);

#[derive(Clone, PartialEq, Eq, Hash)]
pub(super) enum TypeOrigin<'a> {
    Package(DeclId<'a>),
    Local {
        origin: &'a Path,
        start: u32,
        end: u32,
        formal: bool,
    },
    BuiltinError {
        interface: TypeId,
    },
}

#[derive(Clone)]
pub(super) struct StructData<'a> {
    pub(super) origin: &'a Path,
    pub(super) fields: Range<usize>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Term {
    pub(super) ty: TypeId,
    pub(super) approximate: bool,
}

#[derive(Clone)]
pub(super) struct InterfaceData<'a> {
    pub(super) origin: &'a Path,
    pub(super) methods: Range<usize>,
    pub(super) embeddeds: SliceRef,
    pub(super) has_type_elements: bool,
    pub(super) comparable: bool,
    pub(super) builtin_any: bool,
}

#[derive(Clone)]
pub(super) struct SignatureData {
    pub(super) type_params: SliceRef,
    pub(super) parameters: Range<usize>,
    pub(super) results: Range<usize>,
    pub(super) variadic: bool,
}

#[derive(Clone, Copy)]
pub(super) struct TypeParamData {
    pub(super) index: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Ty {
    Invalid,
    Basic(BasicKind),
    /// First-class type parameter (`TypeParamData` lives in the owner pool).
    TypeParam(TypeParamId),
    /// Nominal defined type (`Package`, `Local`, or `BuiltinError` origin).
    Named {
        origin: OriginId,
        arguments: SliceRef,
    },
    /// Transparent type alias (`Package` or `Local` origin).
    Alias {
        origin: OriginId,
        arguments: SliceRef,
    },
    Tuple(SliceRef),
    Union(SliceRef),
    Pointer(TypeId),
    Slice(TypeId),
    Array {
        length: u64,
        element: TypeId,
    },
    Map {
        key: TypeId,
        value: TypeId,
    },
    Chan {
        direction: Direction,
        element: TypeId,
    },
    Struct(StructId),
    Interface(InterfaceId),
    Signature(SignatureId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum Direction {
    Both,
    Send,
    Receive,
}

#[cfg(target_pointer_width = "64")]
const _: () = assert!(std::mem::size_of::<Ty>() <= 24);

pub(super) static BUILTIN_TYS: [Ty; BasicKind::COUNT] = [
    Ty::Basic(BasicKind::Bool),
    Ty::Basic(BasicKind::Int),
    Ty::Basic(BasicKind::Int8),
    Ty::Basic(BasicKind::Int16),
    Ty::Basic(BasicKind::Int32),
    Ty::Basic(BasicKind::Int64),
    Ty::Basic(BasicKind::Uint),
    Ty::Basic(BasicKind::Uint8),
    Ty::Basic(BasicKind::Uint16),
    Ty::Basic(BasicKind::Uint32),
    Ty::Basic(BasicKind::Uint64),
    Ty::Basic(BasicKind::Uintptr),
    Ty::Basic(BasicKind::Float32),
    Ty::Basic(BasicKind::Float64),
    Ty::Basic(BasicKind::Complex64),
    Ty::Basic(BasicKind::Complex128),
    Ty::Basic(BasicKind::String),
    Ty::Basic(BasicKind::Byte),
    Ty::Basic(BasicKind::Rune),
    Ty::Basic(BasicKind::UnsafePointer),
    Ty::Basic(BasicKind::UntypedBool),
    Ty::Basic(BasicKind::UntypedInt),
    Ty::Basic(BasicKind::UntypedRune),
    Ty::Basic(BasicKind::UntypedFloat),
    Ty::Basic(BasicKind::UntypedComplex),
    Ty::Basic(BasicKind::UntypedString),
    Ty::Basic(BasicKind::UntypedNil),
];
