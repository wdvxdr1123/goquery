//! Query-local constraints and the bounded constraint-inference fixed point.
use super::sets::TypeSet;
use super::{MAX_DEPTH, MAX_NODES, Ty, TypeArena, TypeId};
use crate::syntax::Signature;
use std::path::Path;

#[derive(Clone)]
pub(super) enum Constraint {
    Any,
    Comparable,
    Exact(TypeId),
    Approximate(TypeId),
    Set(TypeSet),
}

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
    pub(super) fn constraints(
        &mut self,
        origin: &Path,
        header: &str,
        signature: &Signature,
        environment: &[(&str, TypeId)],
    ) -> Option<Vec<Constraint>> {
        signature
            .type_parameters
            .iter()
            .map(|parameter| {
                self.constraint(origin, &header[parameter.annotation.clone()], environment)
            })
            .collect()
    }

    pub(super) fn constraint(
        &mut self,
        origin: &Path,
        text: &str,
        environment: &[(&str, TypeId)],
    ) -> Option<Constraint> {
        self.constraint_checked(origin, text, environment).ok()
    }

    pub(super) fn constraint_checked(
        &mut self,
        origin: &Path,
        text: &str,
        environment: &[(&str, TypeId)],
    ) -> super::CheckOutcome<Constraint> {
        use super::{CheckOutcome, UnknownReason};
        if self.constraint_depth >= MAX_DEPTH {
            return CheckOutcome::Unknown(UnknownReason::BudgetExceeded);
        }
        self.constraint_depth += 1;
        let result = self.constraint_set_checked(origin, text, environment, 0);
        self.constraint_depth -= 1;
        match result {
            CheckOutcome::Ok(set) => CheckOutcome::Ok(set.constraint()),
            CheckOutcome::Invalid(()) => CheckOutcome::Invalid(()),
            CheckOutcome::Unknown(reason) => CheckOutcome::Unknown(reason),
        }
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

    pub(super) fn infer_constraints_with_unifier<'a>(
        unifier: &mut super::unify::Unifier<'_, 'a>,
        params: &[TypeId],
        constraints: &[Constraint],
    ) -> Option<()> {
        for _ in 0..MAX_DEPTH {
            let known = unifier
                .snapshot(params)
                .iter()
                .filter(|slot| slot.is_some())
                .count();
            for (index, constraint) in constraints.iter().enumerate() {
                unifier.store_mut().step()?;
                let core = match constraint {
                    Constraint::Exact(ty) => Some((
                        super::sets::Term {
                            ty: *ty,
                            approximate: false,
                        },
                        true,
                    )),
                    Constraint::Approximate(ty) => Some((
                        super::sets::Term {
                            ty: *ty,
                            approximate: true,
                        },
                        true,
                    )),
                    Constraint::Set(set) => unifier.store_mut().set_core(set),
                    _ => None,
                };
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
                if let Constraint::Set(set) = constraint
                    && let Some(methods) = set.methods
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
        constraints: &[Constraint],
        bindings: &[Option<TypeId>],
    ) -> Option<()> {
        for (constraint, binding) in constraints.iter().zip(bindings) {
            let actual = (*binding)?;
            let instantiated = self.substitute_constraint(constraint, tparams, bindings)?;
            self.prove_constraint(&instantiated, actual, 0)?;
        }
        Some(())
    }

    // Deferred checks already contain substituted type IDs. Never substitute them
    // again using the checked argument as a new single-parameter environment.
    pub(super) fn prove_constraint(
        &mut self,
        constraint: &Constraint,
        actual: TypeId,
        depth: usize,
    ) -> Option<()> {
        self.step()?;
        if depth >= MAX_DEPTH {
            return None;
        }
        if !matches!(constraint, Constraint::Any) {
            let root = self.unalias(actual)?;
            if matches!(self.node(root)?, Ty::TypeParam(_)) {
                return self.prove_parameter_constraint(root, constraint, depth + 1);
            }
        }
        match constraint {
            Constraint::Set(set) => self.prove_set(set, actual, depth + 1)?,
            Constraint::Any => {}
            Constraint::Comparable => {
                self.comparable(actual, 0)?.then_some(())?;
            }
            Constraint::Exact(expected) | Constraint::Approximate(expected) => {
                let actual = if matches!(constraint, Constraint::Approximate(_)) {
                    self.underlying(actual)?
                } else {
                    actual
                };
                self.identical(*expected, actual)?.then_some(())?;
            }
        }
        Some(())
    }

    pub(super) fn parameter_constraint(&self, input: TypeId) -> Option<&Constraint> {
        self.pool_for(input).parameter_constraints.get(&input)
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
        let set = TypeSet::from_constraint(self.parameter_constraint(input)?);
        for (constraint, actual) in &set.checks {
            self.prove_constraint(constraint, *actual, 0)?;
        }
        Some(set)
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
                let constraint = self.parameter_constraint(input)?.clone();
                self.constraint_comparable(&constraint, depth + 1)
            }
            _ => None,
        }
    }

    fn constraint_comparable(&mut self, constraint: &Constraint, depth: usize) -> Option<bool> {
        self.step()?;
        if depth >= MAX_DEPTH {
            return None;
        }
        match constraint {
            Constraint::Any => Some(false),
            Constraint::Comparable => Some(true),
            // Type parameters require strict comparability of their entire type set.
            Constraint::Exact(ty) | Constraint::Approximate(ty) => {
                self.strict_comparable(*ty, depth + 1)
            }
            Constraint::Set(set) => {
                if set.comparable {
                    return Some(true);
                }
                let Some(terms) = &set.terms else {
                    return Some(false);
                };
                let mut unknown = false;
                for term in terms {
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
}
