//! Formal constraint satisfaction uses bounded term and method-set inclusion.
use super::{Term, TypeSet};
use crate::analysis::types::constraints::Constraint;
use crate::analysis::types::{MAX_DEPTH, Mode, Operand, TypeArena, TypeId};

impl TypeSet {
    pub(in crate::analysis::types) fn from_constraint(constraint: &Constraint) -> Self {
        match constraint {
            Constraint::Set(set) => set.clone(),
            Constraint::Any => Self::all(),
            Constraint::Comparable => Self {
                comparable: true,
                ..Self::all()
            },
            Constraint::Exact(ty) | Constraint::Approximate(ty) => Self {
                terms: Some(vec![Term {
                    ty: *ty,
                    approximate: matches!(constraint, Constraint::Approximate(_)),
                }]),
                ..Self::all()
            },
        }
    }
}

impl TypeArena<'_> {
    pub(in crate::analysis::types) fn prove_parameter_constraint(
        &mut self,
        actual: TypeId,
        required: &Constraint,
        depth: usize,
    ) -> Option<()> {
        self.step()?;
        if depth >= MAX_DEPTH {
            return None;
        }
        let source = TypeSet::from_constraint(self.parameter_constraint(actual)?);
        let target = TypeSet::from_constraint(required);
        // These checks are instantiated already. Validate both declarations even
        // when an empty source set would otherwise make inclusion vacuous.
        for (constraint, ty) in source.checks.iter().chain(&target.checks) {
            self.prove_constraint(constraint, *ty, depth + 1)?;
        }
        if source.terms.as_ref().is_some_and(Vec::is_empty) {
            return Some(());
        }
        if target.comparable {
            self.comparable(actual, depth + 1)?.then_some(())?;
        }
        if let Some(required) = target.methods {
            let available = source.methods?;
            self.assign_interface(
                Operand {
                    mode: Mode::Value,
                    ty: available,
                    value: None,
                },
                required,
                self.workspace.integer_bits()?,
            )?
            .then_some(())?;
        }
        if let Some(target) = &target.terms {
            for &source in source.terms.as_ref()? {
                let mut covered = false;
                for &target in target {
                    self.step()?;
                    // ~T contains defined types too; an exact T cannot cover it.
                    if source.approximate && !target.approximate {
                        continue;
                    }
                    let ty = if target.approximate {
                        self.underlying(source.ty)?
                    } else {
                        source.ty
                    };
                    if self.identical(ty, target.ty)? {
                        covered = true;
                        break;
                    }
                }
                covered.then_some(())?;
            }
        }
        Some(())
    }
}
