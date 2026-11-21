//! Shared `NamedOriginData` loading and lazy generic instantiation by pure `TypeId` substitution.
use super::constraints;
use super::ty::{
    NamedCheckState, NamedState, ORIGIN_FLAG_BOUNDS_LOADED, ORIGIN_FLAG_BOUNDS_LOADING,
    ORIGIN_FLAG_RHS_CONCRETE, ORIGIN_FLAG_TPARAMS_LOADED, ORIGIN_FLAG_TPARAMS_LOADING, OriginId,
    SliceRef, TypeOrigin,
};
use super::{CheckOutcome, MAX_DEPTH, Ty, TypeArena, TypeId, UnknownReason};
use crate::analysis::engine::obj::DeclId;

pub(super) fn parse_type_parameters_of(
    source: &str,
    generic: bool,
) -> Option<(&str, crate::syntax::Signature)> {
    match crate::syntax::type_parameter_end(source) {
        Some(end) => {
            let slice = source.get(..end)?;
            Some((slice, crate::syntax::parse_type_parameters(slice)))
        }
        None if !generic => Some(("", crate::syntax::Signature::default())),
        None => None,
    }
}

impl<'a> TypeArena<'a> {
    pub(super) fn type_parameter_source(&self, declaration: DeclId<'_>) -> Option<String> {
        if !declaration.symbol.generic {
            return Some(String::new());
        }
        let start = declaration.symbol.end;
        let length = declaration
            .symbol
            .type_info
            .as_ref()
            .map_or(super::MAX_BYTES, |info| usize::from(info.parameter_end));
        if length == 0 || length > super::MAX_BYTES {
            return None;
        }
        let end = declaration
            .symbol
            .declaration()?
            .end
            .min(start.checked_add(length)?);
        self.workspace.source_slice(declaration.file, start..end)
    }

    /// Load a package declaration's type parameters ONCE into `DeclArena` (and
    /// optionally resolve their constraint bounds). Subsequent instantiations
    /// reuse the cached `tparams` with zero disk I/O and zero re-parsing.
    pub(super) fn ensure_origin_tparams(
        &mut self,
        origin_id: OriginId,
        load_bounds: bool,
    ) -> Option<Vec<TypeId>> {
        if origin_id.is_local() {
            return Some(Vec::new());
        }
        let (decl, tparams_loaded, tparams_loading, bounds_loaded, bounds_loading, tparams_slice) = {
            let data = self.origin_data(origin_id)?;
            let TypeOrigin::Package(decl) = data.origin else {
                return Some(Vec::new());
            };
            (
                decl,
                data.tparams_loaded(),
                data.tparams_loading(),
                data.bounds_loaded(),
                data.bounds_loading(),
                data.tparams,
            )
        };
        if !decl.symbol.generic {
            if let Some(data) = self.origin_data_mut(origin_id) {
                data.flags |= ORIGIN_FLAG_TPARAMS_LOADED | ORIGIN_FLAG_BOUNDS_LOADED;
            }
            return Some(Vec::new());
        }
        if tparams_loaded {
            let tparams = self.decl.pool.arguments[tparams_slice.range()].to_vec();
            if !load_bounds || bounds_loaded || bounds_loading {
                return Some(tparams);
            }
            return self
                .load_origin_bounds(origin_id, decl, &tparams)
                .map(|()| tparams);
        }
        if tparams_loading {
            return None;
        }
        self.origin_data_mut(origin_id)?.flags |= ORIGIN_FLAG_TPARAMS_LOADING;
        let result = self.load_origin_tparams_uncached(origin_id, decl, load_bounds);
        if let Some(data) = self.origin_data_mut(origin_id) {
            data.flags &= !ORIGIN_FLAG_TPARAMS_LOADING;
        }
        result
    }

    fn load_origin_tparams_uncached(
        &mut self,
        origin_id: OriginId,
        decl: DeclId<'a>,
        load_bounds: bool,
    ) -> Option<Vec<TypeId>> {
        let source = self.type_parameter_source(decl)?;
        let (header, parsed) = parse_type_parameters_of(&source, true)?;
        if !parsed.diagnostics.is_empty() {
            return None;
        }
        let names = constraints::parameter_names(header, &parsed)?;
        if names.len() > self.decl.pool.node_limit {
            return None;
        }
        let tparams = self.with_decl_alloc(|store| {
            let mut params = Vec::with_capacity(names.len());
            for (index, (param_ast, name)) in parsed.type_parameters.iter().zip(&names).enumerate()
            {
                let idx = u16::try_from(index).ok()?;
                let sel = param_ast
                    .name
                    .as_ref()
                    .and_then(|r| decl.symbol.end.checked_add(r.start))
                    .and_then(|off| u32::try_from(off).ok())
                    .unwrap_or(u32::MAX);
                let tp = store.push_named_type_param(decl.file, sel, idx, name, true)?;
                if !tp.is_decl_safe() {
                    return None;
                }
                params.push(tp);
            }
            if store.decl.pool.arguments.len() + params.len() > store.decl.pool.node_limit {
                return None;
            }
            let start = store.decl.pool.arguments.len();
            store.decl.pool.arguments.extend_from_slice(&params);
            let slice = SliceRef::from_range(start..store.decl.pool.arguments.len())?;
            let data = store.origin_data_mut(origin_id)?;
            data.tparams = slice;
            data.flags |= ORIGIN_FLAG_TPARAMS_LOADED;
            Some(params)
        })?;
        // Opportunistically resolve bounds while `header` and `parsed` are in hand.
        let bounds_ok =
            self.populate_origin_bounds(origin_id, decl, header, &parsed, &names, &tparams);
        if load_bounds && !bounds_ok {
            return None;
        }
        Some(tparams)
    }

    fn load_origin_bounds(
        &mut self,
        origin_id: OriginId,
        decl: DeclId<'a>,
        tparams: &[TypeId],
    ) -> Option<()> {
        let source = self.type_parameter_source(decl)?;
        let (header, parsed) = parse_type_parameters_of(&source, true)?;
        if !parsed.diagnostics.is_empty() {
            return None;
        }
        let names = constraints::parameter_names(header, &parsed)?;
        if names.len() != tparams.len() {
            return None;
        }
        self.populate_origin_bounds(origin_id, decl, header, &parsed, &names, tparams)
            .then_some(())
    }

    fn populate_origin_bounds(
        &mut self,
        origin_id: OriginId,
        decl: DeclId<'a>,
        header: &str,
        parsed: &crate::syntax::Signature,
        names: &[&str],
        tparams: &[TypeId],
    ) -> bool {
        if let Some(data) = self.origin_data_mut(origin_id) {
            if data.bounds_loaded() {
                return true;
            }
            if data.bounds_loading() {
                return false;
            }
            data.flags |= ORIGIN_FLAG_BOUNDS_LOADING;
        } else {
            return false;
        }
        let environment: Vec<(&str, TypeId)> =
            names.iter().copied().zip(tparams.iter().copied()).collect();
        let ok = self.with_decl_alloc(|store| {
            let mut bounds = Vec::with_capacity(tparams.len());
            for param_ast in &parsed.type_parameters {
                let ann = &header[param_ast.annotation.clone()];
                let bound = match store.constraint_checked(decl.file, ann, &environment) {
                    CheckOutcome::Ok(bound) if bound.is_decl_safe() => bound,
                    _ => return false,
                };
                bounds.push(bound);
            }
            for (&tp, &bound) in tparams.iter().zip(&bounds) {
                if !store.set_type_param_bound(tp, bound) {
                    return false;
                }
            }
            for &bound in &bounds {
                if !matches!(
                    store.constraint_type_set_checked(bound, 0),
                    CheckOutcome::Ok(_)
                ) {
                    for &tp in tparams {
                        store.set_type_param_bound(tp, TypeId::INVALID);
                    }
                    return false;
                }
            }
            true
        });
        if let Some(data) = self.origin_data_mut(origin_id) {
            data.flags &= !ORIGIN_FLAG_BOUNDS_LOADING;
            if ok {
                data.flags |= ORIGIN_FLAG_BOUNDS_LOADED;
            }
        }
        ok
    }

    /// Load a package declaration's uninstantiated RHS template ONCE into `DeclArena`.
    pub(super) fn ensure_origin_rhs(
        &mut self,
        origin_id: OriginId,
    ) -> Option<(Vec<TypeId>, TypeId)> {
        if origin_id.is_local() {
            return None;
        }
        let (decl, state, rhs_concrete, rhs, tparams_slice) = {
            let data = self.origin_data(origin_id)?;
            let TypeOrigin::Package(decl) = data.origin else {
                return None;
            };
            (
                decl,
                data.state,
                data.rhs_concrete(),
                data.rhs_type(),
                data.tparams,
            )
        };
        match state {
            NamedState::Loaded if self.mode.is_symbolic() || rhs_concrete => {
                let tparams = self.decl.pool.arguments[tparams_slice.range()].to_vec();
                return Some((tparams, rhs?));
            }
            NamedState::Loading | NamedState::Failed => return None,
            NamedState::Unloaded | NamedState::Loaded => {}
        }
        let tparams = self.ensure_origin_tparams(origin_id, !self.mode.is_symbolic())?;
        let param_names: Vec<String> = tparams
            .iter()
            .map(|&tp| self.type_param_name(tp).map(str::to_owned))
            .collect::<Option<_>>()?;
        let (file, symbol) = (decl.file, decl.symbol);
        let end = symbol.declaration()?.end;
        let (body, body_offset) = if end - symbol.end <= super::MAX_BYTES {
            let source = self.workspace.source_slice(file, symbol.end..end)?;
            let body = super::declaration_body(&source)?;
            let offset = symbol.end + source.len() - body.len();
            (body.to_owned(), offset)
        } else {
            let offset = symbol
                .end
                .checked_add(symbol.type_info.as_ref()?.target.as_ref()?.body_start as usize)?;
            (self.workspace.source_slice(file, offset..end)?, offset)
        };
        let prev_state = self.origin_data(origin_id)?.state;
        self.origin_data_mut(origin_id)?.state = NamedState::Loading;
        let schema: Vec<(&str, TypeId)> = param_names
            .iter()
            .map(String::as_str)
            .zip(tparams.iter().copied())
            .collect();
        let symbolic = self.mode.is_symbolic();
        let outcome = self.with_decl_alloc(|store| {
            store.parse_in_checked(file, &body, Some(body_offset), &schema)
        });
        match outcome {
            CheckOutcome::Ok(rhs_ty) => {
                if !symbol.alias && matches!(self.node(rhs_ty), Some(Ty::TypeParam(_))) {
                    if let Some(data) = self.origin_data_mut(origin_id) {
                        data.state = NamedState::Failed;
                    }
                    return None;
                }
                if !rhs_ty.is_decl_safe() {
                    if let Some(data) = self.origin_data_mut(origin_id) {
                        data.state = prev_state;
                    }
                    return None;
                }
                if tparams.is_empty()
                    && let Some(methods) =
                        self.interface_data(rhs_ty).map(|data| data.methods.clone())
                    && let Some(decl_self) =
                        self.with_decl_alloc(|store| store.push_named(decl, &[], decl.symbol.alias))
                {
                    for method in self.methods_slice_mut(rhs_ty, methods) {
                        method.declared_on.get_or_insert(decl_self);
                    }
                }
                if let Some(data) = self.origin_data_mut(origin_id) {
                    data.rhs = rhs_ty;
                    data.state = NamedState::Loaded;
                    if !symbolic {
                        data.flags |= ORIGIN_FLAG_RHS_CONCRETE;
                    }
                }
                Some((tparams, rhs_ty))
            }
            CheckOutcome::Invalid(()) => {
                if let Some(data) = self.origin_data_mut(origin_id) {
                    data.state = if !symbolic {
                        NamedState::Failed
                    } else {
                        prev_state
                    };
                }
                None
            }
            CheckOutcome::Unknown(_) => {
                if let Some(data) = self.origin_data_mut(origin_id) {
                    data.state = prev_state;
                }
                None
            }
        }
    }

    pub(crate) fn check_named_arguments(&mut self, input: TypeId) -> Option<()> {
        if self.named_check_depth >= MAX_DEPTH {
            return None;
        }
        self.named_check_depth += 1;
        let result = self.check_named_arguments_inner(input);
        self.named_check_depth -= 1;
        result
    }

    fn check_named_arguments_inner(&mut self, input: TypeId) -> Option<()> {
        self.step()?;
        let Some((origin_id, declaration, arguments, _)) = self.named_origin_info(input) else {
            return Some(());
        };
        if arguments.is_empty() && !declaration.symbol.generic {
            return Some(());
        }
        match self.named_check_state(input) {
            Some(NamedCheckState::Valid) => return Some(()),
            Some(NamedCheckState::Invalid | NamedCheckState::Pending) => return None,
            None => {}
        }
        // Pending checks break cycles; failed non-invalid checks are removed so
        // interdependent parameter-bound passes can retry once earlier bounds land.
        if !self.named_check_set(input, NamedCheckState::Pending) {
            return None;
        }
        match self.check_named_arguments_step(input, origin_id, arguments) {
            CheckOutcome::Ok(()) => {
                self.named_check_set(input, NamedCheckState::Valid);
                Some(())
            }
            CheckOutcome::Invalid(()) => {
                self.named_check_set(input, NamedCheckState::Invalid);
                None
            }
            CheckOutcome::Unknown(_) => {
                self.named_check_remove(input);
                None
            }
        }
    }

    fn check_named_arguments_step(
        &mut self,
        input: TypeId,
        origin_id: OriginId,
        arguments: SliceRef,
    ) -> CheckOutcome<()> {
        let arguments = self.arguments_slice(input, arguments).to_vec();
        let load_bounds = !self.mode.is_symbolic();
        let Some(tparams) = self.ensure_origin_tparams(origin_id, load_bounds) else {
            return CheckOutcome::Unknown(UnknownReason::MissingMetadata);
        };
        if tparams.len() != arguments.len() || tparams.len() > self.active_node_limit() {
            return CheckOutcome::Unknown(UnknownReason::BudgetExceeded);
        }
        if self.mode.is_symbolic() {
            return CheckOutcome::Ok(());
        }
        let Some(bounds): Option<Vec<TypeId>> = tparams
            .iter()
            .map(|&tp| self.type_param_bound(tp))
            .collect()
        else {
            return CheckOutcome::Unknown(UnknownReason::MissingMetadata);
        };
        let bindings: Vec<Option<TypeId>> = arguments.iter().copied().map(Some).collect();
        for (bound, argument) in bounds.into_iter().zip(arguments) {
            let Some(inst_bound) = self.substitute_for_params(bound, &tparams, &bindings, 0) else {
                return CheckOutcome::Unknown(UnknownReason::BudgetExceeded);
            };
            if self.prove_constraint(inst_bound, argument, 0).is_none() {
                return CheckOutcome::Unknown(UnknownReason::Unsupported);
            }
        }
        CheckOutcome::Ok(())
    }

    /// Recursively verify generic type arity and constraints within a type node.
    pub(crate) fn validate_type(&mut self, input: TypeId) -> Option<()> {
        self.validate_type_inner(input, 0)
    }

    fn validate_type_inner(&mut self, input: TypeId, depth: usize) -> Option<()> {
        self.step()?;
        if depth >= MAX_DEPTH {
            return None;
        }
        match *self.node(input)? {
            Ty::Invalid => None,
            Ty::Alias { .. } => {
                let target = self.unalias(input)?;
                self.validate_type_inner(target, depth + 1)
            }
            Ty::Named { arguments, .. } => {
                if self.named_info(input).is_some() {
                    let args = self.arguments_slice(input, arguments).to_vec();
                    self.check_named_arguments(input)?;
                    for arg in args {
                        self.validate_type_inner(arg, depth + 1)?;
                    }
                }
                Some(())
            }
            Ty::Pointer(element)
            | Ty::Slice(element)
            | Ty::Array { element, .. }
            | Ty::Chan { element, .. } => self.validate_type_inner(element, depth + 1),
            Ty::Map { key, value } => {
                self.validate_type_inner(key, depth + 1)?;
                self.validate_type_inner(value, depth + 1)
            }
            Ty::Struct(_) => {
                let fields = self.struct_data(input)?.fields.clone();
                let field_tys: Vec<TypeId> = self
                    .fields_slice(input, fields)?
                    .iter()
                    .map(|field| field.ty)
                    .collect();
                for field_ty in field_tys {
                    self.validate_type_inner(field_ty, depth + 1)?;
                }
                Some(())
            }
            Ty::Interface(_) => {
                let data = self.interface_data(input)?.clone();
                let method_sigs: Vec<TypeId> = self
                    .methods_slice(input, data.methods)
                    .iter()
                    .map(|method| method.signature)
                    .collect();
                for method_sig in method_sigs {
                    self.validate_type_inner(method_sig, depth + 1)?;
                }
                let embeddeds = self.arguments_slice(input, data.embeddeds).to_vec();
                for emb in embeddeds {
                    self.validate_type_inner(emb, depth + 1)?;
                }
                Some(())
            }
            Ty::Union(slice) => {
                let term_tys: Vec<TypeId> = self
                    .terms_slice(input, slice)
                    .iter()
                    .map(|t| t.ty)
                    .collect();
                for ty in term_tys {
                    self.validate_type_inner(ty, depth + 1)?;
                }
                Some(())
            }
            Ty::Signature(_) => {
                let sig = self.signature_data(input)?.clone();
                let params = self.arg_range(input, sig.parameters).to_vec();
                let res = self.arg_range(input, sig.results).to_vec();
                for param in params {
                    self.validate_type_inner(param, depth + 1)?;
                }
                for result in res {
                    self.validate_type_inner(result, depth + 1)?;
                }
                Some(())
            }
            Ty::Tuple(elements) => {
                let elems = self.arguments_slice(input, elements).to_vec();
                for elem in elems {
                    self.validate_type_inner(elem, depth + 1)?;
                }
                Some(())
            }
            Ty::Basic(_) | Ty::TypeParam(_) => Some(()),
        }
    }

    /// Instantiate a package declaration's cached RHS template by pure `TypeId`
    /// substitution without checking argument bounds (used by `expand` after
    /// `check_named_arguments` and by `TypeSet` resolution for self-referential
    /// generic interface constraints).
    pub(super) fn expand_origin_rhs(&mut self, input: TypeId) -> Option<TypeId> {
        let Some((origin_id, declaration, arguments, _)) = self.named_origin_info(input) else {
            return Some(input);
        };
        let arguments = self.arguments_slice(input, arguments).to_vec();
        let (tparams, rhs) = self.ensure_origin_rhs(origin_id)?;
        if tparams.len() != arguments.len() {
            return None;
        }
        if (!self.mode.is_symbolic() || !declaration.symbol.alias)
            && matches!(self.node(rhs)?, Ty::TypeParam(_))
        {
            return None;
        }
        let result = if tparams.is_empty() {
            rhs
        } else {
            let bindings: Vec<_> = arguments.iter().copied().map(Some).collect();
            if input.is_decl() {
                self.with_decl_alloc(|store| {
                    store.substitute_for_params(rhs, &tparams, &bindings, 0)
                })?
            } else {
                self.with_local_alloc(|store| {
                    store.substitute_for_params(rhs, &tparams, &bindings, 0)
                })?
            }
        };
        if (result.is_local() || input.is_decl_safe())
            && let Some(methods) = self.interface_data(result).map(|data| data.methods.clone())
        {
            for method in self.methods_slice_mut(result, methods) {
                method.declared_on.get_or_insert(input);
            }
        }
        Some(result)
    }

    /// Follow named/aliased interface declarations to their underlying `Ty::Interface`
    /// via `expand_origin_rhs` without re-entering `check_named_arguments`.
    pub(super) fn underlying_interface_unchecked(&mut self, mut input: TypeId) -> Option<TypeId> {
        for _ in 0..MAX_DEPTH {
            if input == TypeId::INVALID {
                return None;
            }
            if matches!(self.node(input)?, Ty::Interface(_)) {
                return Some(input);
            }
            if let Some(interface) = self.error_interface(input) {
                return Some(interface);
            }
            if self.local_origin_id(input).is_some() {
                input = self.local_declaration_underlying(input)?;
                continue;
            }
            if self.named_info(input).is_some() {
                if self.expansion_depth >= MAX_DEPTH {
                    return None;
                }
                self.expansion_depth += 1;
                let next = self.expand_origin_rhs(input);
                self.expansion_depth -= 1;
                input = next?;
                continue;
            }
            return None;
        }
        None
    }

    pub(super) fn expand(&mut self, input: TypeId) -> Option<TypeId> {
        self.step()?;
        if let Some(target) = self.expansion_get(input) {
            return Some(target);
        }
        // Constraints may recursively ask for another declaration's underlying
        // type. Bound that call stack as well as the iterative alias chain.
        if self.expansion_depth >= MAX_DEPTH {
            return None;
        }
        self.expansion_depth += 1;
        let result = if input.is_decl() {
            self.with_decl_alloc(|store| store.expand_declaration(input))
        } else {
            self.with_local_alloc(|store| store.expand_declaration(input))
        };
        self.expansion_depth -= 1;
        result
    }

    fn expand_declaration(&mut self, input: TypeId) -> Option<TypeId> {
        if self.named_info(input).is_none() {
            return Some(input);
        }
        self.check_named_arguments(input)?;
        let result = self.expand_origin_rhs(input)?;
        self.expansion_insert(input, result);
        Some(result)
    }

    pub(crate) fn underlying(&mut self, mut input: TypeId) -> Option<TypeId> {
        for _ in 0..MAX_DEPTH {
            if input == TypeId::INVALID {
                return None;
            }
            if self.local_origin_id(input).is_some() {
                input = self.local_declaration_underlying(input)?;
                continue;
            }
            if let Some(interface) = self.error_interface(input) {
                return Some(interface);
            }
            if self.named_info(input).is_none() {
                return Some(input);
            }
            input = self.expand(input)?;
        }
        None
    }

    pub(crate) fn unalias(&mut self, mut input: TypeId) -> Option<TypeId> {
        for _ in 0..MAX_DEPTH {
            if input == TypeId::INVALID {
                return None;
            }
            if !matches!(self.node(input)?, Ty::Alias { .. }) {
                return Some(input);
            }
            if let Some(expanded) = self.local_alias(input)? {
                input = expanded;
                continue;
            }
            input = self.expand(input)?;
        }
        None
    }

    pub(crate) fn named_declaration(
        &self,
        input: TypeId,
    ) -> Option<(DeclId<'a>, Vec<TypeId>, bool)> {
        let (declaration, arguments, alias) = self.named_info(input)?;
        Some((
            declaration,
            self.arguments_slice(input, arguments).to_vec(),
            alias,
        ))
    }
}
