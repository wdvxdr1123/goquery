//! Tiered type-node pool (`ArenaPool`) shared between `DeclArena` and `LocalArena`.
use super::signature_names::SignatureNames;
use super::ty::{
    InterfaceData, NamedCheckState, NamedOriginData, OriginId, SignatureData, SliceRef, StructData,
    Term, TypeParamData, TypeParamId,
};
use super::{Direction, Field, Method, Ty, TypeId};
use std::collections::HashMap;
use std::path::Path;

#[derive(Clone, PartialEq, Eq, Hash)]
pub(super) enum InternArgs {
    Inline { len: u8, items: [TypeId; 2] },
    Heap(Box<[TypeId]>),
}

impl InternArgs {
    fn from_slice(slice: &[TypeId]) -> Self {
        match slice {
            [] => Self::Inline {
                len: 0,
                items: [TypeId::INVALID, TypeId::INVALID],
            },
            [a] => Self::Inline {
                len: 1,
                items: [*a, TypeId::INVALID],
            },
            [a, b] => Self::Inline {
                len: 2,
                items: [*a, *b],
            },
            many => Self::Heap(many.into()),
        }
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub(super) enum InternKey {
    Pointer(TypeId),
    Slice(TypeId),
    Array {
        length: u64,
        element: TypeId,
    },
    Map {
        key: TypeId,
        value: TypeId,
    },
    Chan {
        direction: Direction,
        element: TypeId,
    },
    Named {
        origin: OriginId,
        arguments: InternArgs,
    },
    Alias {
        origin: OriginId,
        arguments: InternArgs,
    },
    Tuple(InternArgs),
}

pub(super) struct ArenaPool<'a> {
    pub(super) is_local: bool,
    pub(super) epoch: u16,
    pub(super) node_limit: usize,
    pub(super) byte_limit: usize,
    pub(super) interner: HashMap<InternKey, TypeId>,
    pub(super) nodes: Vec<Ty>,
    pub(super) arguments: Vec<TypeId>,
    pub(super) origins: Vec<NamedOriginData<'a>>,
    pub(super) structs: Vec<StructData<'a>>,
    pub(super) interfaces: Vec<InterfaceData<'a>>,
    pub(super) signatures: Vec<SignatureData>,
    pub(super) type_params: Vec<TypeParamData<'a>>,
    pub(super) param_names: String,
    pub(super) formal_params: HashMap<(&'a Path, u32), TypeId>,
    pub(super) fields: Vec<Field>,
    pub(super) field_bytes: usize,
    pub(super) methods: Vec<Method<'a>>,
    pub(super) method_bytes: usize,
    pub(super) terms: Vec<Term>,
    pub(super) interface_sets: HashMap<TypeId, super::sets::TypeSet>,
    pub(super) signature_names: Vec<SignatureNames>,
    pub(super) signature_name_bytes: String,
    pub(super) expansions: HashMap<TypeId, TypeId>,
    pub(super) named_checks: HashMap<TypeId, NamedCheckState>,
}

impl<'a> ArenaPool<'a> {
    pub(super) fn new(is_local: bool, epoch: u16, node_limit: usize, byte_limit: usize) -> Self {
        Self {
            is_local,
            epoch,
            node_limit,
            byte_limit,
            interner: HashMap::new(),
            nodes: Vec::new(),
            arguments: Vec::new(),
            origins: Vec::new(),
            structs: Vec::new(),
            interfaces: Vec::new(),
            signatures: Vec::new(),
            type_params: Vec::new(),
            param_names: String::new(),
            formal_params: HashMap::new(),
            fields: Vec::new(),
            field_bytes: 0,
            methods: Vec::new(),
            method_bytes: 0,
            terms: Vec::new(),
            interface_sets: HashMap::new(),
            signature_names: Vec::new(),
            signature_name_bytes: String::new(),
            expansions: HashMap::new(),
            named_checks: HashMap::new(),
        }
    }

    fn make_id(&self, index: usize) -> Option<TypeId> {
        let index = u16::try_from(index).ok()?;
        Some(if self.is_local {
            TypeId::local(self.epoch, index)
        } else {
            TypeId::decl(index)
        })
    }

    fn make_origin_id(&self, index: usize) -> Option<OriginId> {
        let index = u32::try_from(index).ok()?;
        Some(if self.is_local {
            OriginId::local(index)
        } else {
            OriginId::decl(index)
        })
    }

    pub(super) fn push_raw(&mut self, ty: Ty) -> Option<TypeId> {
        if self.nodes.len() >= self.node_limit {
            return None;
        }
        let id = self.make_id(self.nodes.len())?;
        self.nodes.push(ty);
        Some(id)
    }

    pub(super) fn push_interned(&mut self, ty: Ty) -> Option<TypeId> {
        let key = match ty {
            Ty::Invalid => return Some(TypeId::INVALID),
            Ty::Basic(kind) => return Some(TypeId::basic(kind)),
            Ty::Pointer(element) => Some(InternKey::Pointer(element)),
            Ty::Slice(element) => Some(InternKey::Slice(element)),
            Ty::Array { length, element } => Some(InternKey::Array { length, element }),
            Ty::Map { key, value } => Some(InternKey::Map { key, value }),
            Ty::Chan { direction, element } => Some(InternKey::Chan { direction, element }),
            Ty::Tuple(elements) => Some(InternKey::Tuple(InternArgs::from_slice(
                &self.arguments[elements.range()],
            ))),
            Ty::Named { .. }
            | Ty::Alias { .. }
            | Ty::TypeParam(_)
            | Ty::Union(_)
            | Ty::Interface(_)
            | Ty::Signature(_)
            | Ty::Struct(_) => None,
        };
        if let Some(key) = key {
            if let Some(&id) = self.interner.get(&key) {
                if let Ty::Tuple(arguments) = ty
                    && arguments.range().end == self.arguments.len()
                    && !arguments.is_empty()
                {
                    self.arguments.truncate(arguments.start as usize);
                }
                return Some(id);
            }
            let id = self.push_raw(ty)?;
            self.interner.insert(key, id);
            return Some(id);
        }
        self.push_raw(ty)
    }

    pub(super) fn push_origin_data(&mut self, data: NamedOriginData<'a>) -> Option<OriginId> {
        if self.origins.len() >= self.node_limit {
            return None;
        }
        let idx = self.origins.len();
        let origin_id = self.make_origin_id(idx)?;
        self.origins.push(data);
        Some(origin_id)
    }

    pub(super) fn push_named_instance(
        &mut self,
        origin: OriginId,
        arguments: &[TypeId],
        alias: bool,
    ) -> Option<TypeId> {
        let intern_args = InternArgs::from_slice(arguments);
        let key = if alias {
            InternKey::Alias {
                origin,
                arguments: intern_args,
            }
        } else {
            InternKey::Named {
                origin,
                arguments: intern_args,
            }
        };
        if let Some(&id) = self.interner.get(&key) {
            return Some(id);
        }
        if self.arguments.len() + arguments.len() > self.node_limit {
            return None;
        }
        let start = self.arguments.len();
        self.arguments.extend_from_slice(arguments);
        let Some(slice) = SliceRef::from_range(start..self.arguments.len()) else {
            self.arguments.truncate(start);
            return None;
        };
        let ty = if alias {
            Ty::Alias {
                origin,
                arguments: slice,
            }
        } else {
            Ty::Named {
                origin,
                arguments: slice,
            }
        };
        let Some(id) = self.push_raw(ty) else {
            self.arguments.truncate(start);
            return None;
        };
        self.interner.insert(key, id);
        Some(id)
    }

    pub(super) fn push_origin_node(
        &mut self,
        data: NamedOriginData<'a>,
        alias: bool,
    ) -> Option<(OriginId, TypeId)> {
        if self.origins.len() >= self.node_limit {
            return None;
        }
        let idx = self.origins.len();
        let origin_id = self.make_origin_id(idx)?;
        self.origins.push(data);
        let arguments = SliceRef::default();
        let ty = if alias {
            Ty::Alias {
                origin: origin_id,
                arguments,
            }
        } else {
            Ty::Named {
                origin: origin_id,
                arguments,
            }
        };
        let Some(id) = self.push_raw(ty) else {
            self.origins.truncate(idx);
            return None;
        };
        Some((origin_id, id))
    }

    pub(super) fn push_type_param_data(
        &mut self,
        origin: Option<&'a Path>,
        selection: u32,
        index: u16,
        name: &str,
        ready: bool,
        synthetic: bool,
    ) -> Option<TypeId> {
        if self.type_params.len() >= self.node_limit
            || self.param_names.len().checked_add(name.len())? > self.byte_limit
        {
            return None;
        }
        let name_start = u32::try_from(self.param_names.len()).ok()?;
        let data = TypeParamData::new(
            origin,
            selection,
            index,
            name_start,
            name.len(),
            ready,
            synthetic,
        )?;
        let idx = self.type_params.len();
        let param_id = TypeParamId(u32::try_from(idx).ok()?);
        self.param_names.push_str(name);
        self.type_params.push(data);
        let Some(id) = self.push_raw(Ty::TypeParam(param_id)) else {
            self.type_params.truncate(idx);
            self.param_names.truncate(name_start as usize);
            return None;
        };
        Some(id)
    }

    pub(super) fn push_tuple(&mut self, elements: &[TypeId]) -> Option<TypeId> {
        let key = InternKey::Tuple(InternArgs::from_slice(elements));
        if let Some(&id) = self.interner.get(&key) {
            return Some(id);
        }
        if self.arguments.len() + elements.len() > self.node_limit {
            return None;
        }
        let start = self.arguments.len();
        self.arguments.extend_from_slice(elements);
        let slice = SliceRef::from_range(start..self.arguments.len())?;
        let Some(id) = self.push_raw(Ty::Tuple(slice)) else {
            self.arguments.truncate(start);
            return None;
        };
        self.interner.insert(key, id);
        Some(id)
    }

    pub(super) fn retain_above(&mut self, node_limit: usize) {
        self.interner.retain(|_, id| id.index() < node_limit);
        self.expansions.retain(|k, v| {
            (!k.is_local() || k.index() < node_limit) && (!v.is_local() || v.index() < node_limit)
        });
        self.named_checks
            .retain(|k, _| !k.is_local() || k.index() < node_limit);
        self.interface_sets
            .retain(|k, _| !k.is_local() || k.index() < node_limit);
        self.formal_params.retain(|_, id| id.index() < node_limit);
    }
}
