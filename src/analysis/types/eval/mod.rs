//! Explicit stack-allocated type evaluation context (`TypeEval`) and RAII
//! mode / recursion-depth guards over [`TypeStore`].

#[cfg(test)]
mod tests;

use super::MAX_DEPTH;
use super::format::TypeWriter;
use super::lookup::MethodLookup;
use super::predicates::TypeRelations;
use super::store::{AllocTierGuard, ProjectionGuard, TypeStore};
use super::ty::{AllocTier, CheckMode};
use std::ops::{Deref, DerefMut};

/// Identifies one of the bounded recursion depth counters tracked on [`TypeStore`].
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DepthKind {
    NamedCheck,
    Expansion,
    Constant,
    Constraint,
}

/// RAII guard scoping [`TypeStore::mode`] to an explicit [`CheckMode`] and
/// unconditionally restoring the previous mode on drop (including early return
/// or panic).
pub(crate) struct ModeGuard<'s, 'a> {
    store: &'s mut TypeStore<'a>,
    saved_mode: CheckMode,
}

impl<'s, 'a> ModeGuard<'s, 'a> {
    pub(crate) fn new(store: &'s mut TypeStore<'a>, mode: CheckMode) -> Self {
        let saved_mode = store.mode;
        store.mode = mode;
        Self { store, saved_mode }
    }
}

impl<'a> Deref for ModeGuard<'_, 'a> {
    type Target = TypeStore<'a>;

    fn deref(&self) -> &Self::Target {
        self.store
    }
}

impl DerefMut for ModeGuard<'_, '_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.store
    }
}

impl Drop for ModeGuard<'_, '_> {
    fn drop(&mut self) {
        self.store.mode = self.saved_mode;
    }
}

/// RAII recursion-depth guard that checks and increments a [`TypeStore`] depth
/// counter on entry (returning `None` at [`MAX_DEPTH`]) and unconditionally
/// decrements it on drop (including early return or panic).
#[allow(dead_code)]
pub(crate) struct DepthGuard<'s, 'a> {
    store: &'s mut TypeStore<'a>,
    kind: DepthKind,
}

#[allow(dead_code)]
impl<'s, 'a> DepthGuard<'s, 'a> {
    pub(crate) fn enter(store: &'s mut TypeStore<'a>, kind: DepthKind) -> Option<Self> {
        let counter = depth_counter_mut(store, kind);
        if *counter >= MAX_DEPTH {
            return None;
        }
        *counter += 1;
        Some(Self { store, kind })
    }

    pub(crate) fn depth(&self) -> usize {
        depth_counter(self.store, self.kind)
    }
}

impl<'a> Deref for DepthGuard<'_, 'a> {
    type Target = TypeStore<'a>;

    fn deref(&self) -> &Self::Target {
        self.store
    }
}

impl DerefMut for DepthGuard<'_, '_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.store
    }
}

impl Drop for DepthGuard<'_, '_> {
    fn drop(&mut self) {
        let counter = depth_counter_mut(self.store, self.kind);
        debug_assert!(*counter > 0);
        *counter = counter.saturating_sub(1);
    }
}

#[allow(dead_code)]
fn depth_counter(store: &TypeStore<'_>, kind: DepthKind) -> usize {
    match kind {
        DepthKind::NamedCheck => store.named_check_depth,
        DepthKind::Expansion => store.expansion_depth,
        DepthKind::Constant => store.constant_depth,
        DepthKind::Constraint => store.constraint_depth,
    }
}

#[allow(dead_code)]
fn depth_counter_mut<'s, 'a>(store: &'s mut TypeStore<'a>, kind: DepthKind) -> &'s mut usize {
    match kind {
        DepthKind::NamedCheck => &mut store.named_check_depth,
        DepthKind::Expansion => &mut store.expansion_depth,
        DepthKind::Constant => &mut store.constant_depth,
        DepthKind::Constraint => &mut store.constraint_depth,
    }
}

impl<'a> TypeStore<'a> {
    #[allow(dead_code)]
    pub(crate) fn enter_depth(&mut self, kind: DepthKind) -> Option<DepthGuard<'_, 'a>> {
        DepthGuard::enter(self, kind)
    }
}

/// Explicit evaluation context pairing mutable access to [`TypeStore`] with a
/// scoped [`CheckMode`].
///
/// Constructing a [`TypeEval`] scopes `store.mode` through [`ModeGuard`] so
/// both direct methods and legacy `TypeStore` helpers honor the explicit mode
/// and restore ambient state unconditionally on drop.
pub(crate) struct TypeEval<'s, 'a> {
    guard: ModeGuard<'s, 'a>,
    mode: CheckMode,
}

impl<'s, 'a> TypeEval<'s, 'a> {
    pub(crate) fn new(store: &'s mut TypeStore<'a>, mode: CheckMode) -> Self {
        Self {
            guard: ModeGuard::new(store, mode),
            mode,
        }
    }

    #[allow(dead_code)]
    pub(crate) fn concrete(store: &'s mut TypeStore<'a>) -> Self {
        Self::new(store, CheckMode::Concrete)
    }

    #[allow(dead_code)]
    pub(crate) fn symbolic(store: &'s mut TypeStore<'a>) -> Self {
        Self::new(store, CheckMode::Symbolic)
    }

    pub(crate) fn from_store(store: &'s mut TypeStore<'a>) -> Self {
        let mode = store.mode;
        Self::new(store, mode)
    }

    pub(crate) const fn mode(&self) -> CheckMode {
        self.mode
    }

    pub(crate) fn store(&self) -> &TypeStore<'a> {
        &self.guard
    }

    pub(crate) fn store_mut(&mut self) -> &mut TypeStore<'a> {
        &mut self.guard
    }

    pub(crate) fn step(&mut self) -> Option<()> {
        self.guard.step()
    }

    #[allow(dead_code)]
    pub(crate) fn reset_steps(&mut self) {
        self.guard.reset_steps();
    }

    /// Runs `project` inside a speculative local projection with an explicit
    /// [`CheckMode`], unconditionally rewinding local allocations and restoring
    /// store state on exit or panic.
    pub(crate) fn with_projection<R>(
        &mut self,
        mode: CheckMode,
        project: impl FnOnce(&mut TypeEval<'_, 'a>) -> R,
    ) -> R {
        let mut guard = ProjectionGuard::new(&mut self.guard, mode);
        let mut eval = TypeEval::new(&mut guard, mode);
        project(&mut eval)
    }

    #[allow(dead_code)]
    pub(crate) fn with_decl_alloc<R>(&mut self, f: impl FnOnce(&mut TypeEval<'_, 'a>) -> R) -> R {
        let mode = self.mode;
        let mut guard = AllocTierGuard::new(&mut self.guard, AllocTier::Decl);
        let mut eval = TypeEval::new(&mut guard, mode);
        f(&mut eval)
    }

    #[allow(dead_code)]
    pub(crate) fn with_local_alloc<R>(&mut self, f: impl FnOnce(&mut TypeEval<'_, 'a>) -> R) -> R {
        let mode = self.mode;
        let mut guard = AllocTierGuard::new(&mut self.guard, AllocTier::Local);
        let mut eval = TypeEval::new(&mut guard, mode);
        f(&mut eval)
    }

    #[allow(dead_code)]
    pub(crate) fn enter_depth(&mut self, kind: DepthKind) -> Option<DepthGuard<'_, 'a>> {
        DepthGuard::enter(&mut self.guard, kind)
    }

    pub(crate) fn relations(&mut self) -> TypeRelations<'_, 'a> {
        TypeRelations::new(&mut self.guard, self.mode)
    }

    pub(crate) fn method_lookup(&mut self) -> MethodLookup<'_, 'a> {
        MethodLookup::new(&mut self.guard, self.mode)
    }

    #[allow(dead_code)]
    pub(crate) fn writer(&self) -> TypeWriter<'_, 'a> {
        TypeWriter::new(&self.guard, self.mode)
    }
}
