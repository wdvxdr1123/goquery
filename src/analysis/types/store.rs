//! Two-tier type storage (`DeclArena` + `LocalArena`) with epoch-checked [`TypeId`]s.

use super::lookup::ConcreteMethodMemo;
use super::pool::ArenaPool;
use super::ty::{
    AllocTier, BUILTIN_TYS, CheckMode, InterfaceData, InterfaceId, OriginId, SignatureData,
    SignatureId, SliceRef, StepBudget, StructData, StructId, Term, TypeOrigin, TypeTier,
};
use super::{BasicKind, Field, Method, Ty, TypeId};
use crate::analysis::engine::budget::{
    MAX_BYTES, MAX_DECL_ARENA_NODES, MAX_LOCAL_ARENA_NODES, MAX_NODES, RELATION_STEPS,
};
use crate::analysis::engine::obj::DeclId;
use crate::analysis::engine::resolve::FileEnvCache;
use crate::workspace::{BuildView, Workspace};
use std::collections::HashMap;
use std::ops::Range;
use std::path::Path;

/// Request-scoped storage for package-level declaration types (never rewound).
pub(super) struct DeclArena<'a> {
    pub(super) pool: ArenaPool<'a>,
    pub(super) concrete_methods: HashMap<DeclId<'a>, ConcreteMethodMemo>,
}

impl<'a> DeclArena<'a> {
    fn new() -> Self {
        Self {
            pool: ArenaPool::new(false, 0, MAX_DECL_ARENA_NODES, MAX_BYTES * 3),
            concrete_methods: HashMap::new(),
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct LocalWatermark {
    nodes: usize,
    arguments: usize,
    origins: usize,
    structs: usize,
    interfaces: usize,
    signatures: usize,
    type_params: usize,
    constraint_terms: usize,
    local_parameters: usize,
    local_types: usize,
    local_type_bytes: usize,
    fields: usize,
    field_bytes: usize,
    methods: usize,
    method_bytes: usize,
    terms: usize,
    signature_names: usize,
    signature_name_bytes: usize,
}

/// Region-scoped storage for local types, parameters, and speculative probes.
pub(super) struct LocalArena<'a> {
    pub(super) epoch: u16,
    pub(super) pool: ArenaPool<'a>,
    pub(super) local_parameters: Vec<super::parameters::LocalParameter<'a>>,
    pub(super) local_types: Vec<super::declaration::LocalTypeFact<'a>>,
    pub(super) local_type_bytes: usize,
    pub(super) constraint_terms: usize,
}

impl<'a> LocalArena<'a> {
    fn new() -> Self {
        Self {
            epoch: 1,
            pool: ArenaPool::new(true, 1, MAX_LOCAL_ARENA_NODES, MAX_BYTES),
            local_parameters: Vec::new(),
            local_types: Vec::new(),
            local_type_bytes: 0,
            constraint_terms: 0,
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
        self.pool.parameter_constraints.clear();
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
        self.local_parameters.clear();
        self.local_types.clear();
        self.local_type_bytes = 0;
        self.constraint_terms = 0;
        self.pool.node_limit = MAX_LOCAL_ARENA_NODES;
        self.pool.byte_limit = MAX_BYTES;
    }

    fn mark(&self) -> LocalWatermark {
        LocalWatermark {
            nodes: self.pool.nodes.len(),
            arguments: self.pool.arguments.len(),
            origins: self.pool.origins.len(),
            structs: self.pool.structs.len(),
            interfaces: self.pool.interfaces.len(),
            signatures: self.pool.signatures.len(),
            type_params: self.pool.type_params.len(),
            constraint_terms: self.constraint_terms,
            local_parameters: self.local_parameters.len(),
            local_types: self.local_types.len(),
            local_type_bytes: self.local_type_bytes,
            fields: self.pool.fields.len(),
            field_bytes: self.pool.field_bytes,
            methods: self.pool.methods.len(),
            method_bytes: self.pool.method_bytes,
            terms: self.pool.terms.len(),
            signature_names: self.pool.signature_names.len(),
            signature_name_bytes: self.pool.signature_name_bytes.len(),
        }
    }

    fn rewind(&mut self, mark: LocalWatermark) {
        let removed_nodes = self.pool.nodes.len() > mark.nodes;
        self.pool.nodes.truncate(mark.nodes);
        self.pool.arguments.truncate(mark.arguments);
        self.pool.origins.truncate(mark.origins);
        self.pool.structs.truncate(mark.structs);
        self.pool.interfaces.truncate(mark.interfaces);
        self.pool.signatures.truncate(mark.signatures);
        self.pool.type_params.truncate(mark.type_params);
        self.constraint_terms = mark.constraint_terms;
        self.local_parameters.truncate(mark.local_parameters);
        self.local_types.truncate(mark.local_types);
        self.local_type_bytes = mark.local_type_bytes;
        self.pool.fields.truncate(mark.fields);
        self.pool.field_bytes = mark.field_bytes;
        self.pool.methods.truncate(mark.methods);
        self.pool.method_bytes = mark.method_bytes;
        self.pool.terms.truncate(mark.terms);
        self.pool.signature_names.truncate(mark.signature_names);
        self.pool
            .signature_name_bytes
            .truncate(mark.signature_name_bytes);
        if removed_nodes {
            self.pool.retain_above(mark.nodes);
        }
    }
}

/// Two-tier type store separating permanent declaration types from per-region locals.
pub(crate) struct TypeStore<'a> {
    pub(super) workspace: &'a Workspace,
    pub(super) build_view: BuildView<'a>,
    pub(super) decl: DeclArena<'a>,
    pub(super) local: LocalArena<'a>,
    pub(crate) env_cache: FileEnvCache<'a>,
    pub(super) alloc_tier: AllocTier,
    pub(super) mode: CheckMode,
    pub(super) steps: StepBudget,
    pub(super) named_check_depth: usize,
    pub(super) expansion_depth: usize,
    pub(super) constant_depth: usize,
    pub(super) constraint_depth: usize,
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

    pub(crate) fn with_decl_alloc<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        let saved = self.alloc_tier;
        self.alloc_tier = AllocTier::Decl;
        let result = f(self);
        self.alloc_tier = saved;
        result
    }

    pub(crate) fn with_local_alloc<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        let saved = self.alloc_tier;
        self.alloc_tier = AllocTier::Local;
        let result = f(self);
        self.alloc_tier = saved;
        result
    }

    /// Speculative presentation/inference shares local storage. Callers return
    /// owned projections, never TypeIds allocated after the watermark.
    pub(crate) fn with_projection<R>(
        &mut self,
        symbolic: bool,
        project: impl FnOnce(&mut Self) -> R,
    ) -> R {
        let mark = self.local.mark();
        let saved_mode = self.mode;
        let saved_steps = self.steps;
        let saved_tier = self.alloc_tier;
        let saved_nodes = self.local.pool.node_limit;
        let saved_bytes = self.local.pool.byte_limit;
        self.mode = if symbolic {
            CheckMode::Symbolic
        } else {
            CheckMode::Concrete
        };
        self.alloc_tier = AllocTier::Local;
        self.local.pool.node_limit = saved_nodes.min(mark.nodes.saturating_add(MAX_NODES));
        self.local.pool.byte_limit = saved_bytes.min(
            self.local
                .pool
                .field_bytes
                .max(self.local.pool.method_bytes)
                .saturating_add(MAX_BYTES),
        );
        self.steps.reset();
        let result = project(self);
        self.local.rewind(mark);
        self.mode = saved_mode;
        self.steps = saved_steps;
        self.alloc_tier = saved_tier;
        self.local.pool.node_limit = saved_nodes;
        self.local.pool.byte_limit = saved_bytes;
        result
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

    pub(super) fn step(&mut self) -> Option<()> {
        self.steps.step()
    }

    pub(super) fn node(&self, id: TypeId) -> Option<&Ty> {
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

    pub(super) fn origin_data(&self, owner: TypeId, id: OriginId) -> Option<&TypeOrigin<'a>> {
        self.pool_for(owner).origins.get(id.0 as usize)
    }

    pub(super) fn struct_data(&self, owner: TypeId) -> Option<&StructData<'a>> {
        let Ty::Struct(id) = *self.node(owner)? else {
            return None;
        };
        self.pool_for(owner).structs.get(id.0 as usize)
    }

    pub(super) fn interface_data(&self, owner: TypeId) -> Option<&InterfaceData<'a>> {
        let Ty::Interface(id) = *self.node(owner)? else {
            return None;
        };
        self.pool_for(owner).interfaces.get(id.0 as usize)
    }

    pub(super) fn signature_data(&self, owner: TypeId) -> Option<&SignatureData> {
        let Ty::Signature(id) = *self.node(owner)? else {
            return None;
        };
        self.pool_for(owner).signatures.get(id.0 as usize)
    }

    pub(super) fn type_param_index(&self, owner: TypeId) -> Option<usize> {
        let Ty::TypeParam(id) = *self.node(owner)? else {
            return None;
        };
        self.pool_for(owner)
            .type_params
            .get(id.0 as usize)
            .map(|p| p.index)
    }

    pub(super) fn named_info(&self, owner: TypeId) -> Option<(DeclId<'a>, SliceRef, bool)> {
        let (origin, arguments, alias) = match *self.node(owner)? {
            Ty::Named { origin, arguments } => (origin, arguments, false),
            Ty::Alias { origin, arguments } => (origin, arguments, true),
            _ => return None,
        };
        let TypeOrigin::Package(decl) = *self.origin_data(owner, origin)? else {
            return None;
        };
        Some((decl, arguments, alias))
    }

    pub(super) fn local_decl_info(
        &self,
        owner: TypeId,
    ) -> Option<(&'a Path, Range<usize>, bool, bool)> {
        let (origin, alias) = match *self.node(owner)? {
            Ty::Named { origin, .. } => (origin, false),
            Ty::Alias { origin, .. } => (origin, true),
            _ => return None,
        };
        let TypeOrigin::Local {
            origin,
            start,
            end,
            formal,
        } = *self.origin_data(owner, origin)?
        else {
            return None;
        };
        Some((origin, start as usize..end as usize, formal, alias))
    }

    pub(super) fn error_interface(&self, owner: TypeId) -> Option<TypeId> {
        let Ty::Named { origin, .. } = *self.node(owner)? else {
            return None;
        };
        let TypeOrigin::BuiltinError { interface } = *self.origin_data(owner, origin)? else {
            return None;
        };
        Some(interface)
    }

    pub(super) fn push_type_param(&mut self, index: usize) -> Option<TypeId> {
        let decl_safe = self.alloc_tier == AllocTier::Decl;
        self.target_pool_mut(decl_safe).push_type_param_raw(index)
    }

    pub(super) fn push_local_declaration(
        &mut self,
        origin: &'a Path,
        selection: Range<usize>,
        formal: bool,
    ) -> Option<TypeId> {
        let start = u32::try_from(selection.start).ok()?;
        let end = u32::try_from(selection.end).ok()?;
        self.local.pool.push_origin_raw(
            TypeOrigin::Local {
                origin,
                start,
                end,
                formal,
            },
            false,
        )
    }

    pub(super) fn push_builtin_error(&mut self, interface: TypeId) -> Option<TypeId> {
        let decl_safe = interface.is_decl_safe();
        self.target_pool_mut(decl_safe)
            .push_origin_raw(TypeOrigin::BuiltinError { interface }, false)
    }

    pub(super) fn pool_for(&self, id: TypeId) -> &ArenaPool<'a> {
        if id.is_local() {
            &self.local.pool
        } else {
            &self.decl.pool
        }
    }

    pub(super) fn pool_for_mut(&mut self, id: TypeId) -> &mut ArenaPool<'a> {
        if id.is_local() {
            &mut self.local.pool
        } else {
            &mut self.decl.pool
        }
    }

    pub(super) fn target_pool_mut(&mut self, decl_safe: bool) -> &mut ArenaPool<'a> {
        if self.alloc_tier == AllocTier::Decl && decl_safe {
            &mut self.decl.pool
        } else {
            &mut self.local.pool
        }
    }

    pub(super) fn active_node_limit(&self) -> usize {
        if self.alloc_tier == AllocTier::Decl {
            self.decl.pool.node_limit
        } else {
            self.local.pool.node_limit
        }
    }

    pub(super) fn arguments_slice(&self, owner: TypeId, slice: SliceRef) -> &[TypeId] {
        &self.pool_for(owner).arguments[slice.range()]
    }

    pub(super) fn arg_range(&self, owner: TypeId, range: Range<usize>) -> &[TypeId] {
        &self.pool_for(owner).arguments[range]
    }

    pub(super) fn fields_slice(&self, owner: TypeId, range: Range<usize>) -> Option<&[Field]> {
        self.pool_for(owner).fields.get(range)
    }

    pub(super) fn methods_slice(&self, owner: TypeId, range: Range<usize>) -> &[Method<'a>] {
        &self.pool_for(owner).methods[range]
    }

    pub(super) fn methods_slice_mut(
        &mut self,
        owner: TypeId,
        range: Range<usize>,
    ) -> &mut [Method<'a>] {
        &mut self.pool_for_mut(owner).methods[range]
    }

    pub(super) fn terms_slice(&self, owner: TypeId, slice: SliceRef) -> &[Term] {
        &self.pool_for(owner).terms[slice.range()]
    }

    pub(super) fn store_union(&mut self, terms: Vec<Term>) -> Option<TypeId> {
        let decl_safe = terms.iter().all(|t| t.ty.is_decl_safe());
        let pool = self.target_pool_mut(decl_safe);
        if pool.terms.len() + terms.len() > pool.node_limit {
            return None;
        }
        let start = pool.terms.len();
        pool.terms.extend(terms);
        let Some(slice) = SliceRef::from_range(start..pool.terms.len()) else {
            pool.terms.truncate(start);
            return None;
        };
        let Some(id) = pool.push_raw(Ty::Union(slice)) else {
            pool.terms.truncate(start);
            return None;
        };
        Some(id)
    }

    pub(crate) fn basic(&self, kind: BasicKind) -> Option<TypeId> {
        Some(TypeId::basic(kind))
    }

    pub(crate) fn basic_kind(&mut self, id: TypeId) -> Option<BasicKind> {
        let id = self.underlying(id)?;
        match self.node(id)? {
            Ty::Basic(kind) => Some(*kind),
            _ => None,
        }
    }

    pub(super) fn push(&mut self, ty: Ty) -> Option<TypeId> {
        let decl_safe = match ty {
            Ty::Invalid | Ty::Basic(_) => return self.local.pool.push_interned(ty),
            Ty::Pointer(element)
            | Ty::Slice(element)
            | Ty::Array { element, .. }
            | Ty::Chan { element, .. } => element.is_decl_safe(),
            Ty::Map { key, value } => key.is_decl_safe() && value.is_decl_safe(),
            Ty::TypeParam(_)
            | Ty::Named { .. }
            | Ty::Alias { .. }
            | Ty::Tuple(_)
            | Ty::Union(_)
            | Ty::Struct(_)
            | Ty::Interface(_)
            | Ty::Signature(_) => false,
        };
        self.target_pool_mut(decl_safe).push_interned(ty)
    }

    pub(crate) fn push_named(
        &mut self,
        declaration: DeclId<'a>,
        arguments: &[TypeId],
        alias: bool,
    ) -> Option<TypeId> {
        let decl_safe = arguments.iter().all(|id| id.is_decl_safe());
        self.target_pool_mut(decl_safe)
            .push_named(declaration, arguments, alias)
    }

    pub(crate) fn tuple(&mut self, elements: &[TypeId]) -> Option<TypeId> {
        let decl_safe = elements.iter().all(|id| id.is_decl_safe());
        self.target_pool_mut(decl_safe).push_tuple(elements)
    }

    pub(crate) fn pointer(&mut self, element: TypeId) -> Option<TypeId> {
        self.push(Ty::Pointer(element))
    }

    pub(crate) fn slice_type(&mut self, element: TypeId) -> Option<TypeId> {
        self.push(Ty::Slice(element))
    }

    pub(crate) fn array(&mut self, element: TypeId, length: u64) -> Option<TypeId> {
        let maximum = (1_u64 << (self.integer_bits()? - 1)) - 1;
        (length <= maximum).then_some(())?;
        self.push(Ty::Array { length, element })
    }

    pub(crate) fn array_length(&mut self, id: TypeId) -> Option<u64> {
        let root = self.underlying(id)?;
        match self.node(root)? {
            Ty::Array { length, .. } => Some(*length),
            _ => None,
        }
    }

    pub(super) fn store_struct_nodes(
        &mut self,
        origin: &'a Path,
        fields: Vec<Field>,
    ) -> Option<TypeId> {
        let decl_safe = fields.iter().all(|field| field.ty.is_decl_safe());
        let bytes: usize = fields
            .iter()
            .map(|field| field.name.len() + field.tag.len())
            .sum();
        let pool = self.target_pool_mut(decl_safe);
        if pool.fields.len() + fields.len() > pool.node_limit
            || pool.structs.len() >= pool.node_limit
            || bytes > MAX_BYTES
            || pool.field_bytes + bytes > pool.byte_limit
        {
            return None;
        }
        let struct_idx = pool.structs.len();
        let struct_id = StructId(u32::try_from(struct_idx).ok()?);
        pool.field_bytes += bytes;
        let start = pool.fields.len();
        pool.fields.extend(fields);
        let range = start..pool.fields.len();
        pool.structs.push(StructData {
            origin,
            fields: range,
        });
        let Some(id) = pool.push_raw(Ty::Struct(struct_id)) else {
            pool.structs.truncate(struct_idx);
            return None;
        };
        Some(id)
    }

    pub(super) fn store_interface_nodes(
        &mut self,
        origin: &'a Path,
        methods: Vec<Method<'a>>,
        embeddeds: Vec<TypeId>,
        has_type_elements: bool,
        comparable: bool,
        builtin_any: bool,
    ) -> Option<TypeId> {
        let decl_safe = methods
            .iter()
            .all(|m| m.signature.is_decl_safe() && m.declared_on.is_none_or(TypeId::is_decl_safe))
            && embeddeds.iter().all(|id| id.is_decl_safe());
        let bytes = methods.iter().map(|m| m.name.len()).sum::<usize>();
        let pool = self.target_pool_mut(decl_safe);
        if pool.methods.len() + methods.len() > pool.node_limit
            || pool.arguments.len() + embeddeds.len() > pool.node_limit
            || pool.interfaces.len() >= pool.node_limit
            || bytes > MAX_BYTES
            || pool.method_bytes + bytes > pool.byte_limit
        {
            return None;
        }
        let emb_start = pool.arguments.len();
        pool.arguments.extend(embeddeds);
        let Some(emb_slice) = SliceRef::from_range(emb_start..pool.arguments.len()) else {
            pool.arguments.truncate(emb_start);
            return None;
        };
        let iface_idx = pool.interfaces.len();
        let iface_id = InterfaceId(u32::try_from(iface_idx).ok()?);
        pool.method_bytes += bytes;
        let start = pool.methods.len();
        pool.methods.extend(methods);
        let range = start..pool.methods.len();
        pool.interfaces.push(InterfaceData {
            origin,
            methods: range,
            embeddeds: emb_slice,
            has_type_elements,
            comparable,
            builtin_any,
        });
        let Some(id) = pool.push_raw(Ty::Interface(iface_id)) else {
            pool.interfaces.truncate(iface_idx);
            pool.arguments.truncate(emb_start);
            return None;
        };
        Some(id)
    }

    pub(super) fn store_signature_nodes(
        &mut self,
        type_params: Vec<TypeId>,
        parameters: Vec<TypeId>,
        results: Vec<TypeId>,
        variadic: bool,
    ) -> Option<TypeId> {
        let decl_safe = type_params
            .iter()
            .chain(&parameters)
            .chain(&results)
            .all(|id| id.is_decl_safe());
        let pool = self.target_pool_mut(decl_safe);
        if pool.arguments.len() + type_params.len() + parameters.len() + results.len()
            > pool.node_limit
            || pool.signatures.len() >= pool.node_limit
        {
            return None;
        }
        let sig_idx = pool.signatures.len();
        let sig_id = SignatureId(u32::try_from(sig_idx).ok()?);
        let tp_start = pool.arguments.len();
        pool.arguments.extend(type_params);
        let tp_end = pool.arguments.len();
        let type_params = SliceRef::from_range(tp_start..tp_end)?;
        let start = pool.arguments.len();
        pool.arguments.extend(parameters);
        let middle = pool.arguments.len();
        pool.arguments.extend(results);
        let end = pool.arguments.len();
        pool.signatures.push(SignatureData {
            type_params,
            parameters: start..middle,
            results: middle..end,
            variadic,
        });
        let Some(id) = pool.push_raw(Ty::Signature(sig_id)) else {
            pool.signatures.truncate(sig_idx);
            pool.arguments.truncate(tp_start);
            return None;
        };
        Some(id)
    }

    pub(super) fn expansion_get(&self, id: TypeId) -> Option<TypeId> {
        self.pool_for(id).expansions.get(&id).copied()
    }

    pub(super) fn expansion_insert(&mut self, id: TypeId, target: TypeId) {
        if !id.is_local() && target.is_local() {
            return;
        }
        let pool = self.pool_for_mut(id);
        if pool.expansions.len() < pool.node_limit {
            pool.expansions.insert(id, target);
        }
    }

    pub(super) fn named_check_get(&self, id: TypeId) -> Option<bool> {
        self.pool_for(id).named_checks.get(&id).copied()
    }

    pub(super) fn named_check_set(&mut self, id: TypeId, valid: bool) -> bool {
        let pool = self.pool_for_mut(id);
        if pool.named_checks.len() >= pool.node_limit && !pool.named_checks.contains_key(&id) {
            return false;
        }
        pool.named_checks.insert(id, valid);
        true
    }

    pub(super) fn named_check_remove(&mut self, id: TypeId) {
        self.pool_for_mut(id).named_checks.remove(&id);
    }
}
