//! Semantic outer-type flags, independent of protocol legend indices.
pub(crate) const SIGNATURE: u64 = 1 << 0;
pub(crate) const NUMBER: u64 = 1 << 1;
pub(crate) const BOOL: u64 = 1 << 2;
pub(crate) const STRING: u64 = 1 << 3;
pub(crate) const INTERFACE: u64 = 1 << 4;
pub(crate) const STRUCT: u64 = 1 << 5;
pub(crate) const POINTER: u64 = 1 << 6;
pub(crate) const ARRAY: u64 = 1 << 7;
pub(crate) const SLICE: u64 = 1 << 8;
pub(crate) const MAP: u64 = 1 << 9;
pub(crate) const CHAN: u64 = 1 << 10;
