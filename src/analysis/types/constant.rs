//! Bounded constant values, representability, and literal parsing.
use super::BasicKind;
use super::bigint::ConstInt;

#[derive(Clone, Copy)]
pub(crate) enum Constant {
    Int(i128),
    WideInt([u64; 8]),
    Complex {
        real: super::complex::ComplexPart,
        imaginary: super::complex::ComplexPart,
    },
    Float {
        approximate: f64,
        integral: Option<i128>,
        rounded32: Option<f32>,
        fractional: bool,
    },
    Boolean,
    String {
        length: Option<u64>,
    },
}

impl Constant {
    pub(super) fn from_wide(value: ConstInt) -> Self {
        match value.to_i128() {
            Some(small) => Self::Int(small),
            None => Self::WideInt(value.0),
        }
    }

    pub(crate) fn as_wide(self) -> Option<ConstInt> {
        match self {
            Self::Int(value)
            | Self::Float {
                integral: Some(value),
                ..
            } => Some(ConstInt::from_i128(value)),
            Self::WideInt(limbs) => Some(ConstInt(limbs)),
            Self::Complex { .. } => super::complex::real(self)?.as_wide(),
            _ => None,
        }
    }

    pub(crate) fn format_int(self) -> Option<String> {
        Some(self.as_wide()?.to_string())
    }

    pub(super) fn integer_value(self) -> Option<i128> {
        self.as_wide()?.to_i128()
    }

    pub(super) fn definitely_fractional(self) -> bool {
        matches!(
            self,
            Self::Float {
                fractional: true,
                ..
            }
        )
    }
}

#[derive(Clone, Copy)]
pub(crate) enum ConstAssignError {
    Incompatible,
    Overflow,
    Truncated,
}

pub(super) fn representable(value: Constant, target: BasicKind, int_bits: u32) -> bool {
    if matches!(value, Constant::Complex { .. }) {
        return super::complex::representable(value, target, int_bits);
    }
    if let Some((signed, bits)) = target.integer_width(int_bits) {
        let Some(value) = value.integer_value() else {
            return false;
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
        Constant::Int(value) => match target {
            BasicKind::Float32 | BasicKind::Complex64 => (value as f32).is_finite(),
            BasicKind::Float64 | BasicKind::Complex128 => (value as f64).is_finite(),
            _ => false,
        },
        Constant::WideInt(limbs) => match target {
            BasicKind::Float32 | BasicKind::Complex64 => ConstInt(limbs).to_f32().is_some(),
            BasicKind::Float64 | BasicKind::Complex128 => ConstInt(limbs).to_f64().is_finite(),
            _ => false,
        },
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

pub(super) fn constant_value(mut value: Constant, target: BasicKind) -> Option<Constant> {
    if matches!(target, BasicKind::Complex64 | BasicKind::Complex128) {
        return super::complex::converted(value, target);
    }
    if matches!(value, Constant::Complex { .. }) {
        value = super::complex::real(value)?.constant();
    }
    if target.integer() {
        return value.integer_value().map(Constant::Int);
    }
    if matches!(
        target,
        BasicKind::Float32 | BasicKind::Float64 | BasicKind::Complex64 | BasicKind::Complex128
    ) {
        let narrow = matches!(target, BasicKind::Float32 | BasicKind::Complex64);
        let fractional = value.definitely_fractional();
        let value = match value {
            Constant::Int(value) => {
                if narrow {
                    f64::from(value as f32)
                } else {
                    value as f64
                }
            }
            Constant::WideInt(limbs) => {
                let wide = ConstInt(limbs);
                if narrow {
                    f64::from(wide.to_f32()?)
                } else {
                    wide.to_f64()
                }
            }
            Constant::Float {
                approximate,
                rounded32,
                ..
            } => {
                if narrow {
                    f64::from(rounded32?)
                } else {
                    approximate
                }
            }
            _ => return None,
        };
        if !value.is_finite() {
            return None;
        }
        return Some(Constant::Float {
            approximate: value,
            integral: binary_integer(value),
            rounded32: Some(value as f32).filter(|value| value.is_finite()),
            fractional: fractional && value.fract() != 0.0,
        });
    }
    Some(value)
}

// Prove the exact integral value of the rounded binary float, without a
// saturating float-to-i128 cast at the representability boundary.
fn binary_integer(value: f64) -> Option<i128> {
    if value == 0.0 {
        return Some(0);
    }
    let bits = value.to_bits();
    let exponent = i32::try_from((bits >> 52) & 0x7ff).ok()? - 1023;
    if !(0..128).contains(&exponent) {
        return None;
    }
    let mantissa = u128::from((bits & ((1_u64 << 52) - 1)) | (1_u64 << 52));
    let magnitude = if exponent >= 52 {
        mantissa.checked_shl(u32::try_from(exponent - 52).ok()?)?
    } else {
        let shift = u32::try_from(52 - exponent).ok()?;
        if mantissa & ((1_u128 << shift) - 1) != 0 {
            return None;
        }
        mantissa >> shift
    };
    if bits >> 63 != 0 {
        if magnitude == 1_u128 << 127 {
            Some(i128::MIN)
        } else {
            i128::try_from(magnitude).ok()?.checked_neg()
        }
    } else {
        i128::try_from(magnitude).ok()
    }
}

pub(super) fn fold_integer_shift(operator: &str, value: ConstInt, count: u32) -> Option<Constant> {
    let shifted = if operator == ">>" {
        value.checked_shr(count)?
    } else {
        value.checked_shl(count)?
    };
    Some(Constant::from_wide(shifted))
}

pub(super) fn parse_integer_literal(text: &str) -> Option<Constant> {
    let (text, radix) = if text.starts_with("0x") || text.starts_with("0X") {
        (&text[2..], 16u128)
    } else if text.starts_with("0b") || text.starts_with("0B") {
        (&text[2..], 2u128)
    } else if text.starts_with("0o") || text.starts_with("0O") {
        (&text[2..], 8u128)
    } else if text.starts_with('0') && text.len() > 1 {
        (text, 8u128)
    } else {
        (text, 10u128)
    };
    let radix_int = ConstInt::from_i128(radix as i128);
    let mut acc = ConstInt::ZERO;
    for character in text.chars().filter(|character| *character != '_') {
        let digit = ConstInt::from_i128(i128::from(character.to_digit(radix as u32)?));
        acc = acc.checked_mul(radix_int)?.checked_add(digit)?;
    }
    Some(Constant::from_wide(acc))
}

pub(super) fn parse_float_literal(text: &str) -> Option<Constant> {
    let text = text.replace('_', "");
    if text.starts_with("0x") || text.starts_with("0X") {
        return hex_float(&text);
    }
    let approximate = text.parse::<f64>().ok().filter(|value| value.is_finite())?;
    Some(Constant::Float {
        approximate,
        integral: decimal_integer(&text),
        rounded32: text.parse::<f32>().ok().filter(|value| value.is_finite()),
        fractional: approximate.fract() != 0.0,
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
        fractional: approximate.fract() != 0.0,
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
