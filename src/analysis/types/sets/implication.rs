//! Formal constraint satisfaction uses bounded term and method-set inclusion.
use crate::analysis::engine::types::{MAX_DEPTH, Mode, Operand, TypeArena, TypeId};

impl TypeArena<'_> {
    pub(in crate::analysis::engine::types) fn prove_parameter_constraint(
        &mut self,
        actual: TypeId,
        required: TypeId,
        depth: usize,
    ) -> Option<()> {
        self.step()?;
        if depth >= MAX_DEPTH {
            return None;
        }
        let source_bound = self.type_param_bound(actual)?;
        // Validate both declarations' type arguments even when an empty source
        // set would otherwise make inclusion vacuous.
        self.validate_constraint_args(source_bound, depth + 1)?;
        self.validate_constraint_args(required, depth + 1)?;
        let source = self.constraint_type_set(source_bound, depth + 1)?;
        let target = self.constraint_type_set(required, depth + 1)?;
        if source.terms.is_empty() {
            return Some(());
        }
        if target.comparable {
            self.comparable(actual, depth + 1)?.then_some(())?;
        }
        if let Some(required_methods) = target.methods {
            let available = source.methods?;
            self.assign_interface(
                Operand {
                    mode: Mode::Value,
                    ty: available,
                    value: None,
                },
                required_methods,
                self.integer_bits()?,
            )?
            .then_some(())?;
        }
        if let Some(target_terms) = target.terms.as_slice() {
            for &source_term in source.terms.as_slice()? {
                let mut covered = false;
                for &target_term in target_terms {
                    self.step()?;
                    // ~T contains defined types too; an exact T cannot cover it.
                    if source_term.approximate && !target_term.approximate {
                        continue;
                    }
                    let ty = if target_term.approximate {
                        self.underlying(source_term.ty)?
                    } else {
                        source_term.ty
                    };
                    if self.identical(ty, target_term.ty)? {
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
