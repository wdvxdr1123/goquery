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

const ORIGIN_LOCAL_BIT: u32 = 0x8000_0000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct OriginId(pub(super) u32);

impl OriginId {
    pub(super) const fn decl(index: u32) -> Self {
        Self(index & !ORIGIN_LOCAL_BIT)
    }

    pub(super) const fn local(index: u32) -> Self {
        Self(ORIGIN_LOCAL_BIT | (index & !ORIGIN_LOCAL_BIT))
    }

    pub(super) const fn is_local(self) -> bool {
        (self.0 & ORIGIN_LOCAL_BIT) != 0
    }

    pub(super) const fn index(self) -> usize {
        (self.0 & !ORIGIN_LOCAL_BIT) as usize
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct StructId(pub(super) u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct InterfaceId(pub(super) u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct SignatureId(pub(super) u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct TypeParamId(pub(super) u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum NamedState {
    Unloaded,
    Loading,
    Loaded,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum NamedCheckState {
    Pending,
    Valid,
    Invalid,
}

pub(super) const ORIGIN_FLAG_ALIAS: u8 = 1 << 0;
pub(super) const ORIGIN_FLAG_TPARAMS_LOADED: u8 = 1 << 1;
pub(super) const ORIGIN_FLAG_TPARAMS_LOADING: u8 = 1 << 2;
pub(super) const ORIGIN_FLAG_BOUNDS_LOADED: u8 = 1 << 3;
pub(super) const ORIGIN_FLAG_BOUNDS_LOADING: u8 = 1 << 4;
pub(super) const ORIGIN_FLAG_RHS_CONCRETE: u8 = 1 << 5;

#[derive(Clone, PartialEq, Eq, Hash)]
pub(super) enum TypeOrigin<'a> {
    Package(DeclId<'a>),
    Local {
        origin: &'a Path,
        start: u32,
        end: u32,
    },
    BuiltinError {
        interface: TypeId,
    },
}

/// Shared declaration template for a defined/alias type (`DeclArena` for package
/// declarations, `LocalArena` for function-local `type` declarations).
#[derive(Clone)]
pub(super) struct NamedOriginData<'a> {
    pub(super) origin: TypeOrigin<'a>,
    /// Type parameter `TypeId`s in the origin's pool (`DeclArena` for package origins).
    pub(super) tparams: SliceRef,
    /// Uninstantiated declaration RHS (`TypeId::INVALID` while unloaded/failed).
    pub(super) rhs: TypeId,
    pub(super) state: NamedState,
    pub(super) flags: u8,
}

impl<'a> NamedOriginData<'a> {
    pub(super) const fn new(origin: TypeOrigin<'a>, alias: bool) -> Self {
        Self {
            origin,
            tparams: SliceRef { start: 0, len: 0 },
            rhs: TypeId::INVALID,
            state: NamedState::Unloaded,
            flags: if alias { ORIGIN_FLAG_ALIAS } else { 0 },
        }
    }

    pub(super) const fn is_alias(&self) -> bool {
        (self.flags & ORIGIN_FLAG_ALIAS) != 0
    }

    pub(super) const fn tparams_loaded(&self) -> bool {
        (self.flags & ORIGIN_FLAG_TPARAMS_LOADED) != 0
    }

    pub(super) const fn tparams_loading(&self) -> bool {
        (self.flags & ORIGIN_FLAG_TPARAMS_LOADING) != 0
    }

    pub(super) const fn bounds_loaded(&self) -> bool {
        (self.flags & ORIGIN_FLAG_BOUNDS_LOADED) != 0
    }

    pub(super) const fn bounds_loading(&self) -> bool {
        (self.flags & ORIGIN_FLAG_BOUNDS_LOADING) != 0
    }

    pub(super) const fn rhs_concrete(&self) -> bool {
        (self.flags & ORIGIN_FLAG_RHS_CONCRETE) != 0
    }

    pub(super) const fn rhs_type(&self) -> Option<TypeId> {
        if self.rhs.0 == 0 {
            None
        } else {
            Some(self.rhs)
        }
    }
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
    pub(super) implicit: bool,
    pub(super) builtin_any: bool,
}

#[derive(Clone)]
pub(super) struct SignatureData {
    pub(super) type_params: SliceRef,
    pub(super) parameters: Range<usize>,
    pub(super) results: Range<usize>,
    pub(super) variadic: bool,
}

const TP_NAME_LEN_MASK: u16 = 0x3FFF;
const TP_FLAG_READY: u16 = 0x4000;
const TP_FLAG_SYNTHETIC: u16 = 0x8000;

/// Compact first-class type parameter owned by `ArenaPool::type_params`.
#[derive(Clone, Copy)]
pub(super) struct TypeParamData<'a> {
    pub(super) origin: Option<&'a Path>,
    /// Constraint interface `TypeId` (`TypeId::INVALID` while unset).
    pub(super) bound: TypeId,
    pub(super) selection: u32,
    pub(super) name_start: u32,
    pub(super) name_len_and_flags: u16,
    pub(super) index: u16,
}

impl<'a> TypeParamData<'a> {
    pub(super) fn new(
        origin: Option<&'a Path>,
        selection: u32,
        index: u16,
        name_start: u32,
        name_len: usize,
        ready: bool,
        synthetic: bool,
    ) -> Option<Self> {
        let len = u16::try_from(name_len).ok()?;
        if len > TP_NAME_LEN_MASK {
            return None;
        }
        let mut packed = len;
        if ready {
            packed |= TP_FLAG_READY;
        }
        if synthetic {
            packed |= TP_FLAG_SYNTHETIC;
        }
        Some(Self {
            origin,
            bound: TypeId::INVALID,
            selection,
            name_start,
            name_len_and_flags: packed,
            index,
        })
    }

    pub(super) const fn name_len(self) -> usize {
        (self.name_len_and_flags & TP_NAME_LEN_MASK) as usize
    }

    pub(super) const fn name_range(self) -> Range<usize> {
        let start = self.name_start as usize;
        start..start + self.name_len()
    }

    pub(super) const fn is_ready(self) -> bool {
        (self.name_len_and_flags & TP_FLAG_READY) != 0
    }

    pub(super) fn set_ready(&mut self, ready: bool) {
        if ready {
            self.name_len_and_flags |= TP_FLAG_READY;
        } else {
            self.name_len_and_flags &= !TP_FLAG_READY;
        }
    }

    pub(super) const fn is_synthetic(self) -> bool {
        (self.name_len_and_flags & TP_FLAG_SYNTHETIC) != 0
    }

    pub(super) const fn bound(self) -> Option<TypeId> {
        if self.bound.0 == 0 {
            None
        } else {
            Some(self.bound)
        }
    }

    pub(super) fn set_bound(&mut self, bound: TypeId) {
        self.bound = bound;
    }
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
const _: () = {
    assert!(std::mem::size_of::<Ty>() <= 24);
    assert!(std::mem::size_of::<TypeParamData<'_>>() <= 32);
    assert!(std::mem::size_of::<NamedOriginData<'_>>() <= 48);
};

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
