//! Bounded operands: match untyped types before operator checks and defaulting.
use super::bigint::ConstInt;
use super::constant::{
    ConstAssignError, Constant, constant_value, fold_integer_shift, parse_float_literal,
    parse_integer_literal, representable,
};
use super::{BasicKind, Ty, TypeArena, TypeId};
use crate::analysis::engine::obj::BuiltinFunction;
use crate::syntax::TokenKind;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    Invalid,
    NoValue,
    Builtin(BuiltinFunction),
    TypeExpr,
    Constant,
    Variable,
    Value,
    MapIndex,
    CommaOk,
}

impl Mode {
    pub(crate) const fn is_value(self) -> bool {
        matches!(
            self,
            Self::Constant | Self::Variable | Self::Value | Self::MapIndex | Self::CommaOk
        )
    }
}

#[derive(Clone, Copy)]
pub(crate) enum BinaryError {
    DivisionByZero,
    NegativeShift,
    ShiftOperandType,
    ShiftCountType,
    InvalidShiftCount,
    ShiftCountConversion(ConstAssignError),
    Overflow(Operand),
}

#[derive(Clone, Copy)]
pub(crate) struct Operand {
    pub(crate) mode: Mode,
    pub(crate) ty: TypeId,
    pub(crate) value: Option<Constant>,
}

impl Operand {
    /// Non-value operands carry the arena's permanent invalid type, never a
    /// guessed value type. Only builtins carry additional mode information.
    pub(crate) const fn non_value(mode: Mode) -> Self {
        Self {
            mode,
            ty: TypeId::INVALID,
            value: None,
        }
    }
}

// Package-value memo capacity is computed from its actual element size and
// still capped at 16 KiB; complex components must not grow unbounded payloads.
#[cfg(target_pointer_width = "64")]
const _: () = {
    assert!(std::mem::size_of::<Constant>() <= 80);
    assert!(std::mem::size_of::<Operand>() <= 96);
};

impl TypeArena<'_> {
    pub(crate) fn literal(&mut self, kind: TokenKind, text: &str) -> Option<Operand> {
        if text.len() > super::MAX_BYTES {
            return None;
        }
        let (kind, value) = match kind {
            TokenKind::Int => (BasicKind::UntypedInt, parse_integer_literal(text)),
            TokenKind::Char => (
                BasicKind::UntypedRune,
                crate::syntax::decode_rune_literal(text)
                    .map(|value| Constant::Int(i128::from(value))),
            ),
            TokenKind::Float => (BasicKind::UntypedFloat, parse_float_literal(text)),
            TokenKind::Imag => (
                BasicKind::UntypedComplex,
                parse_float_literal(text.strip_suffix('i')?)
                    .or_else(|| parse_integer_literal(text.strip_suffix('i')?))
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
        operand.mode.is_value().then_some(())?;
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
        operand.mode.is_value().then_some(())?;
        let source = self.basic_kind(operand.ty)?;
        let target_kind = self.basic_kind(target)?;
        if target_kind == BasicKind::UnsafePointer {
            if source != BasicKind::UnsafePointer && source != BasicKind::UntypedNil {
                return None;
            }
            operand.mode = Mode::Value;
            operand.ty = target;
            operand.value = None;
            return Some(operand);
        }
        if !source.untyped() && !self.identical(operand.ty, target)? {
            return None;
        }
        if operand.mode == Mode::Constant {
            representable(operand.value?, target_kind, int_bits).then_some(())?;
            operand.value = Some(constant_value(operand.value?, target_kind)?);
        }
        operand.ty = target;
        Some(operand)
    }

    /// An untyped constant assignment can fail without enough information to
    /// distinguish non-representability from an unavailable exact value.
    pub(super) fn const_assignment_error(
        &mut self,
        input: Operand,
        target: TypeId,
        int_bits: u32,
    ) -> Option<ConstAssignError> {
        if input.mode != Mode::Constant {
            return None;
        }
        let source = self.basic_kind(input.ty).filter(|kind| kind.untyped())?;
        let target = self.basic_kind(target)?;
        if source.string() && !target.string()
            || source.boolean() && !target.boolean()
            || source.numeric_rank().is_some() && target.numeric_rank().is_none()
        {
            return Some(ConstAssignError::Incompatible);
        }
        let value = input.value?;
        if target.integer() && !representable(value, target, int_bits) {
            if value.integer_value().is_some() {
                return Some(if source.integer() {
                    ConstAssignError::Overflow
                } else {
                    ConstAssignError::Truncated
                });
            }
            if value.definitely_fractional() {
                return Some(ConstAssignError::Truncated);
            }
        }
        None
    }

    pub(crate) fn indirect(&mut self, input: Operand) -> Option<Operand> {
        self.step()?;
        let root = self.common_under(input.ty)?;
        let Ty::Pointer(ty) = *self.node(root)? else {
            return None;
        };
        Some(Operand {
            mode: Mode::Variable,
            ty,
            value: None,
        })
    }

    pub(crate) fn unary_expr(
        &mut self,
        operator: &str,
        operand: Operand,
        int_bits: u32,
    ) -> Option<Operand> {
        if let Some(res) = self.unary(operator, operand, int_bits) {
            return Some(res);
        }
        let root = self.underlying(operand.ty)?;
        if !matches!(self.node(root), Some(Ty::TypeParam(_))) {
            return None;
        }
        let terms = self.parameter_set(root)?.terms?;
        if terms.is_empty() {
            return None;
        }
        for term in terms {
            self.step()?;
            let kind = self.basic_kind(term.ty)?;
            let valid = match operator {
                "+" | "-" => kind.numeric_rank().is_some(),
                "^" => kind.integer(),
                "!" => kind.boolean(),
                _ => false,
            };
            if !valid {
                return None;
            }
        }
        Some(Operand {
            mode: Mode::Value,
            ty: operand.ty,
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
                        Some(Constant::WideInt(limbs)) => {
                            ConstInt(limbs).checked_neg().map(Constant::from_wide)
                        }
                        Some(Constant::Float {
                            approximate,
                            integral,
                            rounded32,
                            fractional,
                        }) => Some(Constant::Float {
                            approximate: -approximate,
                            integral: integral.and_then(i128::checked_neg),
                            rounded32: rounded32.map(|value| -value),
                            fractional,
                        }),
                        Some(value @ Constant::Complex { .. }) => super::complex::negate(value),
                        _ => None,
                    };
                }
            }
            "^" if kind.integer() => {
                operand.value = match operand.value.and_then(Constant::as_wide) {
                    Some(value) => {
                        let res = if let Some((false, bits)) = kind.integer_width(int_bits) {
                            let mask = ConstInt::from_i128((1i128 << bits) - 1);
                            value.bitxor(mask)
                        } else {
                            value.bitnot()
                        };
                        Some(Constant::from_wide(res))
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
                operand.value = Some(constant_value(operand.value?, kind)?);
            }
        }
        if operand.mode != Mode::Constant {
            operand.mode = Mode::Value;
        }
        Some(operand)
    }

    pub(crate) fn binary_expr(
        &mut self,
        operator: &str,
        left: Operand,
        right: Operand,
        expected: Option<TypeId>,
        int_bits: u32,
    ) -> Option<Operand> {
        self.checked_binary_expr(operator, left, right, expected, int_bits)?
            .ok()
    }

    pub(crate) fn checked_binary_expr(
        &mut self,
        operator: &str,
        left: Operand,
        right: Operand,
        expected: Option<TypeId>,
        int_bits: u32,
    ) -> Option<Result<Operand, BinaryError>> {
        if matches!(operator, "<<" | ">>") {
            return self.check_shift(operator, left, right, expected, int_bits);
        }
        if let Some(res) = self.binary(operator, left, right, int_bits) {
            return Some(res);
        }

        if matches!(operator, "==" | "!=") {
            if self.basic_kind(left.ty) == Some(BasicKind::UntypedNil)
                && self.basic_kind(right.ty) == Some(BasicKind::UntypedNil)
            {
                return None;
            }
            let cmp_lr = self.assignable(left, right.ty, int_bits) == Some(true)
                && self.comparable(right.ty, 0) == Some(true);
            let cmp_rl = self.assignable(right, left.ty, int_bits) == Some(true)
                && self.comparable(left.ty, 0) == Some(true);
            if cmp_lr || cmp_rl {
                return Some(Ok(Operand {
                    mode: Mode::Value,
                    ty: self.basic(BasicKind::UntypedBool)?,
                    value: None,
                }));
            }
        }

        self.binary_param(operator, left, right, int_bits)
    }

    fn check_shift(
        &mut self,
        operator: &str,
        left: Operand,
        right: Operand,
        expected: Option<TypeId>,
        int_bits: u32,
    ) -> Option<Result<Operand, BinaryError>> {
        let left_kind = self.basic_kind(left.ty);
        let left_untyped = left_kind.is_some_and(BasicKind::untyped);
        let left_value = left.value.and_then(Constant::as_wide);
        if !(self.integer_type(left.ty)? || left_untyped && left_value.is_some()) {
            // A bounded constant without an exact integer projection can still
            // be valid. Only reject cases where the represented facts prove it.
            if left_untyped
                && left_kind?.numeric_rank().is_some()
                && !left.value.is_some_and(Constant::definitely_fractional)
            {
                return None;
            }
            return Some(Err(BinaryError::ShiftOperandType));
        }
        let right_kind = self.basic_kind(right.ty);
        let count = right.value.and_then(Constant::integer_value);
        if right.mode == Mode::Constant {
            if count.is_some_and(|count| count < 0) {
                return Some(Err(BinaryError::NegativeShift));
            }
            if right_kind.is_some_and(BasicKind::untyped) {
                let uint = self.basic(BasicKind::Uint)?;
                if self.assign_basic(right, uint, int_bits).is_none() {
                    let error = self.const_assignment_error(right, uint, int_bits)?;
                    return Some(Err(BinaryError::ShiftCountConversion(error)));
                }
            }
        } else if !self.integer_type(right.ty)? {
            return Some(Err(BinaryError::ShiftCountType));
        }

        if left.mode == Mode::Constant && right.mode == Mode::Constant {
            let count = if let Some(count) = count {
                count
            } else if right.value.is_some_and(Constant::definitely_fractional) {
                return Some(Err(BinaryError::InvalidShiftCount));
            } else {
                return None;
            };
            // go/types bounds constant shift counts independently of machine int.
            if count > 1023 - 1 + 52 {
                return Some(Err(BinaryError::InvalidShiftCount));
            }
            let count = u32::try_from(count).ok()?;
            let kind = left_kind?;
            let ty = if left_untyped && !kind.integer() {
                self.basic(BasicKind::UntypedInt)?
            } else {
                left.ty
            };
            let value = left_value.and_then(|value| fold_integer_shift(operator, value, count));
            let result = Operand {
                mode: Mode::Constant,
                ty,
                value,
            };
            if !left_untyped && !representable(value?, kind, int_bits) {
                return Some(Err(BinaryError::Overflow(result)));
            }
            return Some(Ok(result));
        }

        let ty = if left_untyped {
            self.contextual_shift_type(left, expected, int_bits)?
        } else {
            left.ty
        };
        Some(Ok(Operand {
            mode: Mode::Value,
            ty,
            value: None,
        }))
    }

    fn contextual_shift_type(
        &mut self,
        operand: Operand,
        expected: Option<TypeId>,
        int_bits: u32,
    ) -> Option<TypeId> {
        let target = match expected {
            Some(ty) if self.is_interface(ty) != Some(true) => ty,
            _ => self.basic(BasicKind::Int)?,
        };
        let root = self.underlying(target)?;
        if matches!(self.node(root), Some(Ty::TypeParam(_))) {
            let value = operand.value?;
            let terms = self.parameter_set(root)?.terms?;
            if terms.is_empty() {
                return None;
            }
            for term in terms {
                self.step()?;
                let kind = self.basic_kind(term.ty)?;
                if !kind.integer() || !representable(value, kind, int_bits) {
                    return None;
                }
            }
            return Some(target);
        }
        self.basic_kind(target)?.integer().then_some(())?;
        self.assign_basic(operand, target, int_bits).map(|op| op.ty)
    }

    pub(super) fn integer_type(&mut self, input: TypeId) -> Option<bool> {
        let root = self.underlying(input)?;
        if !matches!(self.node(root)?, Ty::TypeParam(_)) {
            return Some(matches!(
                self.node(root)?,
                Ty::Basic(kind) if kind.integer()
            ));
        }
        let Some(terms) = self.parameter_set(root)?.terms else {
            return Some(false);
        };
        if terms.is_empty() {
            return None;
        }
        for term in terms {
            self.step()?;
            let root = self.underlying(term.ty)?;
            if !matches!(self.node(root)?, Ty::Basic(kind) if kind.integer()) {
                return Some(false);
            }
        }
        Some(true)
    }

    fn binary_param(
        &mut self,
        operator: &str,
        left: Operand,
        right: Operand,
        int_bits: u32,
    ) -> Option<Result<Operand, BinaryError>> {
        let l_untyped = self.basic_kind(left.ty).filter(|k| k.untyped());
        let r_untyped = self.basic_kind(right.ty).filter(|k| k.untyped());

        let (param_ty, untyped_const) = if l_untyped.is_none() && r_untyped.is_none() {
            let root = self.underlying(left.ty)?;
            if !matches!(self.node(root), Some(Ty::TypeParam(_))) {
                return None;
            }
            if !self.identical(left.ty, right.ty)? {
                return None;
            }
            (left.ty, None)
        } else if r_untyped.is_some() && l_untyped.is_none() {
            let root = self.underlying(left.ty)?;
            if !matches!(self.node(root), Some(Ty::TypeParam(_))) {
                return None;
            }
            (left.ty, Some(right))
        } else if l_untyped.is_some() && r_untyped.is_none() {
            let root = self.underlying(right.ty)?;
            if !matches!(self.node(root), Some(Ty::TypeParam(_))) {
                return None;
            }
            (right.ty, Some(left))
        } else {
            return None;
        };

        let root = self.underlying(param_ty)?;
        let terms = self.parameter_set(root)?.terms?;
        if terms.is_empty() {
            return None;
        }
        let comparison = matches!(operator, "==" | "!=" | "<" | "<=" | ">" | ">=");
        let mut all_integer = true;
        for term in terms {
            self.step()?;
            let kind = self.basic_kind(term.ty);
            all_integer &= kind.is_some_and(BasicKind::integer);
            if let Some(uc) = untyped_const {
                if uc.mode != Mode::Constant {
                    return None;
                }
                let val = uc.value?;
                if !representable(val, kind?, int_bits) {
                    return None;
                }
            }
            let valid = match operator {
                "+" => kind.is_some_and(|k| k.numeric_rank().is_some() || k.string()),
                "-" | "*" | "/" => kind.is_some_and(|k| k.numeric_rank().is_some()),
                "%" | "&" | "|" | "^" | "&^" => kind.is_some_and(|k| k.integer()),
                "&&" | "||" => kind.is_some_and(|k| k.boolean()),
                "<" | "<=" | ">" | ">=" => {
                    kind.is_some_and(|k| k.string() || k.numeric_rank().is_some_and(|r| r < 3))
                }
                "==" | "!=" => self.comparable(term.ty, 0) == Some(true),
                _ => false,
            };
            if !valid {
                return None;
            }
        }
        if all_integer
            && matches!(operator, "/" | "%")
            && right.mode == Mode::Constant
            && right.value.is_some_and(super::complex::is_zero)
        {
            return Some(Err(BinaryError::DivisionByZero));
        }
        Some(Ok(Operand {
            mode: Mode::Value,
            ty: if comparison {
                self.basic(BasicKind::UntypedBool)?
            } else {
                param_ty
            },
            value: None,
        }))
    }

    fn binary(
        &mut self,
        operator: &str,
        mut left: Operand,
        mut right: Operand,
        int_bits: u32,
    ) -> Option<Result<Operand, BinaryError>> {
        let mut kind = self.basic_kind(left.ty)?;
        let other = self.basic_kind(right.ty)?;
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
                left.value = Some(constant_value(left.value?, other)?);
            }
            kind = other;
            left.ty = right.ty;
        } else if other.untyped() {
            representable(right.value?, kind, int_bits).then_some(())?;
            if other == BasicKind::UntypedComplex || kind.numeric_rank() == Some(3) {
                right.value = Some(constant_value(right.value?, kind)?);
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
            return Some(Err(BinaryError::DivisionByZero));
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
        } else if let (Some(a), Some(b)) = (
            left.value.and_then(Constant::as_wide),
            right.value.and_then(Constant::as_wide),
        ) {
            match operator {
                "+" => a.checked_add(b),
                "-" => a.checked_sub(b),
                "*" => a.checked_mul(b),
                "/" if kind.integer() => a.checked_div_rem(b).map(|(q, _)| q),
                "%" => a.checked_div_rem(b).map(|(_, r)| r),
                "&" => Some(a.bitand(b)),
                "|" => Some(a.bitor(b)),
                "^" => Some(a.bitxor(b)),
                "&^" => Some(a.bitand(b.bitnot())),
                _ => None,
            }
            .map(Constant::from_wide)
        } else {
            None
        };
        let ty = if comparison {
            self.basic(BasicKind::UntypedBool)?
        } else {
            left.ty
        };
        if constant && !kind.untyped() && !comparison {
            if !representable(value?, kind, int_bits) {
                return kind.integer().then_some(Err(BinaryError::Overflow(Operand {
                    mode: Mode::Constant,
                    ty,
                    value,
                })));
            }
            if kind.numeric_rank() == Some(3) {
                value = Some(constant_value(value?, kind)?);
            }
        }
        Some(Ok(Operand {
            mode: if constant {
                Mode::Constant
            } else {
                Mode::Value
            },
            ty,
            value: constant.then_some(value).flatten(),
        }))
    }
}
