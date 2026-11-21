//! Bounded call inference: typed unification, untyped defaulting, substitution.
use super::unify::{Unifier, UnifyMode};
use super::{BasicKind, MAX_DEPTH, MAX_NODES, Operand, Ty, TypeArena, TypeId};
use std::path::Path;

pub(crate) fn completion_inferable(
    arena: &mut TypeArena<'_>,
    file: &Path,
    parameters: &[(&str, &str)],
    inputs: &[&str],
) -> Option<Vec<bool>> {
    arena.with_projection(true, |arena| {
        completion_inferable_inner(arena, file, parameters, inputs)
    })
}

fn completion_inferable_inner(
    arena: &mut TypeArena<'_>,
    file: &Path,
    parameters: &[(&str, &str)],
    inputs: &[&str],
) -> Option<Vec<bool>> {
    if parameters.len() > arena.local.pool.node_limit {
        return None;
    }
    let mut params = Vec::with_capacity(parameters.len());
    let mut environment = Vec::with_capacity(parameters.len());
    for (name, _) in parameters {
        let ty = arena.push_symbolic_parameter(file, name)?;
        params.push(ty);
        if *name != "_" {
            environment.push((*name, ty));
        }
    }
    let mut inferred = vec![false; params.len()];
    for input in inputs {
        let ty = arena.parse_in(file, input, &environment)?;
        collect_free_params(arena, ty, &params, &mut inferred, 0)?;
    }
    for _ in 0..=parameters.len() {
        let before = inferred.clone();
        for (index, (_, constraint)) in parameters.iter().enumerate() {
            if inferred[index] {
                let bound = arena.constraint(file, constraint, &environment)?;
                collect_free_params(arena, bound, &params, &mut inferred, 0)?;
            }
        }
        if before == inferred {
            return Some(inferred);
        }
    }
    None
}

fn collect_free_params(
    arena: &mut TypeArena<'_>,
    input: TypeId,
    params: &[TypeId],
    inferred: &mut [bool],
    depth: usize,
) -> Option<()> {
    arena.step()?;
    if depth >= MAX_DEPTH {
        return None;
    }
    let input = arena.unalias(input)?;
    match *arena.node(input)? {
        Ty::Invalid => None,
        Ty::TypeParam(_) => {
            if let Some(index) = params.iter().position(|&p| p == input)
                && let Some(slot) = inferred.get_mut(index)
            {
                *slot = true;
            }
            Some(())
        }
        Ty::Pointer(element)
        | Ty::Slice(element)
        | Ty::Array { element, .. }
        | Ty::Chan { element, .. } => {
            collect_free_params(arena, element, params, inferred, depth + 1)
        }
        Ty::Map { key, value } => {
            collect_free_params(arena, key, params, inferred, depth + 1)?;
            collect_free_params(arena, value, params, inferred, depth + 1)
        }
        Ty::Tuple(items) => {
            let items = arena.arguments_slice(input, items).to_vec();
            for item in items {
                collect_free_params(arena, item, params, inferred, depth + 1)?;
            }
            Some(())
        }
        Ty::Named { arguments, .. } => {
            let arguments = arena.arguments_slice(input, arguments).to_vec();
            for argument in arguments {
                collect_free_params(arena, argument, params, inferred, depth + 1)?;
            }
            Some(())
        }
        Ty::Signature(_) => {
            let sig = arena.signature_data(input)?.clone();
            let items = arena
                .arg_range(input, sig.parameters)
                .iter()
                .chain(arena.arg_range(input, sig.results))
                .copied()
                .collect::<Vec<_>>();
            for item in items {
                collect_free_params(arena, item, params, inferred, depth + 1)?;
            }
            Some(())
        }
        Ty::Struct(_) => {
            let fields = arena.struct_data(input)?.fields.clone();
            let field_tys: Vec<_> = arena
                .fields_slice(input, fields)?
                .iter()
                .map(|f| f.ty)
                .collect();
            for ty in field_tys {
                collect_free_params(arena, ty, params, inferred, depth + 1)?;
            }
            Some(())
        }
        Ty::Interface(_) => {
            let data = arena.interface_data(input)?.clone();
            let sigs: Vec<_> = arena
                .methods_slice(input, data.methods)
                .iter()
                .map(|m| m.signature)
                .collect();
            for sig in sigs {
                collect_free_params(arena, sig, params, inferred, depth + 1)?;
            }
            let embeddeds = arena.arguments_slice(input, data.embeddeds).to_vec();
            for emb in embeddeds {
                collect_free_params(arena, emb, params, inferred, depth + 1)?;
            }
            Some(())
        }
        Ty::Union(slice) => {
            let terms: Vec<_> = arena
                .terms_slice(input, slice)
                .iter()
                .map(|t| t.ty)
                .collect();
            for ty in terms {
                collect_free_params(arena, ty, params, inferred, depth + 1)?;
            }
            Some(())
        }
        Ty::Basic(_) | Ty::Alias { .. } => Some(()),
    }
}

impl TypeArena<'_> {
    /// Infer, prove, and instantiate the cached generic function signature once,
    /// avoiding re-lexing or re-parsing declaration headers at call sites.
    pub(crate) fn infer_and_instantiate_generic_signature(
        &mut self,
        generic_ty: TypeId,
        actuals: &[Operand],
        explicit: &[TypeId],
        int_bits: u32,
        spread: bool,
    ) -> Option<(Vec<TypeId>, TypeId)> {
        let sig = self.signature_data(generic_ty)?.clone();
        let params = self.arguments_slice(generic_ty, sig.type_params).to_vec();
        if params.is_empty()
            || params.len() > self.active_node_limit()
            || explicit.len() > params.len()
            || actuals.len() > MAX_NODES
        {
            return None;
        }
        let bounds: Vec<TypeId> = params
            .iter()
            .map(|&tp| self.type_param_bound(tp))
            .collect::<Option<_>>()?;
        let sig_params = self.arg_range(generic_ty, sig.parameters).to_vec();
        let variadic = sig.variadic;
        let fixed = sig_params.len().checked_sub(usize::from(variadic))?;
        if (!variadic && (spread || actuals.len() != fixed))
            || (variadic && (actuals.len() < fixed || (spread && actuals.len() != fixed + 1)))
        {
            return None;
        }
        let mut formals = Vec::with_capacity(actuals.len());
        for index in 0..actuals.len() {
            let mut ty = *sig_params.get(index.min(fixed))?;
            if variadic && index >= fixed && !spread {
                let Ty::Slice(element) = *self.node(ty)? else {
                    return None;
                };
                ty = element;
            }
            formals.push(ty);
        }
        let mut unifier = Unifier::with_params(self, UnifyMode::Assign, &params)?;
        for (&param, &argument) in params.iter().zip(explicit) {
            unifier.bind_param(param, argument)?;
        }
        // Typed arguments constrain parameters before untyped constants default.
        for (&formal, actual) in formals.iter().zip(actuals) {
            if !unifier
                .store_mut()
                .basic_kind(actual.ty)
                .is_some_and(|kind| kind.untyped())
                && !unifier.unify(formal, actual.ty, 0)?
            {
                return None;
            }
        }
        Self::infer_constraints_with_unifier(&mut unifier, &params, &bounds)?;
        let snapshot = unifier.snapshot(&params);
        let mut untyped: Vec<Option<BasicKind>> = vec![None; params.len()];
        for (&formal, actual) in formals.iter().zip(actuals) {
            let Some(kind) = unifier
                .store_mut()
                .basic_kind(actual.ty)
                .filter(|kind| kind.untyped() && *kind != BasicKind::UntypedNil)
            else {
                continue;
            };
            if let Some(index) = params.iter().position(|&p| p == formal)
                && snapshot[index].is_none()
            {
                untyped[index] = Some(match untyped[index] {
                    None => kind,
                    Some(previous) if previous == kind => kind,
                    Some(previous) => {
                        if previous.numeric_rank()? > kind.numeric_rank()? {
                            previous
                        } else {
                            kind
                        }
                    }
                });
            }
        }
        for (index, kind) in untyped.into_iter().enumerate() {
            if snapshot[index].is_none()
                && let Some(kind) = kind
            {
                let default_ty = unifier.store().basic(kind.default_type())?;
                unifier.bind_param(params[index], default_ty)?;
            }
        }
        Self::infer_constraints_with_unifier(&mut unifier, &params, &bounds)?;
        let bindings = unifier.snapshot(&params);
        drop(unifier);

        self.prove_constraints(&params, &bounds, &bindings)?;
        // Inference is not a proof of assignment or constant representability.
        for (formal, actual) in formals.into_iter().zip(actuals) {
            let target = self.substitute_for_params(formal, &params, &bindings, 0)?;
            self.assignable(*actual, target, int_bits)?.then_some(())?;
        }
        let inst_ty = self.instantiate_signature_nodes(generic_ty, &params, &bindings)?;
        let inferred: Vec<TypeId> = bindings.into_iter().collect::<Option<_>>()?;
        Some((inferred, inst_ty))
    }

    pub(super) fn substitute_for_params(
        &mut self,
        input: TypeId,
        tparams: &[TypeId],
        bindings: &[Option<TypeId>],
        depth: usize,
    ) -> Option<TypeId> {
        Unifier::substitute_for_params(self, input, tparams, bindings, depth)
    }
}
