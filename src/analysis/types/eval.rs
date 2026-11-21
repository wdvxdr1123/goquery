//! Explicit stack-allocated type evaluation context (`TypeEval`) and RAII
//! mode / recursion-depth guards over [`TypeStore`].

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::engine::budget::{MAX_BYTES, MAX_LOCAL_ARENA_NODES, MAX_NODES};
    use crate::analysis::engine::types::{BasicKind, Qualifier, TypeId};
    use crate::workspace::Workspace;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::path::Path;

    fn test_store(workspace: &Workspace) -> TypeStore<'_> {
        TypeStore::with_shared_budget(workspace, workspace.build_view(None))
    }

    #[test]
    fn projection_guard_rewinds_on_normal_and_panic() {
        let workspace = Workspace::default();
        let mut store = test_store(&workspace);
        let int_ty = TypeId::basic(BasicKind::Int);
        let baseline_ptr = store.pointer(int_ty).expect("baseline local pointer");
        let baseline_nodes = store.local.pool.nodes.len();
        assert_eq!(store.mode, CheckMode::Concrete);
        assert_eq!(store.alloc_tier, AllocTier::Local);

        // Normal exit rewinds speculative allocations and restores state.
        let inner_nodes = store.with_projection(true, |store| {
            assert_eq!(store.mode, CheckMode::Symbolic);
            assert_eq!(store.alloc_tier, AllocTier::Local);
            let slice_ty = store.slice_type(baseline_ptr).expect("speculative slice");
            let _ = store.pointer(slice_ty).expect("speculative pointer");
            assert!(store.local.pool.nodes.len() > baseline_nodes);
            store.local.pool.nodes.len()
        });
        assert!(inner_nodes > baseline_nodes);
        assert_eq!(store.local.pool.nodes.len(), baseline_nodes);
        assert_eq!(store.mode, CheckMode::Concrete);
        assert_eq!(store.alloc_tier, AllocTier::Local);
        assert_eq!(store.local.pool.node_limit, MAX_LOCAL_ARENA_NODES);
        assert_eq!(store.local.pool.byte_limit, MAX_BYTES);

        // Panic unwind still rewinds watermark and restores all saved fields.
        store.alloc_tier = AllocTier::Decl;
        let panicked = catch_unwind(AssertUnwindSafe(|| {
            let mut guard = ProjectionGuard::new(&mut store, CheckMode::Symbolic);
            assert_eq!(guard.mode, CheckMode::Symbolic);
            assert_eq!(guard.alloc_tier, AllocTier::Local);
            let _ = guard.slice_type(int_ty).expect("panic speculative slice");
            assert!(guard.local.pool.nodes.len() > baseline_nodes);
            panic!("simulated projection panic");
        }));
        assert!(panicked.is_err());
        assert_eq!(store.local.pool.nodes.len(), baseline_nodes);
        assert_eq!(store.mode, CheckMode::Concrete);
        assert_eq!(store.alloc_tier, AllocTier::Decl);
        assert_eq!(store.local.pool.node_limit, MAX_LOCAL_ARENA_NODES);
        assert_eq!(store.local.pool.byte_limit, MAX_BYTES);
    }

    #[test]
    fn alloc_tier_and_mode_guards_restore_on_nested_and_panic() {
        let workspace = Workspace::default();
        let mut store = test_store(&workspace);
        assert_eq!(store.alloc_tier, AllocTier::Local);
        assert_eq!(store.mode, CheckMode::Concrete);

        store.with_decl_alloc(|store| {
            assert_eq!(store.alloc_tier, AllocTier::Decl);
            store.with_local_alloc(|store| {
                assert_eq!(store.alloc_tier, AllocTier::Local);
            });
            assert_eq!(store.alloc_tier, AllocTier::Decl);
        });
        assert_eq!(store.alloc_tier, AllocTier::Local);

        let tier_panicked = catch_unwind(AssertUnwindSafe(|| {
            let guard = AllocTierGuard::new(&mut store, AllocTier::Decl);
            assert_eq!(guard.alloc_tier, AllocTier::Decl);
            panic!("simulated alloc panic");
        }));
        assert!(tier_panicked.is_err());
        assert_eq!(store.alloc_tier, AllocTier::Local);

        let mode_panicked = catch_unwind(AssertUnwindSafe(|| {
            let guard = ModeGuard::new(&mut store, CheckMode::Symbolic);
            assert_eq!(guard.mode, CheckMode::Symbolic);
            panic!("simulated mode panic");
        }));
        assert!(mode_panicked.is_err());
        assert_eq!(store.mode, CheckMode::Concrete);
    }

    #[test]
    fn depth_guard_enforces_limit_and_unwinds_cleanly() {
        let workspace = Workspace::default();
        let mut store = test_store(&workspace);

        for kind in [
            DepthKind::NamedCheck,
            DepthKind::Expansion,
            DepthKind::Constant,
            DepthKind::Constraint,
        ] {
            fn recurse(store: &mut TypeStore<'_>, kind: DepthKind, seen: &mut usize) {
                let Some(mut guard) = store.enter_depth(kind) else {
                    return;
                };
                *seen = (*seen).max(guard.depth());
                recurse(&mut guard, kind, seen);
            }

            let mut max_seen = 0;
            recurse(&mut store, kind, &mut max_seen);
            assert_eq!(max_seen, MAX_DEPTH);
            assert_eq!(depth_counter(&store, kind), 0);

            let panicked = catch_unwind(AssertUnwindSafe(|| {
                let guard = store.enter_depth(kind).expect("depth entry");
                assert_eq!(guard.depth(), 1);
                panic!("simulated depth panic");
            }));
            assert!(panicked.is_err());
            assert_eq!(depth_counter(&store, kind), 0);
        }
    }

    #[test]
    fn type_eval_relations_lookup_and_writer_honor_explicit_mode() {
        let workspace = Workspace::default();
        let mut store = test_store(&workspace);
        assert_eq!(store.mode, CheckMode::Concrete);

        // TypeEval::symbolic overrides ambient Concrete mode and restores on drop.
        {
            let mut eval = TypeEval::symbolic(&mut store);
            assert_eq!(eval.mode(), CheckMode::Symbolic);
            assert_eq!(eval.store().mode, CheckMode::Symbolic);
            eval.step().expect("step within budget");
            eval.reset_steps();
            eval.with_projection(CheckMode::Concrete, |inner| {
                assert_eq!(inner.mode(), CheckMode::Concrete);
                assert_eq!(inner.store().mode, CheckMode::Concrete);
            });
            assert_eq!(eval.mode(), CheckMode::Symbolic);
            assert_eq!(eval.store().mode, CheckMode::Symbolic);
        }
        assert_eq!(store.mode, CheckMode::Concrete);

        // TypeRelations and MethodLookup genuinely scope store.mode to their explicit mode.
        {
            let rel = TypeRelations::new(&mut store, CheckMode::Symbolic);
            assert_eq!(rel.mode(), CheckMode::Symbolic);
            assert_eq!(rel.store().mode, CheckMode::Symbolic);
        }
        assert_eq!(store.mode, CheckMode::Concrete);

        {
            let lookup = MethodLookup::new(&mut store, CheckMode::Symbolic);
            assert_eq!(lookup.mode(), CheckMode::Symbolic);
            assert_eq!(lookup.store().mode, CheckMode::Symbolic);
        }
        assert_eq!(store.mode, CheckMode::Concrete);

        // TypeWriter stays purely read-only (&TypeStore) and obeys its explicit mode
        // even when store.mode differs.
        let mut workspace = Workspace::default();
        let origin = std::path::PathBuf::from("a.go");
        let other = Path::new("b.go");
        workspace
            .files
            .insert(origin.clone(), crate::index::index_source("package p\n"));
        let mut store = test_store(&workspace);
        let param = store
            .push_symbolic_parameter(&origin, "T")
            .expect("local type parameter");
        assert_eq!(store.mode, CheckMode::Concrete);
        let concrete_writer = TypeWriter::new(&store, CheckMode::Concrete);
        let symbolic_writer = TypeWriter::new(&store, CheckMode::Symbolic);
        assert_eq!(
            concrete_writer.format(param, Qualifier::RelativeTo(other)),
            None
        );
        assert_eq!(
            symbolic_writer.format(param, Qualifier::RelativeTo(other)),
            Some("T".to_owned())
        );

        // TypeEval::writer inherits TypeEval's explicit mode.
        let eval_sym = TypeEval::symbolic(&mut store);
        assert_eq!(
            eval_sym
                .writer()
                .format(param, Qualifier::RelativeTo(other)),
            Some("T".to_owned())
        );
        drop(eval_sym);
        assert_eq!(store.mode, CheckMode::Concrete);
    }

    #[test]
    fn local_headroom_tracks_half_budget_and_projection_rewind() {
        let workspace = Workspace::default();
        let mut store = test_store(&workspace);
        assert!(store.local_headroom());

        let mut ty = TypeId::basic(BasicKind::Int);
        let half = MAX_LOCAL_ARENA_NODES / 2;
        assert!(half >= MAX_NODES);
        for _ in 0..half {
            ty = store.pointer(ty).expect("within local node budget");
        }
        assert!(store.local_headroom());

        // Speculative projection pushing past half capacity rewinds cleanly back to headroom.
        store.with_projection(true, |store| {
            let _ = store.pointer(ty).expect("speculative node past half");
            assert!(!store.local_headroom());
        });
        assert!(store.local_headroom());

        // One persistent local node crosses half capacity until reset_local().
        let _ = store.pointer(ty).expect("past half local node budget");
        assert!(!store.local_headroom());

        store.reset_local();
        assert!(store.local_headroom());
    }

    #[test]
    fn relations_and_lookup_explicit_mode_overrides_ambient_store_mode() {
        use crate::analysis::engine::obj::DeclId;
        use crate::workspace::WorkspaceEvent;

        let mut workspace = Workspace::default();
        let file = std::path::PathBuf::from("generic.go");
        workspace
            .apply(WorkspaceEvent::SetBuffer {
                file: file.clone(),
                source: "package p\ntype Box[T comparable] struct { V T }\ntype Alias[T comparable] = T\n"
                    .to_owned(),
                dirty: false,
            })
            .expect("open buffer");
        let (interned_file, stub) = workspace.files.get_key_value(&file).expect("indexed file");
        let box_sym = stub
            .symbols
            .iter()
            .find(|s| s.name == "Box")
            .expect("Box symbol");
        let alias_sym = stub
            .symbols
            .iter()
            .find(|s| s.name == "Alias")
            .expect("Alias symbol");
        let box_decl = DeclId {
            file: interned_file.as_path(),
            symbol: box_sym,
        };
        let alias_decl = DeclId {
            file: interned_file.as_path(),
            symbol: alias_sym,
        };

        let mut store = test_store(&workspace);
        assert_eq!(store.mode, CheckMode::Concrete);

        // Box[[]int] violates `comparable` in Concrete mode, succeeds in Symbolic mode.
        let slice_int = store
            .slice_type(TypeId::basic(BasicKind::Int))
            .expect("slice type");
        let box_slice = store
            .push_named(box_decl, &[slice_int], false)
            .expect("Box[[]int]");
        assert!(
            MethodLookup::new(&mut store, CheckMode::Concrete)
                .lookup_field_or_method(box_slice, false, interned_file, "V")
                .is_none()
        );
        assert_eq!(store.mode, CheckMode::Concrete);
        let found = MethodLookup::new(&mut store, CheckMode::Symbolic).lookup_field_or_method(
            box_slice,
            false,
            interned_file,
            "V",
        );
        assert!(matches!(found, Some(Some(_))));
        assert_eq!(store.mode, CheckMode::Concrete);

        // Alias[[]int] violates `comparable` in Concrete mode, expands to []int in Symbolic mode.
        store.reset_local();
        let slice_int = store
            .slice_type(TypeId::basic(BasicKind::Int))
            .expect("slice type");
        let alias_slice = store
            .push_named(alias_decl, &[slice_int], true)
            .expect("Alias[[]int]");
        assert_eq!(
            TypeRelations::new(&mut store, CheckMode::Concrete).identical(alias_slice, slice_int),
            None
        );
        assert_eq!(store.mode, CheckMode::Concrete);
        assert_eq!(
            TypeRelations::new(&mut store, CheckMode::Symbolic).identical(alias_slice, slice_int),
            Some(true)
        );
        assert_eq!(store.mode, CheckMode::Concrete);
    }
}
