//! Shared analysis ceilings; no request may grow a package type cache.
pub(crate) const MAX_NODES: usize = 256;
pub(crate) const MAX_DEPTH: usize = 32;
pub(crate) const MAX_BYTES: usize = 16 * 1024;
pub(crate) const MAX_BODY_BYTES: usize = 128 * 1024;
pub(crate) const RELATION_STEPS: usize = 4096;
pub(crate) const MAX_SHARED_ARENA_NODES: usize = 4096;
pub(crate) const MAX_DECL_CACHE: usize = 1024;
pub(crate) const MAX_DECL_PROMOTIONS: usize = 64;

// Reference result allocation remains bounded across local and global paths.
pub(crate) const MAX_REFERENCES: usize = 4096;

pub(crate) const MAX_CALL_SITES: usize = 4096;
