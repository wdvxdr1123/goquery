//! Sealed semantic engine: internal modules (`check`, `types`, `scope`, `obj`)
//! remain private to this submodule so `analysis::query` cannot import
//! `RegionInfo`, `DeclTable`, `TypeArena`, `TypeStore`, `TypeId`, or `ScopeTree`.
#[path = "budget.rs"]
pub(super) mod budget;
#[path = "check/mod.rs"]
pub(in crate::analysis::engine) mod check;
#[path = "obj.rs"]
pub(in crate::analysis::engine) mod obj;
#[path = "resolve/mod.rs"]
pub(super) mod resolve;
#[path = "scope/mod.rs"]
pub(in crate::analysis::engine) mod scope;
#[path = "sema/mod.rs"]
pub(super) mod sema;
#[cfg(test)]
#[path = "type_oracle.rs"]
pub(in crate::analysis::engine) mod type_oracle;
#[path = "types/mod.rs"]
pub(in crate::analysis::engine) mod types;
