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
            source:
                "package p\ntype Box[T comparable] struct { V T }\ntype Alias[T comparable] = T\n"
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

#[test]
fn named_origin_loads_once_and_instantiates_without_rereading_disk() {
    use crate::analysis::engine::obj::DeclId;
    use crate::analysis::engine::types::ty::NamedState;
    use std::time::{SystemTime, UNIX_EPOCH};

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("goquery_named_origin_{stamp}"));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let file = dir.join("box.go");
    let source = "package p\ntype Box[T comparable] struct { V T }\n";
    std::fs::write(&file, source).expect("write temp go file");

    let mut workspace = Workspace::default();
    workspace
        .files
        .insert(file.clone(), crate::index::index_source(source));
    let (interned_file, stub) = workspace.files.get_key_value(&file).expect("indexed file");
    let box_sym = stub
        .symbols
        .iter()
        .find(|s| s.name == "Box")
        .expect("Box symbol");
    let box_decl = DeclId {
        file: interned_file.as_path(),
        symbol: box_sym,
    };

    let mut store = test_store(&workspace);
    let int_ty = TypeId::basic(BasicKind::Int);
    let str_ty = TypeId::basic(BasicKind::String);
    let bool_ty = TypeId::basic(BasicKind::Bool);

    let box_int = store
        .with_decl_alloc(|s| s.push_named(box_decl, &[int_ty], false))
        .expect("Box[int]");
    let box_str = store
        .with_decl_alloc(|s| s.push_named(box_decl, &[str_ty], false))
        .expect("Box[string]");
    let box_bool = store
        .with_decl_alloc(|s| s.push_named(box_decl, &[bool_ty], false))
        .expect("Box[bool]");

    // All three instantiations share ONE NamedOriginData in DeclArena.
    assert_eq!(store.decl.pool.origins.len(), 1);
    assert_eq!(store.decl.pool.origins[0].state, NamedState::Unloaded);

    // First expansion loads Box's origin template (tparams + bounds + rhs) from disk.
    let sel_int = MethodLookup::new(&mut store, CheckMode::Concrete)
        .lookup_field_or_method(box_int, false, interned_file, "V")
        .expect("lookup outcome")
        .expect("field V on Box[int]");
    assert_eq!(sel_int.ty, int_ty);
    assert_eq!(store.decl.pool.origins.len(), 1);
    assert_eq!(store.decl.pool.origins[0].state, NamedState::Loaded);
    assert!(store.decl.pool.origins[0].tparams_loaded());
    assert!(store.decl.pool.origins[0].bounds_loaded());

    // Delete the closed file from disk to prove zero additional disk reads or re-parsing.
    std::fs::remove_file(&file).expect("remove temp go file");
    let _ = std::fs::remove_dir(&dir);

    let sel_str = MethodLookup::new(&mut store, CheckMode::Concrete)
        .lookup_field_or_method(box_str, false, interned_file, "V")
        .expect("lookup outcome without disk")
        .expect("field V on Box[string]");
    assert_eq!(sel_str.ty, str_ty);

    let sel_bool = MethodLookup::new(&mut store, CheckMode::Concrete)
        .lookup_field_or_method(box_bool, false, interned_file, "V")
        .expect("lookup outcome without disk")
        .expect("field V on Box[bool]");
    assert_eq!(sel_bool.ty, bool_ty);

    // Invalid argument still fails purely from the cached DeclArena bound (zero disk I/O).
    let slice_int = store.slice_type(int_ty).expect("[]int");
    let box_slice = store
        .push_named(box_decl, &[slice_int], false)
        .expect("Box[[]int]");
    assert!(box_slice.is_local());
    assert!(store.check_named_arguments(box_slice).is_none());
    assert_eq!(store.decl.pool.origins.len(), 1);
}

#[test]
fn local_defined_types_live_in_local_arena_origins_without_string_copies() {
    use crate::workspace::WorkspaceEvent;

    let mut workspace = Workspace::default();
    let file = std::path::PathBuf::from("local.go");
    let source =
        "package p\nfunc f() {\n\ttype Node struct { next *Node }\n\ttype Alias = Node\n}\n";
    workspace
        .apply(WorkspaceEvent::SetBuffer {
            file: file.clone(),
            source: source.to_owned(),
            dirty: false,
        })
        .expect("open buffer");
    let mut store = test_store(&workspace);

    let node_start = source.find("Node").expect("Node");
    let node_rhs_start = source.find("struct").expect("struct");
    let node_rhs = "struct { next *Node }";
    let node_ty = store
        .register_local_type(
            &file,
            "Node",
            node_start..node_start + 4,
            node_rhs,
            node_rhs_start..node_rhs_start + node_rhs.len(),
            false,
        )
        .expect("register local Node");
    let node_struct = store
        .parse_in_at(&file, node_rhs, node_rhs_start, &[("Node", node_ty)])
        .expect("parse Node rhs");
    store.set_local_type_rhs(node_ty, Some(node_struct));

    let alias_start = source.find("Alias").expect("Alias");
    let alias_rhs_start = source.rfind("Node").expect("Alias rhs");
    let alias_ty = store
        .register_local_type(
            &file,
            "Alias",
            alias_start..alias_start + 5,
            "Node",
            alias_rhs_start..alias_rhs_start + 4,
            true,
        )
        .expect("register local Alias");
    store.set_local_type_rhs(alias_ty, Some(node_ty));

    // Open buffers incur ZERO bytes copied into LocalArena::local_text.
    assert!(store.local.local_text.is_empty());
    assert_eq!(store.unalias(alias_ty), Some(node_ty));
    assert_eq!(store.underlying(alias_ty), Some(node_struct));
    assert_eq!(
        store.local_type_annotation(node_ty).map(|(_, ann)| ann),
        Some(node_rhs)
    );
    assert_eq!(
        TypeWriter::new(&store, CheckMode::Concrete).format(node_ty, Qualifier::RelativeTo(&file)),
        Some("Node".to_owned())
    );

    // Speculative local types rewind cleanly without leaking origins or metadata.
    store.with_projection(false, |s| {
        let tmp = s
            .register_local_type(&file, "Tmp", 0..3, "int", 0..3, false)
            .expect("speculative local type");
        s.set_local_type_rhs(tmp, Some(TypeId::basic(BasicKind::Int)));
        assert_eq!(s.local.local_origins.len(), 3);
    });
    assert_eq!(store.local.local_origins.len(), 2);
    assert_eq!(store.local.local_type_meta.len(), 2);
}
