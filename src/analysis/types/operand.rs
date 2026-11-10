//! Bounded operands: match untyped types before operator checks and defaulting.
use super::{BasicKind, Ty, TypeArena, TypeId};
use crate::syntax::{Lexer, TokenKind};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    Constant,
    Variable,
    Value,
}

#[derive(Clone, Copy)]
pub(crate) enum Constant {
    Int(i128),
    Complex {
        real: super::complex::ComplexPart,
        imaginary: super::complex::ComplexPart,
    },
    Float {
        approximate: f64,
        integral: Option<i128>,
        rounded32: Option<f32>,
    },
    Boolean,
    String {
        length: Option<u64>,
    },
}

#[derive(Clone, Copy)]
pub(crate) struct Operand {
    pub(crate) mode: Mode,
    pub(crate) ty: TypeId,
    pub(crate) value: Option<Constant>,
}

// Package-value memo capacity is computed from its actual element size and
// still capped at 16 KiB; complex components must not grow unbounded payloads.
#[cfg(target_pointer_width = "64")]
const _: () = {
    assert!(std::mem::size_of::<Constant>() <= 80);
    assert!(std::mem::size_of::<Operand>() <= 96);
};

impl TypeArena<'_> {
    pub(crate) fn literal(&mut self, text: &str) -> Option<Operand> {
        if text.len() > super::MAX_BYTES {
            return None;
        }
        let token = Lexer::new(text).next()?;
        let (kind, value) = match token.kind {
            TokenKind::Int => (BasicKind::UntypedInt, integer(text).map(Constant::Int)),
            TokenKind::Char => (
                BasicKind::UntypedRune,
                crate::syntax::decode_rune_literal(text)
                    .map(|value| Constant::Int(i128::from(value))),
            ),
            TokenKind::Float => (BasicKind::UntypedFloat, float(text)),
            TokenKind::Imag => (
                BasicKind::UntypedComplex,
                float(text.strip_suffix('i')?)
                    .or_else(|| integer(text.strip_suffix('i')?).map(Constant::Int))
                    .and_then(super::complex::imaginary),
            ),
            TokenKind::String => (
                BasicKind::UntypedString,
                Some(Constant::String {
                    length: Some(crate::syntax::decode_string_literal(text)?.len() as u64),
                }),
            ),
            _ => return None,
        };
        Some(Operand {
            mode: Mode::Constant,
            ty: self.basic(kind)?,
            value,
        })
    }

    pub(crate) fn default_operand(&mut self, operand: Operand, int_bits: u32) -> Option<TypeId> {
        let Some(kind) = self.basic_kind(operand.ty) else {
            return Some(operand.ty);
        };
        if kind == BasicKind::UntypedNil {
            return None;
        }
        if !kind.untyped() {
            return Some(operand.ty);
        }
        let target = kind.default_type();
        if !target.boolean() {
            representable(operand.value?, target, int_bits).then_some(())?;
        }
        self.basic(target)
    }

    pub(crate) fn assign_basic(
        &mut self,
        mut operand: Operand,
        target: TypeId,
        int_bits: u32,
    ) -> Option<Operand> {
        let source = self.basic_kind(operand.ty)?;
        let target_kind = self.basic_kind(target)?;
        if !source.untyped() && !self.identical(operand.ty, target)? {
            return None;
        }
        if operand.mode == Mode::Constant {
            representable(operand.value?, target_kind, int_bits).then_some(())?;
            operand.value = Some(super::conversion::constant_value(
                operand.value?,
                target_kind,
            )?);
        }
        operand.ty = target;
        Some(operand)
    }

    pub(crate) fn indirect(&mut self, input: Operand) -> Option<Operand> {
        self.step()?;
        let root = self.common_under(input.ty)?;
        let Ty::Pointer(ty) = self.nodes[usize::from(root.0)] else {
            return None;
        };
        Some(Operand {
            mode: Mode::Variable,
            ty,
            value: None,
        })
    }

    pub(crate) fn unary(
        &mut self,
        operator: &str,
        mut operand: Operand,
        int_bits: u32,
    ) -> Option<Operand> {
        let kind = self.basic_kind(operand.ty)?;
        match operator {
            "+" | "-" if kind.numeric_rank().is_some() => {
                if operator == "-" {
                    operand.value = match operand.value {
                        Some(Constant::Int(value)) => value.checked_neg().map(Constant::Int),
                        Some(Constant::Float {
                            approximate,
                            integral,
                            rounded32,
                        }) => Some(Constant::Float {
                            approximate: -approximate,
                            integral: integral.and_then(i128::checked_neg),
                            rounded32: rounded32.map(|value| -value),
                        }),
                        Some(value @ Constant::Complex { .. }) => super::complex::negate(value),
                        _ => None,
                    };
                }
            }
            "^" if kind.integer() => {
                operand.value = match operand.value {
                    Some(Constant::Int(value)) => {
                        let mask = if let Some((false, bits)) = kind.integer_width(int_bits) {
                            (1i128 << bits) - 1
                        } else {
                            -1
                        };
                        Some(Constant::Int(value ^ mask))
                    }
                    _ => None,
                };
            }
            "!" if kind.boolean() => {}
            _ => return None,
        }
        if operand.mode == Mode::Constant && !kind.untyped() {
            representable(operand.value?, kind, int_bits).then_some(())?;
            if kind.numeric_rank() == Some(3) {
                operand.value = Some(super::conversion::constant_value(operand.value?, kind)?);
            }
        }
        if operand.mode != Mode::Constant {
            operand.mode = Mode::Value;
        }
        Some(operand)
    }

    pub(crate) fn binary(
        &mut self,
        operator: &str,
        mut left: Operand,
        mut right: Operand,
        int_bits: u32,
    ) -> Option<Operand> {
        let mut kind = self.basic_kind(left.ty)?;
        let other = self.basic_kind(right.ty)?;
        if matches!(operator, "<<" | ">>") {
            if !kind.integer() || !other.integer() {
                return None;
            }
            let Some(Constant::Int(shift)) = right.value else {
                return None;
            };
            if shift < 0 {
                return None;
            }
            // Unlike checked_shl, multiplication also detects lost sign bits.
            left.value = match left.value {
                Some(Constant::Int(value)) => u32::try_from(shift)
                    .ok()
                    .filter(|shift| *shift < 127)
                    .and_then(|shift| {
                        if operator == "<<" {
                            value.checked_mul(1i128 << shift)
                        } else {
                            Some(value >> shift)
                        }
                    })
                    .map(Constant::Int),
                _ => None,
            };
            if right.mode != Mode::Constant {
                return None;
            }
            if left.mode == Mode::Constant && !kind.untyped() {
                representable(left.value?, kind, int_bits).then_some(())?;
            }
            left.mode = if left.mode == Mode::Constant {
                Mode::Constant
            } else {
                Mode::Value
            };
            return Some(left);
        }
        // go/types matchTypes: only untyped operands may implicitly convert.
        if kind.untyped() && other.untyped() {
            if let (Some(a), Some(b)) = (kind.numeric_rank(), other.numeric_rank()) {
                if b > a {
                    kind = other;
                    left.ty = right.ty;
                }
            } else if kind != other {
                return None;
            }
        } else if kind.untyped() {
            representable(left.value?, other, int_bits).then_some(())?;
            if kind == BasicKind::UntypedComplex || other.numeric_rank() == Some(3) {
                left.value = Some(super::conversion::constant_value(left.value?, other)?);
            }
            kind = other;
            left.ty = right.ty;
        } else if other.untyped() {
            representable(right.value?, kind, int_bits).then_some(())?;
            if other == BasicKind::UntypedComplex || kind.numeric_rank() == Some(3) {
                right.value = Some(super::conversion::constant_value(right.value?, kind)?);
            }
            right.ty = left.ty;
        } else if !self.identical(left.ty, right.ty)? {
            return None;
        }
        let comparison = matches!(operator, "==" | "!=" | "<" | "<=" | ">" | ">=");
        let valid = match operator {
            "+" => kind.numeric_rank().is_some() || kind.string(),
            "-" | "*" | "/" => kind.numeric_rank().is_some(),
            "%" | "&" | "|" | "^" | "&^" => kind.integer(),
            "&&" | "||" => kind.boolean(),
            "==" | "!=" => true,
            "<" | "<=" | ">" | ">=" => {
                kind.string() || kind.numeric_rank().is_some_and(|rank| rank < 3)
            }
            _ => false,
        };
        if !valid {
            return None;
        }
        if matches!(operator, "/" | "%")
            && (left.mode == Mode::Constant || kind.integer())
            && right.mode == Mode::Constant
            && right.value.is_some_and(super::complex::is_zero)
        {
            return None;
        }
        let constant = left.mode == Mode::Constant && right.mode == Mode::Constant;
        let mut value = if comparison || kind.boolean() {
            Some(Constant::Boolean)
        } else if kind.string() {
            let length = match (left.value, right.value) {
                (
                    Some(Constant::String { length: Some(a) }),
                    Some(Constant::String { length: Some(b) }),
                ) => a.checked_add(b),
                _ => None,
            };
            Some(Constant::String { length })
        } else if kind.numeric_rank() == Some(3) {
            left.value
                .zip(right.value)
                .and_then(|(left, right)| super::complex::binary(operator, left, right))
        } else if let (Some(Constant::Int(a)), Some(Constant::Int(b))) = (left.value, right.value) {
            match operator {
                "+" => a.checked_add(b),
                "-" => a.checked_sub(b),
                "*" => a.checked_mul(b),
                "/" if kind.integer() => a.checked_div(b),
                "%" => a.checked_rem(b),
                "&" => Some(a & b),
                "|" => Some(a | b),
                "^" => Some(a ^ b),
                "&^" => Some(a & !b),
                _ => None,
            }
            .map(Constant::Int)
        } else {
            None
        };
        let ty = if comparison {
            self.basic(BasicKind::UntypedBool)?
        } else {
            left.ty
        };
        if constant && !kind.untyped() && !comparison {
            representable(value?, kind, int_bits).then_some(())?;
            if kind.numeric_rank() == Some(3) {
                value = Some(super::conversion::constant_value(value?, kind)?);
            }
        }
        Some(Operand {
            mode: if constant {
                Mode::Constant
            } else {
                Mode::Value
            },
            ty,
            value: constant.then_some(value).flatten(),
        })
    }
}

pub(super) fn representable(value: Constant, target: BasicKind, int_bits: u32) -> bool {
    if matches!(value, Constant::Complex { .. }) {
        return super::complex::representable(value, target, int_bits);
    }
    if let Some((signed, bits)) = target.integer_width(int_bits) {
        let value = match value {
            Constant::Int(value) => value,
            Constant::Float {
                integral: Some(value),
                ..
            } => value,
            _ => return false,
        };
        return if signed {
            -(1i128 << (bits - 1)) <= value && value < (1i128 << (bits - 1))
        } else {
            0 <= value && value < (1i128 << bits)
        };
    }
    match value {
        Constant::Complex { .. } => unreachable!("complex constants handled above"),
        Constant::Boolean => target.boolean(),
        Constant::String { .. } => target.string(),
        Constant::Int(value) => {
            matches!(
                target,
                BasicKind::Float32
                    | BasicKind::Float64
                    | BasicKind::Complex64
                    | BasicKind::Complex128
            ) && ((value as f64).is_finite())
        }
        Constant::Float { approximate, .. } => match target {
            // Avoid double-rounding overflow boundaries until exact rational
            // float representability is available.
            BasicKind::Float32 | BasicKind::Complex64 => {
                approximate.abs() < f64::from(f32::MAX) / 2.0
            }
            BasicKind::Float64 | BasicKind::Complex128 => approximate.is_finite(),
            _ => false,
        },
    }
}

fn integer(text: &str) -> Option<i128> {
    let (text, radix) = if text.starts_with("0x") || text.starts_with("0X") {
        (&text[2..], 16)
    } else if text.starts_with("0b") || text.starts_with("0B") {
        (&text[2..], 2)
    } else if text.starts_with("0o") || text.starts_with("0O") {
        (&text[2..], 8)
    } else if text.starts_with('0') && text.len() > 1 {
        (text, 8)
    } else {
        (text, 10)
    };
    text.chars()
        .filter(|character| *character != '_')
        .try_fold(0i128, |value, digit| {
            value
                .checked_mul(radix)?
                .checked_add(i128::from(digit.to_digit(radix as u32)?))
        })
}

fn float(text: &str) -> Option<Constant> {
    let text = text.replace('_', "");
    if text.starts_with("0x") || text.starts_with("0X") {
        return hex_float(&text);
    }
    let approximate = text.parse::<f64>().ok().filter(|value| value.is_finite())?;
    Some(Constant::Float {
        approximate,
        integral: decimal_integer(&text),
        rounded32: text.parse::<f32>().ok().filter(|value| value.is_finite()),
    })
}

/// Hexadecimal float literal (`0x1.8p1`): Go requires the binary `p` exponent.
/// The mantissa is scaled exactly by a power of two, so an integral result is a
/// proved fact; overflow/underflow stays unknown instead of an approximate value.
fn hex_float(text: &str) -> Option<Constant> {
    let rest = text
        .strip_prefix("0x")
        .or_else(|| text.strip_prefix("0X"))?;
    let (mantissa_text, exponent) = rest.split_once(['p', 'P'])?;
    let exponent = exponent.parse::<i32>().ok()?;
    let (integer_digits, fraction_digits) = mantissa_text
        .split_once('.')
        .map_or((mantissa_text, ""), |(integer, fraction)| {
            (integer, fraction)
        });
    if integer_digits.is_empty() && fraction_digits.is_empty() {
        return None;
    }
    if !integer_digits
        .bytes()
        .chain(fraction_digits.bytes())
        .all(|byte| byte.is_ascii_hexdigit())
    {
        return None;
    }
    let mut mantissa = 0u128;
    for digit in integer_digits.bytes().chain(fraction_digits.bytes()) {
        mantissa = mantissa
            .checked_mul(16)?
            .checked_add(u128::from((digit as char).to_digit(16)? as u8))?;
    }
    if mantissa > u128::MAX / 2 {
        return None;
    }
    let scale = exponent.checked_sub(i32::try_from(fraction_digits.len()).ok()?.checked_mul(4)?)?;
    let approximate = (mantissa as f64) * 2f64.powi(scale);
    if !approximate.is_finite() {
        return None;
    }
    let narrow = (mantissa as f32) * 2f32.powi(scale);
    Some(Constant::Float {
        approximate,
        integral: hexadecimal_integer(mantissa, scale),
        rounded32: narrow.is_finite().then_some(narrow),
    })
}

/// Prove the exact integer value of `mantissa * 2^scale` inside the bounded
/// integer precision; fractional or oversized values carry no integral fact.
fn hexadecimal_integer(mantissa: u128, scale: i32) -> Option<i128> {
    if mantissa == 0 {
        return Some(0);
    }
    let magnitude = if scale >= 0 {
        mantissa.checked_shl(u32::try_from(scale).ok()?)?
    } else {
        let shift = u32::try_from(-scale).ok()?;
        if shift >= 128 || mantissa & ((1u128 << shift) - 1) != 0 {
            return None;
        }
        mantissa >> shift
    };
    i128::try_from(magnitude).ok()
}

// Prove integrality from decimal digits, never from rounded binary float bits.
fn decimal_integer(text: &str) -> Option<i128> {
    let (mantissa, exponent) = text
        .split_once(['e', 'E'])
        .map_or(Some((text, 0i32)), |(mantissa, exponent)| {
            Some((mantissa, exponent.parse::<i32>().ok()?))
        })?;
    let fractional = mantissa
        .split_once('.')
        .map_or(0, |(_, fraction)| fraction.len());
    let digits: String = mantissa
        .chars()
        .filter(|character| *character != '.')
        .collect();
    let trimmed = digits.trim_start_matches('0');
    if trimmed.is_empty() {
        return Some(0);
    }
    let significant = trimmed.trim_end_matches('0');
    let trailing = trimmed.len() - significant.len();
    let scale = exponent
        .checked_sub(i32::try_from(fractional).ok()?)?
        .checked_add(i32::try_from(trailing).ok()?)?;
    if scale < 0 || significant.len() > 39 {
        return None;
    }
    significant
        .parse::<i128>()
        .ok()?
        .checked_mul(10i128.checked_pow(u32::try_from(scale).ok()?)?)
}
