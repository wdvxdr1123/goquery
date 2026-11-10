//! Shared native-query ceilings; no request may grow a package type cache.
pub(crate) const MAX_NODES: usize = 256;
pub(crate) const MAX_DEPTH: usize = 32;
pub(crate) const MAX_BYTES: usize = 16 * 1024;
pub(crate) const RELATION_STEPS: usize = 4096;
pub(crate) const MEMO_ENTRIES: usize = 256;
pub(crate) const MEMO_BYTES: usize = 16 * 1024;

// Reference result allocation remains bounded across local and global paths.
pub(crate) const MAX_REFERENCES: usize = 4096;

pub(crate) const MAX_CALL_SITES: usize = 4096;
