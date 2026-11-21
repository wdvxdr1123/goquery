//! First-class type parameters stored directly in `ArenaPool::type_params`.
use super::{CheckOutcome, Ty, TypeArena, TypeId};
use std::path::Path;

pub(crate) struct LocalFormal<'a> {
    pub(crate) name: &'a str,
    pub(crate) selection: usize,
    pub(crate) bound: Option<&'a str>,
}

impl<'a> TypeArena<'a> {
    pub(crate) fn push_symbolic_parameter(&mut self, origin: &Path, name: &str) -> Option<TypeId> {
        let origin = self.workspace.files.get_key_value(origin)?.0.as_path();
        let index = u16::try_from(self.local.pool.type_params.len()).ok()?;
        let selection = u32::from(index);
        self.local
            .pool
            .push_type_param_data(Some(origin), selection, index, name, true, false)
    }

    pub(crate) fn parse_parameterized(
        &mut self,
        origin: &Path,
        source: &str,
        file_offset: Option<usize>,
        formals: &[LocalFormal<'_>],
        receiver: Option<&str>,
    ) -> Option<TypeId> {
        if formals.len() > self.local.pool.node_limit {
            return None;
        }
        let origin = self.workspace.files.get_key_value(origin)?.0.as_path();
        let mut environment = Vec::with_capacity(formals.len());
        let mut imports = Vec::with_capacity(formals.len());
        for formal in formals {
            if environment.iter().any(|(name, _)| *name == formal.name) {
                return None;
            }
            let sel = u32::try_from(formal.selection).ok()?;
            let ty = if let Some(&existing) = self.local.pool.formal_params.get(&(origin, sel)) {
                self.type_param_data(existing)?.is_ready().then_some(())?;
                existing
            } else {
                let index = u16::try_from(self.local.pool.type_params.len()).ok()?;
                let ty = self.local.pool.push_type_param_data(
                    Some(origin),
                    sel,
                    index,
                    formal.name,
                    false,
                    false,
                )?;
                self.local.pool.formal_params.insert((origin, sel), ty);
                ty
            };
            environment.push((formal.name, ty));
            imports.push(ty);
        }
        if imports
            .iter()
            .all(|&ty| self.type_param_data(ty).is_some_and(|p| p.is_ready()))
        {
            return self.parse_in_internal(origin, source, file_offset, &environment);
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
            if matches!(self.node(root)?, Ty::Pointer(_) | Ty::Interface(_)) {
                return None;
            }
        }
        let ty = self.parse_in_internal(origin, source, file_offset, &environment)?;
        // Failed/recursive imports stay unready: later operands must not reuse
        // facts from a declaration whose checking never completed.
        for tp in imports {
            if let Some(data) = self.type_param_data_mut(tp) {
                data.set_ready(true);
            }
        }
        Some(ty)
    }

    /// Parse all bounds into unified constraint interface `TypeId`s on
    /// `TypeParamData::bound`, then validate their type sets and arguments.
    pub(super) fn import_parameter_bounds(
        &mut self,
        origin: &Path,
        bounds: &[(TypeId, &str)],
        environment: &[(&str, TypeId)],
    ) -> Option<()> {
        if bounds.len() > self.active_node_limit() {
            return None;
        }
        let pending: Vec<_> = bounds
            .iter()
            .filter(|(ty, _)| self.type_param_bound(*ty).is_none())
            .copied()
            .collect();
        if pending.is_empty() {
            return Some(());
        }
        // Pass 1: parse each constraint into an interface TypeId and attach it
        // directly to TypeParamData.bound so interdependent bounds resolve cleanly.
        let mut parsed_bounds = Vec::with_capacity(pending.len());
        for (ty, text) in &pending {
            self.step()?;
            let bound = match self.constraint_checked(origin, text, environment) {
                CheckOutcome::Ok(bound) => bound,
                CheckOutcome::Invalid(()) | CheckOutcome::Unknown(_) => {
                    for &(prev_ty, _) in &parsed_bounds {
                        self.set_type_param_bound(prev_ty, TypeId::INVALID);
                    }
                    return None;
                }
            };
            if !self.set_type_param_bound(*ty, bound) {
                for &(prev_ty, _) in &parsed_bounds {
                    self.set_type_param_bound(prev_ty, TypeId::INVALID);
                }
                return None;
            }
            parsed_bounds.push((*ty, bound));
        }
        // Pass 2: validate type sets (union overlap, comparable/methods in unions)
        // once all sibling bounds are in place.
        for &(_, bound) in &parsed_bounds {
            if !matches!(
                self.constraint_type_set_checked(bound, 0),
                CheckOutcome::Ok(_)
            ) {
                for &(prev_ty, _) in &parsed_bounds {
                    self.set_type_param_bound(prev_ty, TypeId::INVALID);
                }
                return None;
            }
        }
        Some(())
    }
}
