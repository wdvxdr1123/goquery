//! Two-tier type storage (`DeclArena` + `LocalArena`) with epoch-checked [`TypeId`]s.
mod alloc;

use super::lookup::ConcreteMethodMemo;
use super::pool::ArenaPool;
use super::ty::{
    AllocTier, BUILTIN_TYS, CheckMode, InterfaceData, NamedOriginData, OriginId, SignatureData,
    SliceRef, StepBudget, StructData, Term, TypeOrigin, TypeParamData, TypeTier,
};
use super::{Field, Method, Ty, TypeId};
use crate::analysis::engine::budget::{
    MAX_BYTES, MAX_DECL_ARENA_NODES, MAX_LOCAL_ARENA_NODES, MAX_NODES, RELATION_STEPS,
};
use crate::analysis::engine::obj::DeclId;
use crate::analysis::engine::resolve::FileEnvCache;
use crate::workspace::{BuildView, Workspace};
use std::collections::HashMap;
use std::ops::{Deref, DerefMut, Range};
use std::path::Path;

/// Request-scoped storage for package-level declaration types (never rewound).
pub(in crate::analysis::engine::types) struct DeclArena<'a> {
    pub(in crate::analysis::engine::types) pool: ArenaPool<'a>,
    pub(in crate::analysis::engine::types) concrete_methods:
        HashMap<DeclId<'a>, ConcreteMethodMemo>,
    pub(in crate::analysis::engine::types) package_origins: HashMap<DeclId<'a>, OriginId>,
}

impl<'a> DeclArena<'a> {
    fn new() -> Self {
        Self {
            pool: ArenaPool::new(false, 0, MAX_DECL_ARENA_NODES, MAX_BYTES * 3),
            concrete_methods: HashMap::new(),
            package_origins: HashMap::new(),
        }
    }
}

/// Compact side metadata for a local `type` declaration in [`LocalArena`].
#[derive(Clone, Copy)]
pub(in crate::analysis::engine::types) struct LocalTypeMeta {
    pub(in crate::analysis::engine::types) name_buf: SliceRef,
    pub(in crate::analysis::engine::types) ann_start: u32,
    pub(in crate::analysis::engine::types) ann_end: u32,
    pub(in crate::analysis::engine::types) ann_in_buf: bool,
}

#[inline]
fn to_u32(len: usize) -> u32 {
    debug_assert!(len <= u32::MAX as usize);
    len as u32
}

#[derive(Clone, Copy)]
pub(crate) struct LocalWatermark {
    nodes: u32,
    arguments: u32,
    origins: u32,
    structs: u32,
    interfaces: u32,
    signatures: u32,
    type_params: u32,
    param_names: u32,
    fields: u32,
    field_bytes: u32,
    methods: u32,
    method_bytes: u32,
    terms: u32,
    signature_names: u32,
    signature_name_bytes: u32,
    local_text: u32,
}

const _: () = assert!(std::mem::size_of::<TypeId>() == 4);
const _: () = assert!(std::mem::size_of::<Ty>() <= 24);
const _: () = assert!(std::mem::size_of::<LocalWatermark>() <= 72);

/// Region-scoped storage for local types, parameters, and speculative probes.
pub(in crate::analysis::engine::types) struct LocalArena<'a> {
    pub(in crate::analysis::engine::types) epoch: u16,
    pub(in crate::analysis::engine::types) pool: ArenaPool<'a>,
    pub(in crate::analysis::engine::types) local_origins: HashMap<(&'a Path, u32, u32), TypeId>,
    pub(in crate::analysis::engine::types) local_type_meta: HashMap<OriginId, LocalTypeMeta>,
    pub(in crate::analysis::engine::types) local_text: String,
}

impl<'a> LocalArena<'a> {
    fn new() -> Self {
        Self {
            epoch: 1,
            pool: ArenaPool::new(true, 1, MAX_LOCAL_ARENA_NODES, MAX_BYTES),
            local_origins: HashMap::new(),
            local_type_meta: HashMap::new(),
            local_text: String::new(),
        }
    }

    fn reset(&mut self) {
        self.epoch = (self.epoch % 0x7FFF) + 1;
        self.pool.epoch = self.epoch;
        self.pool.nodes.clear();
        self.pool.arguments.clear();
        self.pool.origins.clear();
        self.pool.structs.clear();
        self.pool.interfaces.clear();
        self.pool.signatures.clear();
        self.pool.type_params.clear();
        self.pool.param_names.clear();
        self.pool.formal_params.clear();
        self.pool.fields.clear();
        self.pool.field_bytes = 0;
        self.pool.methods.clear();
        self.pool.method_bytes = 0;
        self.pool.terms.clear();
        self.pool.interface_sets.clear();
        self.pool.signature_names.clear();
        self.pool.signature_name_bytes.clear();
        self.pool.interner.clear();
        self.pool.expansions.clear();
        self.pool.named_checks.clear();
        self.local_origins.clear();
        self.local_type_meta.clear();
        self.local_text.clear();
        self.pool.node_limit = MAX_LOCAL_ARENA_NODES;
        self.pool.byte_limit = MAX_BYTES;
    }

    fn mark(&self) -> LocalWatermark {
        LocalWatermark {
            nodes: to_u32(self.pool.nodes.len()),
            arguments: to_u32(self.pool.arguments.len()),
            origins: to_u32(self.pool.origins.len()),
            structs: to_u32(self.pool.structs.len()),
            interfaces: to_u32(self.pool.interfaces.len()),
            signatures: to_u32(self.pool.signatures.len()),
            type_params: to_u32(self.pool.type_params.len()),
            param_names: to_u32(self.pool.param_names.len()),
            fields: to_u32(self.pool.fields.len()),
            field_bytes: to_u32(self.pool.field_bytes),
            methods: to_u32(self.pool.methods.len()),
            method_bytes: to_u32(self.pool.method_bytes),
            terms: to_u32(self.pool.terms.len()),
            signature_names: to_u32(self.pool.signature_names.len()),
            signature_name_bytes: to_u32(self.pool.signature_name_bytes.len()),
            local_text: to_u32(self.local_text.len()),
        }
    }

    fn rewind(&mut self, mark: LocalWatermark) {
        let nodes = mark.nodes as usize;
        let origins = mark.origins as usize;
        let removed_nodes = self.pool.nodes.len() > nodes;
        let removed_origins = self.pool.origins.len() > origins;
        self.pool.nodes.truncate(nodes);
        self.pool.arguments.truncate(mark.arguments as usize);
        self.pool.origins.truncate(origins);
        self.pool.structs.truncate(mark.structs as usize);
        self.pool.interfaces.truncate(mark.interfaces as usize);
        self.pool.signatures.truncate(mark.signatures as usize);
        self.pool.type_params.truncate(mark.type_params as usize);
        self.pool.param_names.truncate(mark.param_names as usize);
        self.pool.fields.truncate(mark.fields as usize);
        self.pool.field_bytes = mark.field_bytes as usize;
        self.pool.methods.truncate(mark.methods as usize);
        self.pool.method_bytes = mark.method_bytes as usize;
        self.pool.terms.truncate(mark.terms as usize);
        self.pool
            .signature_names
            .truncate(mark.signature_names as usize);
        self.pool
            .signature_name_bytes
            .truncate(mark.signature_name_bytes as usize);
        self.local_text.truncate(mark.local_text as usize);
        if removed_nodes {
            self.pool.retain_above(nodes);
            self.local_origins.retain(|_, id| id.index() < nodes);
        }
        if removed_origins {
            self.local_type_meta.retain(|oid, _| oid.index() < origins);
        }
    }
}

/// RAII guard temporarily switching [`TypeStore::alloc_tier`] and restoring
/// the previous tier unconditionally on drop (including early return or panic).
pub(crate) struct AllocTierGuard<'s, 'a> {
    store: &'s mut TypeStore<'a>,
    saved_tier: AllocTier,
}

impl<'s, 'a> AllocTierGuard<'s, 'a> {
    pub(crate) fn new(store: &'s mut TypeStore<'a>, tier: AllocTier) -> Self {
        let saved_tier = store.alloc_tier;
        store.alloc_tier = tier;
        Self { store, saved_tier }
    }
}

impl<'a> Deref for AllocTierGuard<'_, 'a> {
    type Target = TypeStore<'a>;

    fn deref(&self) -> &Self::Target {
        self.store
    }
}

impl DerefMut for AllocTierGuard<'_, '_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.store
    }
}

impl Drop for AllocTierGuard<'_, '_> {
    fn drop(&mut self) {
        self.store.alloc_tier = self.saved_tier;
    }
}

/// RAII guard for speculative local projections. Captures a [`LocalWatermark`]
/// and ambient execution limits on entry, then unconditionally rewinds local
/// allocations and restores `mode`, `steps`, `alloc_tier`, `node_limit`, and
/// `byte_limit` on drop (even on early return or panic).
pub(crate) struct ProjectionGuard<'s, 'a> {
    store: &'s mut TypeStore<'a>,
    mark: LocalWatermark,
    saved_mode: CheckMode,
    saved_steps: StepBudget,
    saved_tier: AllocTier,
    saved_nodes: usize,
    saved_bytes: usize,
}

impl<'s, 'a> ProjectionGuard<'s, 'a> {
    pub(crate) fn new(store: &'s mut TypeStore<'a>, mode: CheckMode) -> Self {
        let mark = store.local.mark();
        let saved_mode = store.mode;
        let saved_steps = store.steps;
        let saved_tier = store.alloc_tier;
        let saved_nodes = store.local.pool.node_limit;
        let saved_bytes = store.local.pool.byte_limit;
        store.mode = mode;
        store.alloc_tier = AllocTier::Local;
        store.local.pool.node_limit =
            saved_nodes.min((mark.nodes as usize).saturating_add(MAX_NODES));
        store.local.pool.byte_limit = saved_bytes.min(
            store
                .local
                .pool
                .field_bytes
                .max(store.local.pool.method_bytes)
                .saturating_add(MAX_BYTES),
        );
        store.steps.reset();
        Self {
            store,
            mark,
            saved_mode,
            saved_steps,
            saved_tier,
            saved_nodes,
            saved_bytes,
        }
    }
}

impl<'a> Deref for ProjectionGuard<'_, 'a> {
    type Target = TypeStore<'a>;

    fn deref(&self) -> &Self::Target {
        self.store
    }
}

impl DerefMut for ProjectionGuard<'_, '_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.store
    }
}

impl Drop for ProjectionGuard<'_, '_> {
    fn drop(&mut self) {
        self.store.local.rewind(self.mark);
        self.store.mode = self.saved_mode;
        self.store.steps = self.saved_steps;
        self.store.alloc_tier = self.saved_tier;
        self.store.local.pool.node_limit = self.saved_nodes;
        self.store.local.pool.byte_limit = self.saved_bytes;
    }
}

/// Two-tier type store separating permanent declaration types from per-region locals.
pub(crate) struct TypeStore<'a> {
    pub(in crate::analysis::engine::types) workspace: &'a Workspace,
    pub(in crate::analysis::engine::types) build_view: BuildView<'a>,
    pub(in crate::analysis::engine::types) decl: DeclArena<'a>,
    pub(in crate::analysis::engine::types) local: LocalArena<'a>,
    pub(crate) env_cache: FileEnvCache<'a>,
    pub(in crate::analysis::engine::types) alloc_tier: AllocTier,
    pub(in crate::analysis::engine::types) mode: CheckMode,
    pub(in crate::analysis::engine::types) steps: StepBudget,
    pub(in crate::analysis::engine::types) named_check_depth: usize,
    pub(in crate::analysis::engine::types) expansion_depth: usize,
    pub(in crate::analysis::engine::types) constant_depth: usize,
    pub(in crate::analysis::engine::types) constraint_depth: usize,
}

impl<'a> TypeStore<'a> {
    pub(crate) fn with_shared_budget(workspace: &'a Workspace, build_view: BuildView<'a>) -> Self {
        Self {
            workspace,
            build_view,
            decl: DeclArena::new(),
            local: LocalArena::new(),
            env_cache: FileEnvCache::new(build_view),
            alloc_tier: AllocTier::Local,
            mode: CheckMode::Concrete,
            steps: StepBudget::new(RELATION_STEPS),
            named_check_depth: 0,
            expansion_depth: 0,
            constant_depth: 0,
            constraint_depth: 0,
        }
    }

    pub(crate) fn reset_local(&mut self) {
        self.local.reset();
        self.steps.reset();
        self.alloc_tier = AllocTier::Local;
    }

    /// Reports whether `LocalArena` still has at least half its node and byte
    /// budget available for a speculative projection without resetting the
    /// active region.
    #[allow(dead_code)]
    pub(crate) fn local_headroom(&self) -> bool {
        let pool = &self.local.pool;
        pool.nodes.len().saturating_mul(2) <= pool.node_limit
            && pool.arguments.len().saturating_mul(2) <= pool.node_limit
            && pool.field_bytes.max(pool.method_bytes).saturating_mul(2) <= pool.byte_limit
    }

    pub(crate) fn with_decl_alloc<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        let mut guard = AllocTierGuard::new(self, AllocTier::Decl);
        f(&mut guard)
    }

    pub(crate) fn with_local_alloc<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        let mut guard = AllocTierGuard::new(self, AllocTier::Local);
        f(&mut guard)
    }

    pub(crate) fn projection_guard(&mut self, mode: CheckMode) -> ProjectionGuard<'_, 'a> {
        ProjectionGuard::new(self, mode)
    }

    /// Speculative presentation/inference shares local storage. Callers return
    /// owned projections, never TypeIds allocated after the watermark.
    pub(crate) fn with_projection<R>(
        &mut self,
        symbolic: bool,
        project: impl FnOnce(&mut Self) -> R,
    ) -> R {
        let mode = if symbolic {
            CheckMode::Symbolic
        } else {
            CheckMode::Concrete
        };
        let mut guard = self.projection_guard(mode);
        project(&mut guard)
    }

    pub(crate) fn resolver(&self) -> crate::analysis::engine::resolve::View<'a> {
        crate::analysis::engine::resolve::View::new(self.workspace, self.build_view)
    }

    pub(crate) fn integer_bits(&self) -> Option<u32> {
        self.build_view.integer_bits()
    }

    pub(crate) fn workspace(&self) -> &'a Workspace {
        self.workspace
    }

    pub(crate) fn reset_steps(&mut self) {
        self.steps.reset();
    }

    pub(in crate::analysis::engine::types) fn step(&mut self) -> Option<()> {
        self.steps.step()
    }

    pub(in crate::analysis::engine::types) fn node(&self, id: TypeId) -> Option<&Ty> {
        match id.tier() {
            TypeTier::Invalid => Some(&Ty::Invalid),
            TypeTier::Builtin => id.as_basic().map(|kind| &BUILTIN_TYS[kind.index()]),
            TypeTier::Decl => self.decl.pool.nodes.get(id.index()),
            TypeTier::Local { epoch } => {
                debug_assert_eq!(epoch, self.local.epoch & 0x7FFF, "stale local TypeId");
                (epoch == (self.local.epoch & 0x7FFF))
                    .then(|| self.local.pool.nodes.get(id.index()))
                    .flatten()
            }
        }
    }

    pub(in crate::analysis::engine::types) fn origin_data(
        &self,
        id: OriginId,
    ) -> Option<&NamedOriginData<'a>> {
        if id.is_local() {
            self.local.pool.origins.get(id.index())
        } else {
            self.decl.pool.origins.get(id.index())
        }
    }

    pub(in crate::analysis::engine::types) fn origin_data_mut(
        &mut self,
        id: OriginId,
    ) -> Option<&mut NamedOriginData<'a>> {
        if id.is_local() {
            self.local.pool.origins.get_mut(id.index())
        } else {
            self.decl.pool.origins.get_mut(id.index())
        }
    }

    pub(in crate::analysis::engine::types) fn get_or_insert_package_origin(
        &mut self,
        declaration: DeclId<'a>,
        alias: bool,
    ) -> Option<OriginId> {
        if let Some(&id) = self.decl.package_origins.get(&declaration) {
            return Some(id);
        }
        let id = self.decl.pool.push_origin_data(NamedOriginData::new(
            TypeOrigin::Package(declaration),
            alias,
        ))?;
        self.decl.package_origins.insert(declaration, id);
        Some(id)
    }

    pub(in crate::analysis::engine::types) fn struct_data(
        &self,
        owner: TypeId,
    ) -> Option<&StructData<'a>> {
        let Ty::Struct(id) = *self.node(owner)? else {
            return None;
        };
        self.pool_for(owner).structs.get(id.0 as usize)
    }

    pub(in crate::analysis::engine::types) fn interface_data(
        &self,
        owner: TypeId,
    ) -> Option<&InterfaceData<'a>> {
        let Ty::Interface(id) = *self.node(owner)? else {
            return None;
        };
        self.pool_for(owner).interfaces.get(id.0 as usize)
    }

    pub(in crate::analysis::engine::types) fn signature_data(
        &self,
        owner: TypeId,
    ) -> Option<&SignatureData> {
        let Ty::Signature(id) = *self.node(owner)? else {
            return None;
        };
        self.pool_for(owner).signatures.get(id.0 as usize)
    }

    pub(in crate::analysis::engine::types) fn type_param_data(
        &self,
        owner: TypeId,
    ) -> Option<&TypeParamData<'a>> {
        let Ty::TypeParam(id) = *self.node(owner)? else {
            return None;
        };
        self.pool_for(owner).type_params.get(id.0 as usize)
    }

    pub(in crate::analysis::engine::types) fn type_param_data_mut(
        &mut self,
        owner: TypeId,
    ) -> Option<&mut TypeParamData<'a>> {
        let Ty::TypeParam(id) = *self.node(owner)? else {
            return None;
        };
        self.pool_for_mut(owner).type_params.get_mut(id.0 as usize)
    }

    #[allow(dead_code)]
    pub(in crate::analysis::engine::types) fn type_param_index(
        &self,
        owner: TypeId,
    ) -> Option<usize> {
        self.type_param_data(owner).map(|p| usize::from(p.index))
    }

    pub(in crate::analysis::engine::types) fn type_param_bound(
        &self,
        owner: TypeId,
    ) -> Option<TypeId> {
        self.type_param_data(owner)?.bound()
    }

    pub(in crate::analysis::engine::types) fn set_type_param_bound(
        &mut self,
        owner: TypeId,
        bound: TypeId,
    ) -> bool {
        if !owner.is_local() && bound.is_local() {
            return false;
        }
        let Some(data) = self.type_param_data_mut(owner) else {
            return false;
        };
        data.set_bound(bound);
        true
    }

    pub(in crate::analysis::engine::types) fn type_param_info(
        &self,
        owner: TypeId,
    ) -> Option<(&'a Path, usize, &str)> {
        let Ty::TypeParam(id) = *self.node(owner)? else {
            return None;
        };
        let pool = self.pool_for(owner);
        let data = *pool.type_params.get(id.0 as usize)?;
        if !data.is_ready() || data.is_synthetic() {
            return None;
        }
        let origin = data.origin?;
        let name = pool.param_names.get(data.name_range())?;
        Some((origin, data.selection as usize, name))
    }

    pub(in crate::analysis::engine::types) fn type_param_name(
        &self,
        owner: TypeId,
    ) -> Option<&str> {
        let Ty::TypeParam(id) = *self.node(owner)? else {
            return None;
        };
        let pool = self.pool_for(owner);
        let data = *pool.type_params.get(id.0 as usize)?;
        pool.param_names.get(data.name_range())
    }

    pub(in crate::analysis::engine::types) fn named_origin_info(
        &self,
        owner: TypeId,
    ) -> Option<(OriginId, DeclId<'a>, SliceRef, bool)> {
        let (origin_id, arguments, alias) = match *self.node(owner)? {
            Ty::Named { origin, arguments } => (origin, arguments, false),
            Ty::Alias { origin, arguments } => (origin, arguments, true),
            _ => return None,
        };
        let TypeOrigin::Package(decl) = self.origin_data(origin_id)?.origin else {
            return None;
        };
        Some((origin_id, decl, arguments, alias))
    }

    pub(in crate::analysis::engine::types) fn named_info(
        &self,
        owner: TypeId,
    ) -> Option<(DeclId<'a>, SliceRef, bool)> {
        let (_, decl, arguments, alias) = self.named_origin_info(owner)?;
        Some((decl, arguments, alias))
    }

    pub(in crate::analysis::engine::types) fn local_origin_id(
        &self,
        owner: TypeId,
    ) -> Option<OriginId> {
        let origin_id = match *self.node(owner)? {
            Ty::Named { origin, .. } | Ty::Alias { origin, .. } => origin,
            _ => return None,
        };
        matches!(
            self.origin_data(origin_id)?.origin,
            TypeOrigin::Local { .. }
        )
        .then_some(origin_id)
    }

    pub(in crate::analysis::engine::types) fn local_decl_info(
        &self,
        owner: TypeId,
    ) -> Option<(OriginId, &'a Path, Range<usize>, bool)> {
        let (origin_id, alias) = match *self.node(owner)? {
            Ty::Named { origin, .. } => (origin, false),
            Ty::Alias { origin, .. } => (origin, true),
            _ => return None,
        };
        let TypeOrigin::Local { origin, start, end } = self.origin_data(origin_id)?.origin else {
            return None;
        };
        Some((origin_id, origin, start as usize..end as usize, alias))
    }

    pub(in crate::analysis::engine::types) fn error_interface(
        &self,
        owner: TypeId,
    ) -> Option<TypeId> {
        let Ty::Named { origin, .. } = *self.node(owner)? else {
            return None;
        };
        let TypeOrigin::BuiltinError { interface } = self.origin_data(origin)?.origin else {
            return None;
        };
        Some(interface)
    }

    pub(in crate::analysis::engine::types) fn pool_for(&self, id: TypeId) -> &ArenaPool<'a> {
        if id.is_local() {
            &self.local.pool
        } else {
            &self.decl.pool
        }
    }

    pub(in crate::analysis::engine::types) fn pool_for_mut(
        &mut self,
        id: TypeId,
    ) -> &mut ArenaPool<'a> {
        if id.is_local() {
            &mut self.local.pool
        } else {
            &mut self.decl.pool
        }
    }

    pub(in crate::analysis::engine::types) fn target_pool_mut(
        &mut self,
        decl_safe: bool,
    ) -> &mut ArenaPool<'a> {
        if self.alloc_tier == AllocTier::Decl && decl_safe {
            &mut self.decl.pool
        } else {
            &mut self.local.pool
        }
    }

    pub(in crate::analysis::engine::types) fn active_node_limit(&self) -> usize {
        if self.alloc_tier == AllocTier::Decl {
            self.decl.pool.node_limit
        } else {
            self.local.pool.node_limit
        }
    }

    pub(in crate::analysis::engine::types) fn arguments_slice(
        &self,
        owner: TypeId,
        slice: SliceRef,
    ) -> &[TypeId] {
        &self.pool_for(owner).arguments[slice.range()]
    }

    pub(in crate::analysis::engine::types) fn arg_range(
        &self,
        owner: TypeId,
        range: Range<usize>,
    ) -> &[TypeId] {
        &self.pool_for(owner).arguments[range]
    }

    pub(in crate::analysis::engine::types) fn fields_slice(
        &self,
        owner: TypeId,
        range: Range<usize>,
    ) -> Option<&[Field]> {
        self.pool_for(owner).fields.get(range)
    }

    pub(in crate::analysis::engine::types) fn methods_slice(
        &self,
        owner: TypeId,
        range: Range<usize>,
    ) -> &[Method<'a>] {
        &self.pool_for(owner).methods[range]
    }

    pub(in crate::analysis::engine::types) fn methods_slice_mut(
        &mut self,
        owner: TypeId,
        range: Range<usize>,
    ) -> &mut [Method<'a>] {
        &mut self.pool_for_mut(owner).methods[range]
    }

    pub(in crate::analysis::engine::types) fn terms_slice(
        &self,
        owner: TypeId,
        slice: SliceRef,
    ) -> &[Term] {
        &self.pool_for(owner).terms[slice.range()]
    }
}
