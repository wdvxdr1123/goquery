//! Unified interface-based type parameter constraints and bounded constraint inference.
use super::sets::{TermList, TypeSet};
use super::ty::NamedCheckState;
use super::{MAX_DEPTH, MAX_NODES, Ty, TypeArena, TypeId};
use crate::syntax::Signature;
use std::path::Path;

pub(super) fn parameter_names<'s>(header: &'s str, signature: &Signature) -> Option<Vec<&'s str>> {
    if signature.type_parameters.len() > MAX_NODES {
        return None;
    }
    let mut names = Vec::new();
    for parameter in &signature.type_parameters {
        let name = &header[parameter.name.as_ref()?.clone()];
        if name != "_" && names.contains(&name) {
            return None;
        }
        names.push(name);
    }
    Some(names)
}

impl TypeArena<'_> {
    pub(super) fn constraint(
        &mut self,
        origin: &Path,
        text: &str,
        environment: &[(&str, TypeId)],
    ) -> Option<TypeId> {
        self.constraint_checked(origin, text, environment).ok()
    }

    pub(super) fn constraint_checked(
        &mut self,
        origin: &Path,
        text: &str,
        environment: &[(&str, TypeId)],
    ) -> super::CheckOutcome<TypeId> {
        use super::{CheckOutcome, UnknownReason};
        if self.constraint_depth >= MAX_DEPTH {
            return CheckOutcome::Unknown(UnknownReason::BudgetExceeded);
        }
        self.constraint_depth += 1;
        let result = self.parse_constraint_checked(origin, text, None, environment, 0);
        self.constraint_depth -= 1;
        result
    }

    pub(super) fn universe_constraint(
        &self,
        origin: &Path,
        name: &str,
        environment: &[(&str, TypeId)],
    ) -> Option<bool> {
        Some(
            !environment.iter().any(|(parameter, _)| *parameter == name)
                && self.resolver().unqualified_symbol(origin, name).is_none()
                && !self
                    .workspace
                    .files
                    .get(origin)?
                    .imports
                    .iter()
                    .any(|import| self.resolver().import_name(origin, import) == Some(name)),
        )
    }

    fn is_unconstrained_any(&self, bound: TypeId) -> bool {
        self.interface_data(bound).is_some_and(|data| {
            data.builtin_any
                && data.methods.is_empty()
                && !data.comparable
                && data.embeddeds.is_empty()
        })
    }

    /// Validate named type arguments inside a constraint interface without
    /// re-entering an already-pending `check_named_arguments` cycle.
    pub(super) fn validate_constraint_args(&mut self, bound: TypeId, depth: usize) -> Option<()> {
        self.step()?;
        if depth >= MAX_DEPTH {
            return None;
        }
        if self.is_unconstrained_any(bound) {
            return Some(());
        }
        match *self.node(bound)? {
            Ty::Invalid => None,
            Ty::Named { arguments, .. } | Ty::Alias { arguments, .. } => {
                if self.named_info(bound).is_some() {
                    if self.named_check_state(bound) != Some(NamedCheckState::Pending) {
                        self.check_named_arguments(bound)?;
                    }
                    let args = self.arguments_slice(bound, arguments).to_vec();
                    for arg in args {
                        if self.named_check_state(arg) != Some(NamedCheckState::Pending) {
                            self.validate_type(arg)?;
                        }
                    }
                    if self.interface_constructor(bound) == Some(true)
                        && let Some(rhs) = self.expand_origin_rhs(bound)
                    {
                        self.validate_constraint_args(rhs, depth + 1)?;
                    }
                }
                Some(())
            }
            Ty::Interface(_) => {
                let data = self.interface_data(bound)?.clone();
                let embeddeds = self.arguments_slice(bound, data.embeddeds).to_vec();
                for emb in embeddeds {
                    self.validate_constraint_args(emb, depth + 1)?;
                }
                Some(())
            }
            Ty::Union(slice) => {
                let terms: Vec<TypeId> = self
                    .terms_slice(bound, slice)
                    .iter()
                    .map(|t| t.ty)
                    .collect();
                for ty in terms {
                    self.validate_constraint_args(ty, depth + 1)?;
                }
                Some(())
            }
            Ty::Basic(_) | Ty::TypeParam(_) => Some(()),
            _ => self.validate_type(bound),
        }
    }

    pub(super) fn infer_constraints_with_unifier<'a>(
        unifier: &mut super::unify::Unifier<'_, 'a>,
        params: &[TypeId],
        bounds: &[TypeId],
    ) -> Option<()> {
        for _ in 0..MAX_DEPTH {
            let known = unifier
                .snapshot(params)
                .iter()
                .filter(|slot| slot.is_some())
                .count();
            for (index, &bound) in bounds.iter().enumerate() {
                unifier.store_mut().step()?;
                let Some(set) = unifier.store_mut().constraint_type_set(bound, 0) else {
                    continue;
                };
                let core = unifier.store_mut().set_core(&set);
                if let Some((term, single)) = core {
                    let bindings = unifier.snapshot(params);
                    if let Some(actual) = bindings[index] {
                        let actual = if term.approximate || !single {
                            unifier.store_mut().underlying(actual)?
                        } else {
                            actual
                        };
                        if !unifier.unify(term.ty, actual, usize::from(single))? {
                            return None;
                        }
                    } else if single
                        && !term.approximate
                        && let Some(ty) = super::unify::Unifier::substitute_for_params(
                            unifier.store_mut(),
                            term.ty,
                            params,
                            &bindings,
                            0,
                        )
                    {
                        unifier.bind_param(params[index], ty)?;
                    }
                }
                let bindings = unifier.snapshot(params);
                if let Some(methods) = set.methods
                    && let Some(actual) = bindings[index]
                {
                    // An unsuccessful speculative method match must not retain
                    // partial bindings. Final satisfaction remains mandatory.
                    let saved = unifier.save_state();
                    if unifier.unify(methods, actual, 0) != Some(true) {
                        unifier.restore_state(saved);
                    }
                }
            }
            if unifier
                .snapshot(params)
                .iter()
                .filter(|slot| slot.is_some())
                .count()
                == known
            {
                return Some(());
            }
        }
        None
    }

    pub(super) fn prove_constraints(
        &mut self,
        tparams: &[TypeId],
        bounds: &[TypeId],
        bindings: &[Option<TypeId>],
    ) -> Option<()> {
        for (&bound, binding) in bounds.iter().zip(bindings) {
            let actual = (*binding)?;
            let instantiated = self.substitute_for_params(bound, tparams, bindings, 0)?;
            self.prove_constraint(instantiated, actual, 0)?;
        }
        Some(())
    }

    pub(super) fn prove_constraint(
        &mut self,
        bound: TypeId,
        actual: TypeId,
        depth: usize,
    ) -> Option<()> {
        self.step()?;
        if depth >= MAX_DEPTH {
            return None;
        }
        if self.is_unconstrained_any(bound) {
            return Some(());
        }
        let root = self.unalias(actual)?;
        if matches!(self.node(root)?, Ty::TypeParam(_)) {
            return self.prove_parameter_constraint(root, bound, depth + 1);
        }
        self.validate_constraint_args(bound, depth + 1)?;
        let set = self.constraint_type_set(bound, depth + 1)?;
        self.prove_set(&set, actual)
    }

    /// The common underlying type, including compatible channel directions.
    pub(super) fn common_under(&mut self, input: TypeId) -> Option<TypeId> {
        self.step()?;
        let input = self.underlying(input)?;
        if !matches!(self.node(input)?, Ty::TypeParam(_)) {
            return Some(input);
        }
        let set = self.parameter_set(input)?;
        let ty = self.set_core(&set)?.0.ty;
        self.underlying(ty)
    }

    pub(super) fn parameter_set(&mut self, input: TypeId) -> Option<TypeSet> {
        let bound = self.type_param_bound(input)?;
        self.validate_constraint_args(bound, 0)?;
        self.constraint_type_set(bound, 0)
    }

    pub(super) fn comparable(&mut self, input: TypeId, depth: usize) -> Option<bool> {
        self.step()?;
        if depth >= MAX_DEPTH {
            return None;
        }
        let input = self.underlying(input)?;
        match *self.node(input)? {
            Ty::Basic(kind) => Some(!kind.untyped()),
            Ty::Pointer(_) | Ty::Chan { .. } | Ty::Interface(_) => Some(true),
            Ty::Array { element, .. } => self.comparable(element, depth + 1),
            Ty::Struct(_) => {
                let fields = self.struct_data(input)?.fields.clone();
                let types: Vec<_> = self
                    .fields_slice(input, fields)?
                    .iter()
                    .map(|field| field.ty)
                    .collect();
                for ty in types {
                    if !self.comparable(ty, depth + 1)? {
                        return Some(false);
                    }
                }
                Some(true)
            }
            Ty::Slice(_) | Ty::Map { .. } | Ty::Signature(_) => Some(false),
            Ty::TypeParam(_) => {
                let bound = self.type_param_bound(input)?;
                let set = self.constraint_type_set(bound, depth + 1)?;
                if set.comparable {
                    return Some(true);
                }
                match &set.terms {
                    TermList::All => Some(false),
                    TermList::Empty => Some(true),
                    TermList::Normal(terms) => {
                        let mut unknown = false;
                        for term in terms.iter() {
                            match self.strict_comparable(term.ty, depth + 1) {
                                Some(false) => return Some(false),
                                Some(true) => {}
                                None => unknown = true,
                            }
                        }
                        if unknown { None } else { Some(true) }
                    }
                }
            }
            _ => None,
        }
    }
}
