//! Index and slice operands retain identity and validate Go bounds/key rules.
use super::{
    AssignmentError, BasicKind, CheckError, ConstAssignError, Constant, Mode, Operand, Ty,
    TypeArena, TypeId,
};
use CheckError::{Invalid, Unknown};

#[derive(Clone, Copy)]
pub(crate) enum IndexError {
    NonIndexable,
    Key {
        target: TypeId,
        reason: AssignmentError,
    },
    Value(IndexValueError),
    Bounds {
        index: u64,
        length: u64,
    },
}

#[derive(Clone, Copy)]
pub(crate) enum IndexValueError {
    NonInteger,
    Negative,
    Overflow,
    ImplicitConversion(ConstAssignError),
}

pub(crate) enum SliceError {
    NonSliceable,
    Unaddressable,
    StringFull,
}

pub(crate) struct SliceResult {
    pub(crate) operand: Option<Operand>,
    pub(crate) errors: [Option<IndexError>; 3],
    pub(crate) swapped: Option<(usize, u64, u64)>,
}

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
        int_bits: u32,
    ) -> Result<Operand, CheckError<IndexError>> {
        self.step().ok_or(Unknown)?;
        if !input.mode.is_value() || !index.mode.is_value() {
            return Err(Unknown);
        }
        let projection = self.index_projection(input)?;
        if let Some(key) = projection.key {
            if self.comparable(key, 0) != Some(true) {
                return Err(Unknown);
            }
            self.check_assignment(index, key, int_bits)
                .map_err(|error| {
                    error.map(|reason| IndexError::Key {
                        target: key,
                        reason,
                    })
                })?;
            return Ok(Operand {
                mode: Mode::MapIndex,
                ty: projection.element,
                value: None,
            });
        }
        self.bounded_index(index, projection.bound, int_bits)?;
        Ok(Operand {
            mode: projection.mode,
            ty: projection.element,
            value: None,
        })
    }

    fn bounded_index(
        &mut self,
        index: Operand,
        limit: Option<u64>,
        int_bits: u32,
    ) -> Result<Option<u64>, CheckError<IndexError>> {
        let value = self
            .checked_index_value(index, int_bits)
            .map_err(|error| error.map(IndexError::Value))?;
        if let (Some(index), Some(length)) = (value, limit)
            && index >= length
        {
            return Err(Invalid(IndexError::Bounds { index, length }));
        }
        Ok(value)
    }

    fn index_projection(
        &mut self,
        input: Operand,
    ) -> Result<IndexProjection, CheckError<IndexError>> {
        let root = self.underlying(input.ty).ok_or(Unknown)?;
        if !matches!(self.node(root).ok_or(Unknown)?, Ty::TypeParam(_)) {
            return self.index_term(input, root);
        }
        let set = self.parameter_set(root).ok_or(Unknown)?;
        let terms = set.terms.ok_or(Unknown)?;
        let mut terms = terms.into_iter();
        let ty = terms.next().ok_or(Unknown)?.ty;
        let root = self.underlying(ty).ok_or(Unknown)?;
        let mut projection = self.index_term(Operand { ty, ..input }, root)?;
        for term in terms {
            self.step().ok_or(Unknown)?;
            let root = self.underlying(term.ty).ok_or(Unknown)?;
            let next = self.index_term(
                Operand {
                    ty: term.ty,
                    ..input
                },
                root,
            )?;
            let same_key = match (projection.key, next.key) {
                (Some(a), Some(b)) => self.identical(a, b).ok_or(Unknown)?,
                (None, None) => true,
                _ => false,
            };
            if !same_key
                || !self
                    .identical(projection.element, next.element)
                    .ok_or(Unknown)?
            {
                return Err(Invalid(IndexError::NonIndexable));
            }
            if next.mode == Mode::Value {
                projection.mode = Mode::Value;
            }
            // Match go/types' ordered length accumulation: an unknown first
            // length stays unknown; later finite array lengths can reduce it.
            if let (Some(previous), Some(next)) = (projection.bound, next.bound) {
                projection.bound = Some(previous.min(next));
            }
        }
        Ok(projection)
    }

    fn index_term(
        &mut self,
        input: Operand,
        mut root: TypeId,
    ) -> Result<IndexProjection, CheckError<IndexError>> {
        let pointer = if let Ty::Pointer(element) = *self.node(root).ok_or(Unknown)? {
            root = self.underlying(element).ok_or(Unknown)?;
            true
        } else {
            false
        };
        let (key, element, mode, bound) = match *self.node(root).ok_or(Unknown)? {
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
                        return Err(Unknown);
                    };
                    Some(length)
                } else {
                    None
                };
                (
                    None,
                    self.basic(BasicKind::Byte).ok_or(Unknown)?,
                    Mode::Value,
                    bound,
                )
            }
            _ => return Err(Invalid(IndexError::NonIndexable)),
        };
        Ok(IndexProjection {
            key,
            element,
            mode,
            bound,
        })
    }

    pub(crate) fn index_value(&mut self, index: Operand, int_bits: u32) -> Option<Option<u64>> {
        self.checked_index_value(index, int_bits).ok()
    }

    pub(super) fn checked_index_value(
        &mut self,
        index: Operand,
        int_bits: u32,
    ) -> Result<Option<u64>, CheckError<IndexValueError>> {
        use IndexValueError::*;
        if !index.mode.is_value() {
            return Err(Unknown);
        }
        let root = self.underlying(index.ty).ok_or(Unknown)?;
        if matches!(self.node(root).ok_or(Unknown)?, Ty::TypeParam(_)) {
            if index.mode == Mode::Constant {
                return Err(Unknown);
            }
            return if self.integer_type(root).ok_or(Unknown)? {
                Ok(None)
            } else {
                Err(Invalid(NonInteger))
            };
        }
        let Ty::Basic(kind) = *self.node(root).ok_or(Unknown)? else {
            return Err(Invalid(NonInteger));
        };
        if kind.untyped() && kind.numeric_rank().is_none() {
            return Err(Invalid(ImplicitConversion(ConstAssignError::Incompatible)));
        }
        if index.mode != Mode::Constant {
            return if kind.integer() {
                Ok(None)
            } else {
                Err(Invalid(NonInteger))
            };
        }
        if !kind.integer() && !kind.untyped() {
            return Err(Invalid(NonInteger));
        }
        let constant = index.value.ok_or(Unknown)?;
        let value = constant.integer_value().ok_or_else(|| {
            if kind.untyped() && constant.definitely_fractional() {
                Invalid(ImplicitConversion(ConstAssignError::Truncated))
            } else {
                Unknown
            }
        })?;
        let fits = super::constant::representable(Constant::Int(value), BasicKind::Int, int_bits);
        if kind.untyped() && !fits {
            let reason = if kind.integer() {
                ConstAssignError::Overflow
            } else {
                ConstAssignError::Truncated
            };
            return Err(Invalid(ImplicitConversion(reason)));
        }
        if value < 0 {
            return Err(Invalid(Negative));
        }
        if !fits {
            return Err(Invalid(Overflow));
        }
        Ok(Some(u64::try_from(value).map_err(|_| Unknown)?))
    }

    pub(crate) fn is_array(&mut self, input: super::TypeId) -> Option<bool> {
        let root = self.underlying(input)?;
        Some(matches!(self.node(root)?, Ty::Array { .. }))
    }

    fn slice_root(&mut self, input: TypeId) -> Option<TypeId> {
        let root = self.underlying(input)?;
        if !matches!(self.node(root)?, Ty::TypeParam(_)) {
            return Some(root);
        }
        let terms = self.parameter_set(root)?.terms?;
        let mut common = None;
        let mut has_string = false;
        for term in terms {
            self.step()?;
            let mut ty = self.underlying(term.ty)?;
            if matches!(self.node(ty)?, Ty::Basic(kind) if kind.string()) {
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
    ) -> Result<SliceResult, CheckError<SliceError>> {
        self.step().ok_or(Unknown)?;
        if full && (indices[1].is_none() || indices[2].is_none()) || !full && indices[2].is_some() {
            return Err(Unknown);
        }
        let mut root = self.slice_root(input.ty).ok_or(Unknown)?;
        let pointer = if let Ty::Pointer(element) = *self.node(root).ok_or(Unknown)? {
            root = self.underlying(element).ok_or(Unknown)?;
            true
        } else {
            false
        };
        let (ty, bound) = match *self.node(root).ok_or(Unknown)? {
            Ty::Slice(_) if !pointer => (input.ty, None),
            Ty::Array { length, element } => {
                if !pointer && input.mode != Mode::Variable {
                    return Err(Invalid(SliceError::Unaddressable));
                }
                (self.push(Ty::Slice(element)).ok_or(Unknown)?, Some(length))
            }
            Ty::Basic(kind) if !pointer && kind.string() => {
                if full {
                    return Err(Invalid(SliceError::StringFull));
                }
                let bound = if input.mode == Mode::Constant {
                    let Some(Constant::String {
                        length: Some(length),
                    }) = input.value
                    else {
                        return Err(Unknown);
                    };
                    Some(length)
                } else {
                    None
                };
                let ty = if kind.untyped() {
                    self.basic(BasicKind::String).ok_or(Unknown)?
                } else {
                    input.ty
                };
                (ty, bound)
            }
            _ => return Err(Invalid(SliceError::NonSliceable)),
        };
        let limit = bound.and_then(|length| length.checked_add(1));
        let mut known = true;
        let mut errors = [None; 3];
        let mut values = [Some(0), bound, bound];
        for (position, index) in indices.into_iter().enumerate() {
            if let Some(index) = index {
                match self.bounded_index(index, limit, int_bits) {
                    Ok(value) => values[position] = value,
                    Err(failure) => {
                        values[position] = None;
                        match failure {
                            Invalid(error) => errors[position] = Some(error),
                            Unknown => known = false,
                        }
                    }
                }
            }
        }
        let mut swapped = None;
        'indices: for (i, low) in values.iter().enumerate() {
            for (j, high) in values.iter().enumerate().skip(i + 1) {
                if let (Some(low), Some(high)) = (low, high)
                    && low > high
                {
                    swapped = Some((j, *low, *high));
                    break 'indices;
                }
            }
        }
        Ok(SliceResult {
            operand: known.then_some(Operand {
                mode: Mode::Value,
                ty,
                value: None,
            }),
            errors,
            swapped,
        })
    }
}
