//! Query-local formal identities are keyed by declaration, not import order/name.
use super::{Ty, TypeArena, TypeId};
use std::path::Path;

pub(crate) struct LocalFormal<'a> {
    pub(crate) name: &'a str,
    pub(crate) selection: usize,
    pub(crate) bound: Option<&'a str>,
}

pub(super) struct LocalParameter<'a> {
    origin: &'a Path,
    selection: usize,
    name: String,
    ty: TypeId,
    ready: bool,
}

impl TypeArena<'_> {
    pub(super) fn local_parameter(&self, ty: TypeId) -> Option<(&Path, usize, &str)> {
        let parameter = self
            .local_parameters
            .iter()
            .find(|p| p.ty == ty && p.ready)?;
        Some((parameter.origin, parameter.selection, &parameter.name))
    }

    pub(crate) fn parse_parameterized(
        &mut self,
        origin: &Path,
        source: &str,
        formals: &[LocalFormal<'_>],
        receiver: Option<&str>,
    ) -> Option<TypeId> {
        if formals.len() > 32 {
            return None;
        }
        let origin = self.workspace.files.get_key_value(origin)?.0.as_path();
        let mut environment = Vec::new();
        let mut imports = Vec::new();
        for formal in formals {
            if environment.iter().any(|(name, _)| *name == formal.name) {
                return None;
            }
            let previous = self.local_parameters.iter().position(|parameter| {
                parameter.origin == origin && parameter.selection == formal.selection
            });
            let index = if let Some(index) = previous {
                self.local_parameters[index].ready.then_some(())?;
                index
            } else {
                let index = self.local_parameters.len();
                if index >= 32 {
                    return None;
                }
                let bytes: usize = self.local_parameters.iter().map(|p| p.name.len()).sum();
                if bytes.checked_add(formal.name.len())? > super::MAX_BYTES {
                    return None;
                }
                let ty = self.push(Ty::Parameter(index))?;
                self.local_parameters.push(LocalParameter {
                    origin,
                    selection: formal.selection,
                    name: formal.name.to_owned(),
                    ty,
                    ready: false,
                });
                index
            };
            environment.push((formal.name, self.local_parameters[index].ty));
            imports.push(index);
        }
        if imports
            .iter()
            .all(|index| self.local_parameters[*index].ready)
        {
            return self.parse_in(origin, source, &environment);
        }
        if receiver.is_some() && formals.iter().any(|formal| formal.bound.is_some()) {
            return None;
        }
        let bounds: Vec<_> = formals
            .iter()
            .zip(&environment)
            .filter_map(|(formal, (_, ty))| formal.bound.map(|bound| (*ty, bound)))
            .collect();
        self.import_parameter_bounds(origin, &bounds, &environment)?;
        if let Some(receiver) = receiver {
            let base = self.import_receiver_parameters(origin, receiver, &environment)?;
            let root = self.underlying(base)?;
            if matches!(
                self.nodes[usize::from(root.0)],
                Ty::Pointer(_) | Ty::Interface { .. }
            ) {
                return None;
            }
        }
        let ty = self.parse_in(origin, source, &environment)?;
        // Failed/recursive imports stay unready: later operands must not reuse
        // facts from a declaration whose checking never completed.
        for index in imports {
            self.local_parameters[index].ready = true;
        }
        Some(ty)
    }

    /// Resolve independent bounds first, then retry dependencies with new facts.
    pub(super) fn import_parameter_bounds(
        &mut self,
        origin: &Path,
        bounds: &[(TypeId, &str)],
        environment: &[(&str, TypeId)],
    ) -> Option<()> {
        if bounds.len() > 32 {
            return None;
        }
        let mut pending: Vec<_> = bounds
            .iter()
            .filter(|(ty, _)| {
                !self
                    .parameter_constraints
                    .iter()
                    .any(|(previous, _)| previous == ty)
            })
            .copied()
            .collect();
        while !pending.is_empty() {
            let before = pending.len();
            let mut remaining = Vec::new();
            for (ty, text) in pending {
                self.step()?;
                if let Some(constraint) = self.constraint(origin, text, environment) {
                    if self.parameter_constraints.len() >= 32 {
                        return None;
                    }
                    self.parameter_constraints.push((ty, constraint));
                } else {
                    if self.invalid_type {
                        return None;
                    }
                    remaining.push((ty, text));
                }
            }
            if remaining.len() == before {
                return None;
            }
            pending = remaining;
        }
        Some(())
    }
}
