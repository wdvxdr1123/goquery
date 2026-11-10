//! Go conversions over represented types; constant rounding stays explicit.
use super::operand;
use super::{BasicKind, Constant, Direction, Mode, Operand, Tags, Ty, TypeArena, TypeId};

impl TypeArena<'_> {
    pub(crate) fn convert(
        &mut self,
        input: Operand,
        target: TypeId,
        int_bits: u32,
    ) -> Option<Operand> {
        self.underlying(target)?;
        self.underlying(input.ty)?;
        let source_kind = self.basic_kind(input.ty);
        if source_kind == Some(BasicKind::UntypedNil) {
            self.nilable(target)?.then_some(())?;
            return Some(Operand {
                mode: Mode::Value,
                ty: target,
                value: None,
            });
        }
        if self.is_interface(target)? {
            if !self.assignable(input, target, int_bits)? && !self.convertible(input.ty, target)? {
                return None;
            }
            return Some(Operand {
                mode: Mode::Value,
                ty: target,
                value: None,
            });
        }
        let target_kind = self.basic_kind(target);
        if input.mode == Mode::Constant
            && let Some(kind) = target_kind
        {
            let source = source_kind?;
            let value = if source.integer() && kind.string() {
                let Constant::Int(value) = input.value? else {
                    return None;
                };
                let rune = u32::try_from(value)
                    .ok()
                    .and_then(char::from_u32)
                    .unwrap_or(char::REPLACEMENT_CHARACTER);
                Constant::String {
                    length: Some(rune.len_utf8() as u64),
                }
            } else {
                operand::representable(input.value?, kind, int_bits).then_some(())?;
                constant_value(input.value?, kind)?
            };
            return Some(Operand {
                mode: Mode::Constant,
                ty: target,
                value: Some(value),
            });
        }
        self.convertible(input.ty, target)?.then_some(Operand {
            mode: Mode::Value,
            ty: target,
            value: None,
        })
    }

    pub(super) fn nilable(&mut self, target: TypeId) -> Option<bool> {
        let target = self.underlying(target)?;
        Some(matches!(
            self.nodes[usize::from(target.0)],
            Ty::Pointer(_)
                | Ty::Slice(_)
                | Ty::Map { .. }
                | Ty::Chan { .. }
                | Ty::Signature { .. }
                | Ty::Interface { .. }
        ))
    }

    fn convertible(&mut self, source: TypeId, target: TypeId) -> Option<bool> {
        if self.identical(source, target)? {
            return Some(true);
        }
        let (s, t) = (self.underlying(source)?, self.underlying(target)?);
        if self.identical_with_tags(s, t, Tags::Ignore)? {
            return Some(true);
        }
        let (sv, tv) = (self.unalias(source)?, self.unalias(target)?);
        if let (Ty::Pointer(a), Ty::Pointer(b)) = (
            self.nodes[usize::from(sv.0)].clone(),
            self.nodes[usize::from(tv.0)].clone(),
        ) {
            let (a, b) = (self.underlying(a)?, self.underlying(b)?);
            if self.identical_with_tags(a, b, Tags::Ignore)? {
                return Some(true);
            }
        }
        if let (Some(a), Some(b)) = (self.basic_kind(s), self.basic_kind(t)) {
            if let (Some(a), Some(b)) = (a.numeric_rank(), b.numeric_rank()) {
                return Some(a <= 2 && b <= 2 || a == 3 && b == 3);
            }
            if a.integer() && b.string() {
                return Some(true);
            }
        }
        if self.basic_kind(s).is_some_and(BasicKind::string) && self.bytes_or_runes(t)?
            || self.bytes_or_runes(s)? && self.basic_kind(t).is_some_and(BasicKind::string)
        {
            return Some(true);
        }
        match (
            self.nodes[usize::from(s.0)].clone(),
            self.nodes[usize::from(t.0)].clone(),
        ) {
            (
                Ty::Chan {
                    direction: Direction::Both,
                    element: a,
                },
                Ty::Chan { element: b, .. },
            ) if !matches!(self.nodes[usize::from(sv.0)], Ty::Named { .. })
                || !matches!(self.nodes[usize::from(tv.0)], Ty::Named { .. }) =>
            {
                self.identical(a, b)
            }
            (
                Ty::Slice(element),
                Ty::Array {
                    element: target, ..
                },
            ) => self.identical(element, target),
            (Ty::Slice(element), Ty::Pointer(array)) => {
                let array = self.underlying(array)?;
                let Ty::Array {
                    element: target, ..
                } = self.nodes[usize::from(array.0)]
                else {
                    return Some(false);
                };
                self.identical(element, target)
            }
            _ => Some(false),
        }
    }

    fn bytes_or_runes(&mut self, input: TypeId) -> Option<bool> {
        let input = self.underlying(input)?;
        let Ty::Slice(element) = self.nodes[usize::from(input.0)] else {
            return Some(false);
        };
        Some(
            self.basic_kind(element).is_some_and(|kind| {
                matches!(kind.canonical(), BasicKind::Uint8 | BasicKind::Int32)
            }),
        )
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
        return match value {
            Constant::Float {
                integral: Some(value),
                ..
            } => Some(Constant::Int(value)),
            value => Some(value),
        };
    }
    if matches!(
        target,
        BasicKind::Float32 | BasicKind::Float64 | BasicKind::Complex64 | BasicKind::Complex128
    ) {
        let narrow = matches!(target, BasicKind::Float32 | BasicKind::Complex64);
        let value = match value {
            Constant::Int(value) => {
                if narrow {
                    f64::from(value as f32)
                } else {
                    value as f64
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
