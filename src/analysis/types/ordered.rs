//! min/max match every argument before folding bounded constant metadata.
use super::{BasicKind, Constant, Mode, Operand, Ty, TypeArena, TypeId};

pub(crate) enum MinMaxError {
    Unordered(usize),
    Mismatch { index: usize, previous: TypeId },
}

impl TypeArena<'_> {
    pub(crate) fn checked_builtin_minmax(
        &mut self,
        arguments: &[Operand],
        maximum: bool,
        int_bits: u32,
    ) -> Option<Result<Operand, MinMaxError>> {
        if arguments.is_empty() || arguments.len() > 256 {
            return None;
        }
        let mut target = None;
        for (index, argument) in arguments.iter().enumerate() {
            self.step()?;
            if !self.ordered_type(argument.ty)? {
                return Some(Err(MinMaxError::Unordered(index)));
            }
            if self
                .basic_kind(argument.ty)
                .is_none_or(|kind| !kind.untyped())
            {
                if let Some(previous) = target {
                    if !self.identical(previous, argument.ty)? {
                        return Some(Err(MinMaxError::Mismatch { index, previous }));
                    }
                } else {
                    target = Some(argument.ty);
                }
            }
        }
        let typed = target.is_some();
        let target = if let Some(target) = target {
            target
        } else {
            let mut target = arguments[0].ty;
            let mut kind = self.basic_kind(target)?;
            for argument in &arguments[1..] {
                let next = self.basic_kind(argument.ty)?;
                if kind.string() != next.string() {
                    return None;
                }
                if next.numeric_rank() > kind.numeric_rank() {
                    target = argument.ty;
                    kind = next;
                }
            }
            target
        };
        let kind = self.basic_kind(target);
        let mut actuals = Vec::new();
        for argument in arguments {
            self.step()?;
            let actual = if !typed {
                *argument
            } else if kind.is_some() {
                self.assign_basic(*argument, target, int_bits)?
            } else {
                self.identical(argument.ty, target)?.then_some(())?;
                *argument
            };
            actuals.push(actual);
        }
        let constant = actuals
            .iter()
            .all(|argument| argument.mode == Mode::Constant);
        let value = if !constant {
            None
        } else {
            let kind = kind?;
            if kind.string() {
                let mut length = None;
                for (index, argument) in actuals.iter().enumerate() {
                    let Some(Constant::String { length: next }) = argument.value else {
                        return None;
                    };
                    if index == 0 {
                        length = next;
                    } else if length != next {
                        length = None;
                    }
                }
                Some(Constant::String { length })
            } else if kind.integer() {
                let mut result: Option<super::bigint::ConstInt> = None;
                for argument in &actuals {
                    let value = argument.value.and_then(Constant::as_wide)?;
                    result = Some(result.map_or(value, |previous| {
                        if maximum {
                            previous.max(value)
                        } else {
                            previous.min(value)
                        }
                    }));
                }
                Some(Constant::from_wide(result?))
            } else {
                // Rounding is monotone: min/max of independently rounded values
                // preserves each rounding projection. Integral facts require all
                // candidate constants to have exact represented integral values.
                let mut result: Option<(f64, Option<f32>, Option<i128>)> = None;
                for argument in &actuals {
                    let next = numeric(argument.value?)?;
                    result = Some(match result {
                        None => next,
                        Some((approximate, narrow, integral)) => (
                            if maximum {
                                f64::max(approximate, next.0)
                            } else {
                                f64::min(approximate, next.0)
                            },
                            narrow.zip(next.1).map(|(a, b)| {
                                if maximum {
                                    f32::max(a, b)
                                } else {
                                    f32::min(a, b)
                                }
                            }),
                            integral.zip(next.2).map(|(a, b)| {
                                if maximum {
                                    i128::max(a, b)
                                } else {
                                    i128::min(a, b)
                                }
                            }),
                        ),
                    });
                }
                let (approximate, rounded32, integral) = result?;
                Some(Constant::Float {
                    approximate,
                    integral,
                    rounded32,
                    fractional: false,
                })
            }
        };
        Some(Ok(Operand {
            mode: if constant {
                Mode::Constant
            } else {
                Mode::Value
            },
            ty: target,
            value,
        }))
    }

    fn ordered_type(&mut self, input: TypeId) -> Option<bool> {
        let root = self.underlying(input)?;
        if matches!(self.node(root)?, Ty::TypeParam(_)) {
            let set = self.parameter_set(root)?;
            let Some(terms) = set.terms.as_slice() else {
                return Some(false);
            };
            if terms.is_empty() {
                return None;
            }
            for &term in terms {
                self.step()?;
                let root = self.underlying(term.ty)?;
                if !matches!(self.node(root)?, Ty::Basic(kind) if ordered(*kind)) {
                    return Some(false);
                }
            }
            Some(true)
        } else {
            Some(matches!(self.node(root)?, Ty::Basic(kind) if ordered(*kind)))
        }
    }
}

fn ordered(kind: BasicKind) -> bool {
    kind.string() || kind.numeric_rank().is_some_and(|rank| rank < 3)
}

fn numeric(value: Constant) -> Option<(f64, Option<f32>, Option<i128>)> {
    match value {
        Constant::Int(value) => Some((
            value as f64,
            Some(value as f32).filter(|value| value.is_finite()),
            Some(value),
        )),
        Constant::WideInt(limbs) => {
            let wide = super::bigint::ConstInt(limbs);
            Some((wide.to_f64(), wide.to_f32(), None))
        }
        Constant::Float {
            approximate,
            rounded32,
            integral,
            ..
        } => Some((approximate, rounded32, integral)),
        _ => None,
    }
}
