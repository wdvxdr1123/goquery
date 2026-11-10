//! Bounded complex constants and the numeric builtin type functions.
use super::conversion;
use super::operand;
use super::{BasicKind, Constant, Mode, Operand, TypeArena};

/// Keep exact bounded integers, or independently parsed floating projections.
/// A rounded approximate zero never proves an exact zero or integer.
#[derive(Clone, Copy)]
pub(crate) enum ComplexPart {
    Integer(i128),
    Float {
        approximate: f64,
        rounded32: Option<f32>,
    },
}

impl ComplexPart {
    fn from_constant(value: Constant) -> Option<Self> {
        match value {
            Constant::Int(value) => Some(Self::Integer(value)),
            Constant::Float {
                integral: Some(value),
                ..
            } => Some(Self::Integer(value)),
            Constant::Float {
                approximate,
                rounded32,
                ..
            } if approximate.is_finite() => Some(Self::Float {
                approximate,
                rounded32,
            }),
            _ => None,
        }
    }

    /// Only exactly representable values enter approximate component arithmetic;
    /// a rounded projection never becomes a bounded integer fact.
    fn approximate(self) -> Option<f64> {
        match self {
            Self::Integer(value) => (value.unsigned_abs() <= (1u128 << 53)).then_some(value as f64),
            Self::Float { approximate, .. } => approximate.is_finite().then_some(approximate),
        }
    }

    fn from_approximate(value: f64) -> Option<Self> {
        if !value.is_finite() {
            return None;
        }
        Some(Self::Float {
            approximate: value,
            rounded32: Some(value as f32).filter(|rounded| rounded.is_finite()),
        })
    }

    pub(super) fn constant(self) -> Constant {
        match self {
            Self::Integer(value) => Constant::Int(value),
            Self::Float {
                approximate,
                rounded32,
            } => Constant::Float {
                approximate,
                integral: None,
                rounded32,
            },
        }
    }

    fn is_zero(self) -> bool {
        matches!(self, Self::Integer(0))
    }

    fn negate(self) -> Option<Self> {
        Some(match self {
            Self::Integer(value) => Self::Integer(value.checked_neg()?),
            Self::Float {
                approximate,
                rounded32,
            } => Self::Float {
                approximate: -approximate,
                rounded32: rounded32.map(|value| -value),
            },
        })
    }
}

pub(super) fn parts(value: Constant) -> Option<(ComplexPart, ComplexPart)> {
    match value {
        Constant::Complex { real, imaginary } => Some((real, imaginary)),
        value => Some((ComplexPart::from_constant(value)?, ComplexPart::Integer(0))),
    }
}

pub(super) fn real(value: Constant) -> Option<ComplexPart> {
    let (real, imaginary) = parts(value)?;
    imaginary.is_zero().then_some(real)
}

pub(super) fn imaginary(value: Constant) -> Option<Constant> {
    Some(Constant::Complex {
        real: ComplexPart::Integer(0),
        imaginary: ComplexPart::from_constant(value)?,
    })
}

pub(super) fn is_zero(value: Constant) -> bool {
    parts(value).is_some_and(|(real, imaginary)| real.is_zero() && imaginary.is_zero())
}

pub(super) fn negate(value: Constant) -> Option<Constant> {
    let (real, imaginary) = parts(value)?;
    Some(Constant::Complex {
        real: real.negate()?,
        imaginary: imaginary.negate()?,
    })
}

/// Exact component arithmetic first; fraction components fall back to the same
/// rounded projection ordinary float literals already carry, and any inexact or
/// non-finite result stays unknown. No approximate component ever proves zero or
/// an integral value.
pub(super) fn binary(operator: &str, left: Constant, right: Constant) -> Option<Constant> {
    let (a, b) = parts(left)?;
    let (c, d) = parts(right)?;
    if let (
        ComplexPart::Integer(a),
        ComplexPart::Integer(b),
        ComplexPart::Integer(c),
        ComplexPart::Integer(d),
    ) = (a, b, c, d)
    {
        if let Some(value) = integer_binary(operator, a, b, c, d) {
            return Some(value);
        }
        // Inexact division or overflow may fall through only when f64 holds every
        // input exactly; larger bounded integers stay unknown.
        if [a, b, c, d]
            .iter()
            .any(|value| value.unsigned_abs() > (1u128 << 53))
        {
            return None;
        }
    }
    let (a, b, c, d) = (
        a.approximate()?,
        b.approximate()?,
        c.approximate()?,
        d.approximate()?,
    );
    let (real, imaginary) = match operator {
        "+" => (a + c, b + d),
        "-" => (a - c, b - d),
        "*" => (a * c - b * d, a * d + b * c),
        "/" => {
            let denominator = c * c + d * d;
            if denominator == 0.0 || !denominator.is_finite() {
                return None;
            }
            ((a * c + b * d) / denominator, (b * c - a * d) / denominator)
        }
        _ => return None,
    };
    Some(Constant::Complex {
        real: ComplexPart::from_approximate(real)?,
        imaginary: ComplexPart::from_approximate(imaginary)?,
    })
}

/// Exact bounded integer components. Division only succeeds when both resulting
/// components are integral; otherwise the caller may round as Go does at the
/// value boundary, but never claims an exact fraction or an overflowed value.
fn integer_binary(operator: &str, a: i128, b: i128, c: i128, d: i128) -> Option<Constant> {
    let (real, imaginary) = match operator {
        "+" => (a.checked_add(c)?, b.checked_add(d)?),
        "-" => (a.checked_sub(c)?, b.checked_sub(d)?),
        "*" => (
            a.checked_mul(c)?.checked_sub(b.checked_mul(d)?)?,
            a.checked_mul(d)?.checked_add(b.checked_mul(c)?)?,
        ),
        "/" => {
            let denominator = c.checked_mul(c)?.checked_add(d.checked_mul(d)?)?;
            let real = a.checked_mul(c)?.checked_add(b.checked_mul(d)?)?;
            let imaginary = b.checked_mul(c)?.checked_sub(a.checked_mul(d)?)?;
            if denominator == 0
                || real.checked_rem(denominator)? != 0
                || imaginary.checked_rem(denominator)? != 0
            {
                return None;
            }
            (
                real.checked_div(denominator)?,
                imaginary.checked_div(denominator)?,
            )
        }
        _ => return None,
    };
    Some(Constant::Complex {
        real: ComplexPart::Integer(real),
        imaginary: ComplexPart::Integer(imaginary),
    })
}

pub(super) fn representable(value: Constant, target: BasicKind, int_bits: u32) -> bool {
    let Some((real, imaginary)) = parts(value) else {
        return false;
    };
    let component = match target {
        BasicKind::Complex64 => BasicKind::Float32,
        BasicKind::Complex128 => BasicKind::Float64,
        _ => {
            return imaginary.is_zero()
                && operand::representable(real.constant(), target, int_bits);
        }
    };
    operand::representable(real.constant(), component, int_bits)
        && operand::representable(imaginary.constant(), component, int_bits)
}

pub(super) fn converted(value: Constant, target: BasicKind) -> Option<Constant> {
    let (real, imaginary) = parts(value)?;
    let component = match target {
        BasicKind::Complex64 => BasicKind::Float32,
        BasicKind::Complex128 => BasicKind::Float64,
        _ => return None,
    };
    Some(Constant::Complex {
        real: ComplexPart::from_constant(conversion::constant_value(real.constant(), component)?)?,
        imaginary: ComplexPart::from_constant(conversion::constant_value(
            imaginary.constant(),
            component,
        )?)?,
    })
}

impl TypeArena<'_> {
    pub(crate) fn builtin_complex(
        &mut self,
        arguments: &[Operand],
        int_bits: u32,
    ) -> Option<Operand> {
        self.step()?;
        let [mut left, mut right] = *arguments else {
            return None;
        };
        let a = self.basic_kind(left.ty)?;
        let b = self.basic_kind(right.ty)?;
        let kind = match (a.untyped(), b.untyped()) {
            (true, true) => {
                if a.numeric_rank().is_none()
                    || b.numeric_rank().is_none()
                    || left.mode != Mode::Constant
                    || right.mode != Mode::Constant
                {
                    return None;
                }
                // go/types first converts numeric constants with zero imaginary
                // parts to untyped float, without defaulting them to float64.
                real(left.value?)?;
                real(right.value?)?;
                BasicKind::UntypedFloat
            }
            (true, false) => {
                left = self.assign_basic(left, right.ty, int_bits)?;
                b
            }
            (false, true) => {
                right = self.assign_basic(right, left.ty, int_bits)?;
                a
            }
            (false, false) => {
                self.identical(left.ty, right.ty)?.then_some(())?;
                a
            }
        };
        let result = match kind {
            BasicKind::Float32 => BasicKind::Complex64,
            BasicKind::Float64 => BasicKind::Complex128,
            BasicKind::UntypedFloat => BasicKind::UntypedComplex,
            _ => return None,
        };
        let constant = left.mode == Mode::Constant && right.mode == Mode::Constant;
        Some(Operand {
            mode: if constant {
                Mode::Constant
            } else {
                Mode::Value
            },
            ty: self.basic(result)?,
            value: if constant {
                Some(Constant::Complex {
                    real: real(left.value?)?,
                    imaginary: real(right.value?)?,
                })
            } else {
                None
            },
        })
    }

    pub(crate) fn builtin_component(&mut self, input: Operand, imaginary: bool) -> Option<Operand> {
        self.step()?;
        let kind = self.basic_kind(input.ty)?;
        let target = match kind {
            BasicKind::Complex64 => BasicKind::Float32,
            BasicKind::Complex128 => BasicKind::Float64,
            kind if kind.untyped()
                && kind.numeric_rank().is_some()
                && input.mode == Mode::Constant =>
            {
                BasicKind::UntypedFloat
            }
            _ => return None,
        };
        let value = if input.mode == Mode::Constant {
            let (real, imag) = parts(input.value?)?;
            Some(if imaginary { imag } else { real }.constant())
        } else {
            None
        };
        Some(Operand {
            mode: if input.mode == Mode::Constant {
                Mode::Constant
            } else {
                Mode::Value
            },
            ty: self.basic(target)?,
            value,
        })
    }
}
