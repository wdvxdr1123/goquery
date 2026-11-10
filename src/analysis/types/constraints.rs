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
        if self.constraint_depth >= MAX_DEPTH {
            return None;
        }
        self.constraint_depth += 1;
        let result = self
            .constraint_set(origin, text, environment, 0)
            .map(TypeSet::constraint);
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
                && self.workspace.unqualified_symbol(origin, name).is_none()
                && !self
                    .workspace
                    .files
                    .get(origin)?
                    .imports
                    .iter()
                    .any(|import| self.workspace.import_name(import) == Some(name)),
        )
    }

    pub(super) fn infer_constraints(
        &mut self,
        constraints: &[Constraint],
        bindings: &mut [Option<TypeId>],
    ) -> Option<()> {
        for _ in 0..MAX_DEPTH {
            let known = bindings.iter().filter(|slot| slot.is_some()).count();
            for (index, constraint) in constraints.iter().enumerate() {
                self.step()?;
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
                    Constraint::Set(set) => self.set_core(set),
                    _ => None,
                };
                if let Some((term, single)) = core {
                    if let Some(actual) = bindings[index] {
                        let actual = if term.approximate || !single {
                            self.underlying(actual)?
                        } else {
                            actual
                        };
                        self.unify(term.ty, actual, bindings, usize::from(single))?;
                    } else if single
                        && !term.approximate
                        && let Some(ty) = self.substitute(term.ty, bindings, 0)
                    {
                        bindings[index] = Some(ty);
                    }
                }
                if let Constraint::Set(set) = constraint
                    && let Some(methods) = set.methods
                    && let Some(actual) = bindings[index]
                {
                    // An unsuccessful speculative method match must not retain
                    // partial bindings. Final satisfaction remains mandatory.
                    let mut inferred = bindings.to_vec();
                    if self.unify(methods, actual, &mut inferred, 0).is_some() {
                        bindings.copy_from_slice(&inferred);
                    }
                }
            }
            if bindings.iter().filter(|slot| slot.is_some()).count() == known {
                return Some(());
            }
        }
        None
    }

    pub(super) fn prove_constraints(
        &mut self,
        constraints: &[Constraint],
        bindings: &[Option<TypeId>],
    ) -> Option<()> {
        for (constraint, binding) in constraints.iter().zip(bindings) {
            let actual = (*binding)?;
            let instantiated = self.substitute_constraint(constraint, bindings)?;
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
            if matches!(self.nodes[usize::from(root.0)], Ty::Parameter(_)) {
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
        self.parameter_constraints
            .iter()
            .find(|(ty, _)| *ty == input)
            .map(|(_, constraint)| constraint)
    }

    /// The common underlying type, including compatible channel directions.
    pub(super) fn common_under(&mut self, input: TypeId) -> Option<TypeId> {
        self.step()?;
        let input = self.underlying(input)?;
        if !matches!(self.nodes[usize::from(input.0)], Ty::Parameter(_)) {
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
        match self.nodes[usize::from(input.0)] {
            Ty::Basic(kind) => Some(!kind.untyped()),
            Ty::Pointer(_) | Ty::Chan { .. } | Ty::Interface { .. } => Some(true),
            Ty::Array { element, .. } => self.comparable(element, depth + 1),
            Ty::Struct { ref fields, .. } => {
                let types: Vec<_> = self.fields[fields.clone()]
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
            Ty::Slice(_) | Ty::Map { .. } | Ty::Signature { .. } => Some(false),
            Ty::Parameter(_) => {
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
