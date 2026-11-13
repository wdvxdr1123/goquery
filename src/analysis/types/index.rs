//! Index and slice operands retain identity and validate Go bounds/key rules.
use super::{BasicKind, Constant, Mode, Operand, Ty, TypeArena, TypeId};

#[derive(Clone, Copy)]
struct IndexProjection {
    key: Option<TypeId>,
    element: TypeId,
    mode: Mode,
    bound: Option<u64>,
}

impl TypeArena<'_> {
    pub(crate) fn index(
        &mut self,
        input: Operand,
        index: Operand,
        result: usize,
        count: usize,
        int_bits: u32,
    ) -> Option<Operand> {
        self.step()?;
        let projection = self.index_projection(input)?;
        if let Some(key) = projection.key {
            if !matches!(count, 1 | 2) || result >= count {
                return None;
            }
            self.comparable(key, 0)?.then_some(())?;
            self.assignable(index, key, int_bits)?.then_some(())?;
            return Some(Operand {
                mode: Mode::Value,
                ty: if result == 0 {
                    projection.element
                } else {
                    self.basic(BasicKind::Bool)?
                },
                value: None,
            });
        }
        if count != 1 || result != 0 {
            return None;
        }
        if let Some(value) = self.index_value(index, int_bits)?
            && projection.bound.is_some_and(|length| value >= length)
        {
            return None;
        }
        Some(Operand {
            mode: projection.mode,
            ty: projection.element,
            value: None,
        })
    }

    fn index_projection(&mut self, input: Operand) -> Option<IndexProjection> {
        let root = self.underlying(input.ty)?;
        if !matches!(self.nodes[usize::from(root.0)], Ty::Parameter(_)) {
            return self.index_term(input, root);
        }
        let set = self.parameter_set(root)?;
        let terms = set.terms?;
        let mut terms = terms.into_iter();
        let ty = terms.next()?.ty;
        let root = self.underlying(ty)?;
        let mut projection = self.index_term(Operand { ty, ..input }, root)?;
        for term in terms {
            self.step()?;
            let root = self.underlying(term.ty)?;
            let next = self.index_term(
                Operand {
                    ty: term.ty,
                    ..input
                },
                root,
            )?;
            match (projection.key, next.key) {
                (Some(a), Some(b)) => self.identical(a, b)?.then_some(())?,
                (None, None) => {}
                _ => return None,
            }
            self.identical(projection.element, next.element)?
                .then_some(())?;
            if next.mode == Mode::Value {
                projection.mode = Mode::Value;
            }
            // Match go/types' ordered length accumulation: an unknown first
            // length stays unknown; later finite array lengths can reduce it.
            if let (Some(previous), Some(next)) = (projection.bound, next.bound) {
                projection.bound = Some(previous.min(next));
            }
        }
        Some(projection)
    }

    fn index_term(&mut self, input: Operand, mut root: TypeId) -> Option<IndexProjection> {
        let pointer = if let Ty::Pointer(element) = self.nodes[usize::from(root.0)] {
            root = self.underlying(element)?;
            true
        } else {
            false
        };
        let (key, element, mode, bound) = match self.nodes[usize::from(root.0)].clone() {
            Ty::Map { key, value } if !pointer => (Some(key), value, Mode::Value, None),
            Ty::Array { length, element } => (
                None,
                element,
                if pointer || input.mode == Mode::Variable {
                    Mode::Variable
                } else {
                    Mode::Value
                },
                Some(length),
            ),
            Ty::Slice(element) if !pointer => (None, element, Mode::Variable, None),
            Ty::Basic(kind) if !pointer && kind.string() => {
                let bound = if input.mode == Mode::Constant {
                    let Some(Constant::String {
                        length: Some(length),
                    }) = input.value
                    else {
                        return None;
                    };
                    Some(length)
                } else {
                    None
                };
                (None, self.basic(BasicKind::Uint8)?, Mode::Value, bound)
            }
            _ => return None,
        };
        Some(IndexProjection {
            key,
            element,
            mode,
            bound,
        })
    }

    pub(crate) fn index_value(&mut self, index: Operand, int_bits: u32) -> Option<Option<u64>> {
        let root = self.underlying(index.ty)?;
        if matches!(self.nodes[usize::from(root.0)], Ty::Parameter(_)) {
            if index.mode == Mode::Constant {
                return None;
            }
            let terms = self.parameter_set(root)?.terms?;
            if terms.is_empty() {
                return None;
            }
            for term in terms {
                self.step()?;
                self.basic_kind(term.ty)?.integer().then_some(())?;
            }
            return Some(None);
        }
        let kind = self.basic_kind(root)?;
        if index.mode != Mode::Constant {
            return kind.integer().then_some(None);
        }
        let value = if kind.untyped() && kind.numeric_rank().is_some() {
            // go/types converts every untyped numeric index to int first.
            // Zero imaginary components and integrality require exact facts.
            match super::complex::real(index.value?)?.constant() {
                Constant::Int(value) => value,
                _ => return None,
            }
        } else {
            let Constant::Int(value) = index.value? else {
                return None;
            };
            kind.integer().then_some(())?;
            value
        };
        super::operand::representable(Constant::Int(value), BasicKind::Int, int_bits)
            .then_some(())?;
        Some(Some(u64::try_from(value).ok()?))
    }

    pub(crate) fn is_array(&mut self, input: super::TypeId) -> Option<bool> {
        let root = self.underlying(input)?;
        Some(matches!(self.nodes[usize::from(root.0)], Ty::Array { .. }))
    }

    pub(crate) fn field_addressable(&mut self, owner: Operand) -> Option<bool> {
        let root = self.underlying(owner.ty)?;
        Some(match self.nodes[usize::from(root.0)] {
            Ty::Pointer(element) => {
                let root = self.underlying(element)?;
                matches!(self.nodes[usize::from(root.0)], Ty::Struct { .. })
            }
            Ty::Struct { .. } => owner.mode == Mode::Variable,
            _ => false,
        })
    }

    fn slice_root(&mut self, input: TypeId) -> Option<TypeId> {
        let root = self.underlying(input)?;
        if !matches!(self.nodes[usize::from(root.0)], Ty::Parameter(_)) {
            return Some(root);
        }
        let terms = self.parameter_set(root)?.terms?;
        let mut common = None;
        let mut has_string = false;
        for term in terms {
            self.step()?;
            let mut ty = self.underlying(term.ty)?;
            if matches!(self.nodes[usize::from(ty.0)], Ty::Basic(kind) if kind.string()) {
                has_string = true;
                let byte = self.basic(BasicKind::Uint8)?;
                ty = self.push(Ty::Slice(byte))?;
            }
            if let Some(previous) = common {
                self.identical(previous, ty)?.then_some(())?;
            } else {
                common = Some(ty);
            }
        }
        let common = common?;
        if has_string {
            self.basic(BasicKind::String)
        } else {
            Some(common)
        }
    }

    pub(crate) fn slice(
        &mut self,
        input: Operand,
        indices: [Option<Operand>; 3],
        full: bool,
        int_bits: u32,
    ) -> Option<Operand> {
        self.step()?;
        if full && (indices[1].is_none() || indices[2].is_none()) || !full && indices[2].is_some() {
            return None;
        }
        let mut root = self.slice_root(input.ty)?;
        let pointer = if let Ty::Pointer(element) = self.nodes[usize::from(root.0)] {
            root = self.underlying(element)?;
            true
        } else {
            false
        };
        let (ty, bound) = match self.nodes[usize::from(root.0)].clone() {
            Ty::Slice(_) if !pointer => (input.ty, None),
            Ty::Array { length, element } => {
                if !pointer && input.mode != Mode::Variable {
                    return None;
                }
                (self.push(Ty::Slice(element))?, Some(length))
            }
            Ty::Basic(kind) if !pointer && kind.string() && !full => {
                let bound = if input.mode == Mode::Constant {
                    let Some(Constant::String {
                        length: Some(length),
                    }) = input.value
                    else {
                        return None;
                    };
                    Some(length)
                } else {
                    None
                };
                let ty = if kind.untyped() {
                    self.basic(BasicKind::String)?
                } else {
                    input.ty
                };
                (ty, bound)
            }
            _ => return None,
        };
        let mut values = [Some(0), bound, bound];
        for (slot, index) in values.iter_mut().zip(indices) {
            if let Some(index) = index {
                *slot = self.index_value(index, int_bits)?;
                if slot.is_some_and(|value| bound.is_some_and(|length| value > length)) {
                    return None;
                }
            }
        }
        for (i, low) in values.iter().enumerate() {
            for high in &values[i + 1..] {
                if let (Some(low), Some(high)) = (low, high)
                    && low > high
                {
                    return None;
                }
            }
        }
        Some(Operand {
            mode: Mode::Value,
            ty,
            value: None,
        })
    }
}
