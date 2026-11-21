//! Unified bounded type unifier (`unify.go`) supporting union-find type parameter
//! equivalence classes (`P <-> Q`), occurs check, substitution, and schema matching.
use super::predicates::TypePairs;
use super::{MAX_DEPTH, MAX_NODES, Tags, Ty, TypeId, TypeStore};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum UnifyMode {
    /// Exact structural unification (nested arguments).
    Exact,
    /// Outer assignment unification for call inference (depth == 0 unwrapping + interfaces).
    Assign,
    /// Potential method-signature equivalence for implementation search (`gopls` schema match).
    Schema,
}

#[derive(Clone)]
struct Handle {
    parent: usize,
    bound: Option<TypeId>,
}

#[derive(Clone)]
pub(super) struct UnifierState {
    handles: Vec<Handle>,
    indices: HashMap<TypeId, usize>,
}

pub(super) struct Unifier<'u, 'a> {
    store: &'u mut TypeStore<'a>,
    mode: UnifyMode,
    handles: Vec<Handle>,
    indices: HashMap<TypeId, usize>,
}

impl<'u, 'a> Unifier<'u, 'a> {
    pub(super) fn new(store: &'u mut TypeStore<'a>, mode: UnifyMode) -> Self {
        Self {
            store,
            mode,
            handles: Vec::new(),
            indices: HashMap::new(),
        }
    }

    pub(super) fn with_params(
        store: &'u mut TypeStore<'a>,
        mode: UnifyMode,
        params: &[TypeId],
    ) -> Option<Self> {
        let mut unifier = Self::new(store, mode);
        for &param in params {
            unifier.ensure_handle(param)?;
        }
        Some(unifier)
    }

    pub(super) fn store(&self) -> &TypeStore<'a> {
        self.store
    }

    pub(super) fn store_mut(&mut self) -> &mut TypeStore<'a> {
        self.store
    }

    pub(super) fn save_state(&self) -> UnifierState {
        UnifierState {
            handles: self.handles.clone(),
            indices: self.indices.clone(),
        }
    }

    pub(super) fn restore_state(&mut self, state: UnifierState) {
        self.handles = state.handles;
        self.indices = state.indices;
    }

    fn ensure_handle(&mut self, param: TypeId) -> Option<usize> {
        if let Some(&idx) = self.indices.get(&param) {
            return Some(idx);
        }
        if self.handles.len() >= 64 {
            return None;
        }
        let idx = self.handles.len();
        self.handles.push(Handle {
            parent: idx,
            bound: None,
        });
        self.indices.insert(param, idx);
        Some(idx)
    }

    fn find_root(&mut self, mut idx: usize) -> usize {
        while self.handles[idx].parent != idx {
            let parent = self.handles[idx].parent;
            let grand = self.handles[parent].parent;
            self.handles[idx].parent = grand;
            idx = parent;
        }
        idx
    }

    pub(super) fn bind_param(&mut self, param: TypeId, target: TypeId) -> Option<()> {
        let idx = self.ensure_handle(param)?;
        let root = self.find_root(idx);
        self.handles[root].bound = Some(target);
        Some(())
    }

    pub(super) fn resolve(&mut self, mut input: TypeId) -> Option<TypeId> {
        for _ in 0..MAX_DEPTH {
            self.store.step()?;
            input = self.store.unalias(input)?;
            let Some(&idx) = self.indices.get(&input) else {
                return Some(input);
            };
            let root = self.find_root(idx);
            match self.handles[root].bound {
                Some(bound) if bound != input => input = bound,
                _ => return Some(input),
            }
        }
        None
    }

    pub(super) fn snapshot(&mut self, params: &[TypeId]) -> Vec<Option<TypeId>> {
        params
            .iter()
            .map(|&param| {
                let idx = *self.indices.get(&param)?;
                let root = self.find_root(idx);
                self.handles[root].bound
            })
            .collect()
    }

    fn occurs_in(&mut self, target_root: usize, input: TypeId) -> Option<bool> {
        let mut pending = vec![(input, 0usize)];
        let mut seen = HashSet::new();
        while let Some((ty, depth)) = pending.pop() {
            self.store.step()?;
            if depth >= MAX_DEPTH {
                return None;
            }
            let ty = self.resolve(ty)?;
            if let Some(&idx) = self.indices.get(&ty)
                && self.find_root(idx) == target_root
            {
                return Some(true);
            }
            if !seen.insert(ty) {
                continue;
            }
            let TypePairs::Children(children) = self.store.identity_pairs(ty, ty, Tags::Include)?
            else {
                return None;
            };
            if pending.len() + children.len() > MAX_NODES {
                return None;
            }
            pending.extend(children.into_iter().map(|(child, _)| (child, depth + 1)));
        }
        Some(false)
    }

    /// Unify two types according to `self.mode`.
    /// Returns `Some(true)` on match, `Some(false)` on mismatch, `None` on budget exhaustion/unknown.
    pub(super) fn unify(&mut self, x: TypeId, y: TypeId, depth: usize) -> Option<bool> {
        self.store.step()?;
        if depth >= MAX_DEPTH {
            return None;
        }
        let x = self.resolve(x)?;
        let y = self.resolve(y)?;
        if x == y {
            return Some(true);
        }

        if self.mode == UnifyMode::Schema {
            return self.unify_schema_pair(x, y, depth);
        }
        self.unify_call_pair(x, y, depth)
    }

    fn unify_param_with_type(
        &mut self,
        param: TypeId,
        other: TypeId,
        depth: usize,
    ) -> Option<bool> {
        let p_idx = self.find_root(*self.indices.get(&param)?);
        if let Some(&o_idx) = self.indices.get(&other) {
            let o_root = self.find_root(o_idx);
            if p_idx == o_root {
                return Some(true);
            }
            let bound_p = self.handles[p_idx].bound;
            let bound_o = self.handles[o_root].bound;
            match (bound_p, bound_o) {
                (Some(bp), Some(bo)) => {
                    if !self.unify(bp, bo, depth + 1)? {
                        return Some(false);
                    }
                    let rp = self.find_root(p_idx);
                    let ro = self.find_root(o_root);
                    self.handles[ro].parent = rp;
                    return Some(true);
                }
                (Some(bp), None) => {
                    if self.occurs_in(o_root, bp)? {
                        return Some(false);
                    }
                    self.handles[o_root].parent = p_idx;
                    return Some(true);
                }
                (None, Some(bo)) => {
                    if self.occurs_in(p_idx, bo)? {
                        return Some(false);
                    }
                    self.handles[p_idx].parent = o_root;
                    return Some(true);
                }
                (None, None) => {
                    self.handles[o_root].parent = p_idx;
                    return Some(true);
                }
            }
        }
        if let Some(previous) = self.handles[p_idx].bound {
            return self.unify(previous, other, depth + 1);
        }
        if self.occurs_in(p_idx, other)? {
            return Some(false);
        }
        self.handles[p_idx].bound = Some(other);
        Some(true)
    }

    fn unify_schema_pair(&mut self, left: TypeId, right: TypeId, depth: usize) -> Option<bool> {
        let mut pending = vec![(left, right, depth)];
        while let Some((left, right, depth)) = pending.pop() {
            self.store.step()?;
            if depth >= MAX_DEPTH {
                return None;
            }
            let left = self.resolve(left)?;
            let right = self.resolve(right)?;
            if left == right {
                continue;
            }
            let parameter = match (*self.store.node(left)?, *self.store.node(right)?) {
                (Ty::TypeParam(_), _) => Some((left, right)),
                (_, Ty::TypeParam(_)) => Some((right, left)),
                _ => None,
            };
            if let Some((param, value)) = parameter {
                let p_idx = self.ensure_handle(param)?;
                let p_root = self.find_root(p_idx);
                if matches!(self.store.node(value)?, Ty::TypeParam(_)) {
                    let v_idx = self.ensure_handle(value)?;
                    let v_root = self.find_root(v_idx);
                    if p_root == v_root {
                        continue;
                    }
                    match (self.handles[p_root].bound, self.handles[v_root].bound) {
                        (Some(bp), Some(bv)) => {
                            pending.push((bp, bv, depth + 1));
                            self.handles[v_root].parent = p_root;
                            continue;
                        }
                        (Some(_), None) => {
                            self.handles[v_root].parent = p_root;
                            continue;
                        }
                        (None, _) => {
                            self.handles[p_root].parent = v_root;
                            continue;
                        }
                    }
                }
                if let Some(previous) = self.handles[p_root].bound {
                    pending.push((previous, value, depth + 1));
                    continue;
                }
                if self.occurs_in(p_root, value)? {
                    return Some(false);
                }
                self.handles[p_root].bound = Some(value);
                continue;
            }
            let TypePairs::Children(children) =
                self.store.identity_pairs(left, right, Tags::Include)?
            else {
                return Some(false);
            };
            if pending.len() + children.len() > MAX_NODES {
                return None;
            }
            pending.extend(
                children
                    .into_iter()
                    .map(|(left, right)| (left, right, depth + 1)),
            );
        }
        Some(true)
    }

    fn unify_call_pair(
        &mut self,
        mut formal: TypeId,
        mut actual: TypeId,
        depth: usize,
    ) -> Option<bool> {
        if self.indices.contains_key(&formal) {
            return self.unify_param_with_type(formal, actual, depth);
        }
        if self.indices.contains_key(&actual) {
            return self.unify_param_with_type(actual, formal, depth);
        }
        if depth == 0
            && self.mode == UnifyMode::Assign
            && self.store.interface_constructor(formal)?
        {
            let root = self.store.underlying(formal)?;
            let iface = self.store.interface_data(root)?.clone();
            if !iface.has_type_elements && !iface.comparable {
                let methods = self.store.methods_slice(root, iface.methods).to_vec();
                for method in methods {
                    let signature = self.store.lookup_method(actual, &method)??;
                    if !self.unify(method.signature, signature, depth + 1)? {
                        return Some(false);
                    }
                }
                return Some(true);
            }
        }
        // Assignment may compare an unnamed outer constructor with a named
        // argument's underlying type. Nested constructors require exact identity.
        if depth == 0
            && self.mode == UnifyMode::Assign
            && matches!(
                self.store.node(formal)?,
                Ty::Pointer(_)
                    | Ty::Slice(_)
                    | Ty::Array { .. }
                    | Ty::Map { .. }
                    | Ty::Chan { .. }
                    | Ty::Struct(_)
                    | Ty::Signature(_)
            )
        {
            actual = self.store.underlying(actual)?;
        }
        if depth == 0
            && self.mode == UnifyMode::Assign
            && matches!(self.store.node(formal)?, Ty::Named { .. })
            && matches!(
                self.store.node(actual)?,
                Ty::Pointer(_)
                    | Ty::Slice(_)
                    | Ty::Array { .. }
                    | Ty::Map { .. }
                    | Ty::Chan { .. }
                    | Ty::Struct(_)
                    | Ty::Signature(_)
            )
        {
            formal = self.store.underlying(formal)?;
        }
        if self.store.error_interface(formal).is_some()
            && self.store.error_interface(actual).is_some()
        {
            return Some(true);
        }
        let pairs = match (*self.store.node(formal)?, *self.store.node(actual)?) {
            (Ty::Interface(_), Ty::Interface(_)) => {
                let ld = self.store.interface_data(formal)?.clone();
                let rd = self.store.interface_data(actual)?.clone();
                if ld.comparable != rd.comparable
                    || ld.has_type_elements != rd.has_type_elements
                    || ld.embeddeds.len() != rd.embeddeds.len()
                    || !self.store.same_interface_methods(
                        formal,
                        ld.methods.clone(),
                        actual,
                        rd.methods.clone(),
                    )?
                {
                    return Some(false);
                }
                let mut pairs: Vec<(TypeId, TypeId)> = self
                    .store
                    .methods_slice(formal, ld.methods)
                    .iter()
                    .map(|m| m.signature)
                    .zip(
                        self.store
                            .methods_slice(actual, rd.methods)
                            .iter()
                            .map(|m| m.signature),
                    )
                    .collect();
                pairs.extend(
                    self.store
                        .arguments_slice(formal, ld.embeddeds)
                        .iter()
                        .copied()
                        .zip(
                            self.store
                                .arguments_slice(actual, rd.embeddeds)
                                .iter()
                                .copied(),
                        ),
                );
                pairs
            }
            (Ty::Union(a), Ty::Union(b)) => {
                let at = self.store.terms_slice(formal, a);
                let bt = self.store.terms_slice(actual, b);
                if at.len() != bt.len()
                    || at
                        .iter()
                        .zip(bt)
                        .any(|(x, y)| x.approximate != y.approximate)
                {
                    return Some(false);
                }
                at.iter()
                    .map(|x| x.ty)
                    .zip(bt.iter().map(|y| y.ty))
                    .collect()
            }
            (Ty::Pointer(a), Ty::Pointer(b)) | (Ty::Slice(a), Ty::Slice(b)) => vec![(a, b)],
            (Ty::Signature(_), Ty::Signature(_)) => {
                let fs = self.store.signature_data(formal)?.clone();
                let acs = self.store.signature_data(actual)?.clone();
                if fs.variadic != acs.variadic
                    || fs.parameters.len() != acs.parameters.len()
                    || fs.results.len() != acs.results.len()
                {
                    return Some(false);
                }
                self.store
                    .arg_range(formal, fs.parameters)
                    .iter()
                    .chain(self.store.arg_range(formal, fs.results))
                    .copied()
                    .zip(
                        self.store
                            .arg_range(actual, acs.parameters)
                            .iter()
                            .chain(self.store.arg_range(actual, acs.results))
                            .copied(),
                    )
                    .collect()
            }
            (Ty::Struct(_), Ty::Struct(_)) => {
                let fd = self.store.struct_data(formal)?.clone();
                let ad = self.store.struct_data(actual)?.clone();
                if !self.store.same_struct_fields(
                    (fd.origin, formal, fd.fields.clone()),
                    (ad.origin, actual, ad.fields.clone()),
                )? {
                    return Some(false);
                }
                self.store
                    .fields_slice(formal, fd.fields)?
                    .iter()
                    .map(|f| f.ty)
                    .zip(
                        self.store
                            .fields_slice(actual, ad.fields)?
                            .iter()
                            .map(|f| f.ty),
                    )
                    .collect()
            }
            (
                Ty::Array {
                    length: a,
                    element: ae,
                },
                Ty::Array {
                    length: b,
                    element: be,
                },
            ) if a == b => vec![(ae, be)],
            (Ty::Map { key: a, value: av }, Ty::Map { key: b, value: bv }) => {
                vec![(a, b), (av, bv)]
            }
            (
                Ty::Chan {
                    direction: a,
                    element: ae,
                },
                Ty::Chan {
                    direction: b,
                    element: be,
                },
            ) if a == b || (depth == 0 && b == super::Direction::Both) => vec![(ae, be)],
            (Ty::Named { .. }, Ty::Named { .. })
                if self.store.named_info(formal).is_some()
                    && self.store.named_info(actual).is_some() =>
            {
                let (a, aa, _) = self.store.named_info(formal)?;
                let (b, ba, _) = self.store.named_info(actual)?;
                if a.file != b.file || a.symbol.start != b.symbol.start || aa.len() != ba.len() {
                    return self.store.identical(formal, actual);
                }
                if a.symbol.generic && aa.is_empty() {
                    return None;
                }
                self.store
                    .arguments_slice(formal, aa)
                    .iter()
                    .copied()
                    .zip(self.store.arguments_slice(actual, ba).iter().copied())
                    .collect()
            }
            (Ty::Tuple(a), Ty::Tuple(b)) if a.len() == b.len() => self
                .store
                .arguments_slice(formal, a)
                .iter()
                .copied()
                .zip(self.store.arguments_slice(actual, b).iter().copied())
                .collect(),
            _ => return self.store.identical(formal, actual),
        };
        let saved = self.mode;
        if saved == UnifyMode::Assign {
            self.mode = UnifyMode::Exact;
        }
        let mut matched = true;
        for (a, b) in pairs {
            if !self.unify(a, b, depth + 1)? {
                matched = false;
                break;
            }
        }
        self.mode = saved;
        Some(matched)
    }

    pub(super) fn match_schema(
        store: &mut TypeStore<'a>,
        left: TypeId,
        right: TypeId,
    ) -> Option<bool> {
        Unifier::new(store, UnifyMode::Schema).unify(left, right, 0)
    }

    pub(super) fn substitute_for_params(
        store: &mut TypeStore<'a>,
        input: TypeId,
        tparams: &[TypeId],
        bindings: &[Option<TypeId>],
        depth: usize,
    ) -> Option<TypeId> {
        store.step()?;
        if depth >= MAX_DEPTH {
            return None;
        }
        if store.local_decl_info(input).is_some() || store.error_interface(input).is_some() {
            return Some(input);
        }
        let node = match *store.node(input)? {
            Ty::Invalid => return None,
            Ty::Union(slice) => {
                let terms = store.terms_slice(input, slice).to_vec();
                let mut substituted = Vec::with_capacity(terms.len());
                for mut term in terms {
                    term.ty =
                        Self::substitute_for_params(store, term.ty, tparams, bindings, depth + 1)?;
                    substituted.push(term);
                }
                return store.store_union(substituted);
            }
            Ty::TypeParam(_) => {
                if let Some(pos) = tparams.iter().position(|&tp| tp == input) {
                    return *bindings.get(pos)?;
                }
                return Some(input);
            }
            Ty::Basic(_) => return Some(input),
            Ty::Interface(_) => {
                let data = store.interface_data(input)?.clone();
                if data.builtin_any {
                    return Some(input);
                }
                let mut methods = store.methods_slice(input, data.methods).to_vec();
                for method in &mut methods {
                    method.signature = Self::substitute_for_params(
                        store,
                        method.signature,
                        tparams,
                        bindings,
                        depth + 1,
                    )?;
                    if let Some(receiver) = method.declared_on {
                        method.declared_on = Some(Self::substitute_for_params(
                            store,
                            receiver,
                            tparams,
                            bindings,
                            depth + 1,
                        )?);
                    }
                }
                let emb_list = store.arguments_slice(input, data.embeddeds).to_vec();
                let mut embeddeds = Vec::with_capacity(emb_list.len());
                for emb in emb_list {
                    embeddeds.push(Self::substitute_for_params(
                        store,
                        emb,
                        tparams,
                        bindings,
                        depth + 1,
                    )?);
                }
                return store
                    .store_unified_interface_checked(
                        data.origin,
                        methods,
                        embeddeds,
                        data.has_type_elements,
                        data.comparable,
                        data.implicit,
                    )
                    .ok();
            }
            Ty::Signature(_) => {
                let sig = store.signature_data(input)?.clone();
                let (tp_list, parameters, results) = (
                    store.arguments_slice(input, sig.type_params).to_vec(),
                    store.arg_range(input, sig.parameters).to_vec(),
                    store.arg_range(input, sig.results).to_vec(),
                );
                let parameters = parameters
                    .into_iter()
                    .map(|ty| Self::substitute_for_params(store, ty, tparams, bindings, depth + 1))
                    .collect::<Option<_>>()?;
                let results = results
                    .into_iter()
                    .map(|ty| Self::substitute_for_params(store, ty, tparams, bindings, depth + 1))
                    .collect::<Option<_>>()?;
                let ty =
                    store.store_generic_signature(tp_list, parameters, results, sig.variadic)?;
                store.copy_signature_names(input, ty, false);
                return Some(ty);
            }
            Ty::Struct(_) => {
                let data = store.struct_data(input)?.clone();
                let mut fields = store.fields_slice(input, data.fields)?.to_vec();
                for field in &mut fields {
                    field.ty =
                        Self::substitute_for_params(store, field.ty, tparams, bindings, depth + 1)?;
                }
                return store.store_struct(data.origin, fields);
            }
            Ty::Named { .. } => {
                let (declaration, arguments, _) = store.named_info(input)?;
                let arguments = store.arguments_slice(input, arguments).to_vec();
                let mut substituted = Vec::with_capacity(arguments.len());
                for argument in arguments {
                    substituted.push(Self::substitute_for_params(
                        store,
                        argument,
                        tparams,
                        bindings,
                        depth + 1,
                    )?);
                }
                return store.push_named(declaration, &substituted, false);
            }
            Ty::Alias { .. } => {
                let (declaration, arguments, _) = store.named_info(input)?;
                let arguments = store.arguments_slice(input, arguments).to_vec();
                let mut substituted = Vec::with_capacity(arguments.len());
                for argument in arguments {
                    substituted.push(Self::substitute_for_params(
                        store,
                        argument,
                        tparams,
                        bindings,
                        depth + 1,
                    )?);
                }
                return store.push_named(declaration, &substituted, true);
            }
            Ty::Tuple(elements) => {
                let elements = store.arguments_slice(input, elements).to_vec();
                let mut substituted = Vec::with_capacity(elements.len());
                for element in elements {
                    substituted.push(Self::substitute_for_params(
                        store,
                        element,
                        tparams,
                        bindings,
                        depth + 1,
                    )?);
                }
                return store.tuple(&substituted);
            }
            Ty::Pointer(element) => Ty::Pointer(Self::substitute_for_params(
                store,
                element,
                tparams,
                bindings,
                depth + 1,
            )?),
            Ty::Slice(element) => Ty::Slice(Self::substitute_for_params(
                store,
                element,
                tparams,
                bindings,
                depth + 1,
            )?),
            Ty::Array { length, element } => Ty::Array {
                length,
                element: Self::substitute_for_params(store, element, tparams, bindings, depth + 1)?,
            },
            Ty::Map { key, value } => Ty::Map {
                key: Self::substitute_for_params(store, key, tparams, bindings, depth + 1)?,
                value: Self::substitute_for_params(store, value, tparams, bindings, depth + 1)?,
            },
            Ty::Chan { direction, element } => Ty::Chan {
                direction,
                element: Self::substitute_for_params(store, element, tparams, bindings, depth + 1)?,
            },
        };
        store.push(node)
    }
}
