//! Type node allocation, interning, and cache helpers on [`TypeStore`].
use super::TypeStore;
use crate::analysis::engine::budget::MAX_BYTES;
use crate::analysis::engine::obj::DeclId;
use crate::analysis::engine::types::ty::{
    AllocTier, InterfaceData, InterfaceId, NamedCheckState, NamedOriginData, SignatureData,
    SignatureId, SliceRef, StructData, StructId, Term, TypeOrigin,
};
use crate::analysis::engine::types::{BasicKind, Field, Method, Ty, TypeId};
use std::path::Path;

impl<'a> TypeStore<'a> {
    pub(in crate::analysis::engine::types) fn push_type_param(
        &mut self,
        index: usize,
    ) -> Option<TypeId> {
        let index = u16::try_from(index).ok()?;
        let decl_safe = self.alloc_tier == AllocTier::Decl;
        self.target_pool_mut(decl_safe)
            .push_type_param_data(None, u32::MAX, index, "", false, true)
    }

    pub(in crate::analysis::engine::types) fn push_named_type_param(
        &mut self,
        origin: &'a Path,
        selection: u32,
        index: u16,
        name: &str,
        ready: bool,
    ) -> Option<TypeId> {
        let decl_safe = self.alloc_tier == AllocTier::Decl;
        self.target_pool_mut(decl_safe).push_type_param_data(
            Some(origin),
            selection,
            index,
            name,
            ready,
            false,
        )
    }

    pub(in crate::analysis::engine::types) fn push_builtin_error(
        &mut self,
        interface: TypeId,
    ) -> Option<TypeId> {
        let decl_safe = interface.is_decl_safe();
        self.target_pool_mut(decl_safe)
            .push_origin_node(
                NamedOriginData::new(TypeOrigin::BuiltinError { interface }, false),
                false,
            )
            .map(|(_, id)| id)
    }

    pub(in crate::analysis::engine::types) fn store_union(
        &mut self,
        terms: Vec<Term>,
    ) -> Option<TypeId> {
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

    pub(in crate::analysis::engine::types) fn push(&mut self, ty: Ty) -> Option<TypeId> {
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
        let origin = self.get_or_insert_package_origin(declaration, alias)?;
        let decl_safe = arguments.iter().all(|id| id.is_decl_safe());
        self.target_pool_mut(decl_safe)
            .push_named_instance(origin, arguments, alias)
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

    pub(in crate::analysis::engine::types) fn store_struct_nodes(
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

    #[allow(clippy::too_many_arguments)]
    pub(in crate::analysis::engine::types) fn store_interface_nodes(
        &mut self,
        origin: &'a Path,
        methods: Vec<Method<'a>>,
        embeddeds: Vec<TypeId>,
        has_type_elements: bool,
        comparable: bool,
        implicit: bool,
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
            implicit,
            builtin_any,
        });
        let Some(id) = pool.push_raw(Ty::Interface(iface_id)) else {
            pool.interfaces.truncate(iface_idx);
            pool.arguments.truncate(emb_start);
            return None;
        };
        Some(id)
    }

    pub(in crate::analysis::engine::types) fn store_signature_nodes(
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

    pub(in crate::analysis::engine::types) fn expansion_get(&self, id: TypeId) -> Option<TypeId> {
        self.pool_for(id).expansions.get(&id).copied()
    }

    pub(in crate::analysis::engine::types) fn expansion_insert(
        &mut self,
        id: TypeId,
        target: TypeId,
    ) {
        if !id.is_local() && target.is_local() {
            return;
        }
        let pool = self.pool_for_mut(id);
        if pool.expansions.len() < pool.node_limit {
            pool.expansions.insert(id, target);
        }
    }

    pub(in crate::analysis::engine::types) fn named_check_state(
        &self,
        id: TypeId,
    ) -> Option<NamedCheckState> {
        self.pool_for(id).named_checks.get(&id).copied()
    }

    pub(in crate::analysis::engine::types) fn named_check_set(
        &mut self,
        id: TypeId,
        state: NamedCheckState,
    ) -> bool {
        let pool = self.pool_for_mut(id);
        if pool.named_checks.len() >= pool.node_limit && !pool.named_checks.contains_key(&id) {
            return false;
        }
        pool.named_checks.insert(id, state);
        true
    }

    pub(in crate::analysis::engine::types) fn named_check_remove(&mut self, id: TypeId) {
        self.pool_for_mut(id).named_checks.remove(&id);
    }
}
