use super::BasicKind;
use crate::analysis::obj::DeclId;
use std::ops::Range;
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct TypeId(pub(super) u16);

impl TypeId {
    /// Permanent first node in every arena, also used by non-value operands.
    pub(crate) const INVALID: Self = Self(0);
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

#[derive(Clone)]
pub(super) enum Ty<'a> {
    Invalid,
    Basic(BasicKind),
    Error(TypeId),
    /// A formal's identity is its TypeId; the index is only for schema substitution.
    Parameter(usize),
    /// Declaration-only local identity: no underlying/checking facts are invented.
    LocalDeclaration {
        origin: &'a Path,
        selection: Range<usize>,
        formal: bool,
    },
    Interface {
        origin: &'a Path,
        methods: Range<usize>,
        builtin_any: bool,
    },
    Signature {
        parameters: Range<usize>,
        results: Range<usize>,
        variadic: bool,
    },
    Struct {
        origin: &'a Path,
        fields: Range<usize>,
    },
    Named {
        declaration: DeclId<'a>,
        arguments: SliceRef,
    },
    Alias {
        declaration: DeclId<'a>,
        arguments: SliceRef,
    },
    Tuple(SliceRef),
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum Direction {
    Both,
    Send,
    Receive,
}

// Keep the 64-bit per-node ceiling unchanged when adding projection metadata.
#[cfg(target_pointer_width = "64")]
const _: () = assert!(std::mem::size_of::<Ty<'static>>() <= 48);
