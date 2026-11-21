//! Render established operand failures at their parsed source ranges.
use super::{Code, RegionChecker};
use crate::analysis::engine::types::{
    AssignmentError, BasicKind, BinaryError, CheckError, ConstAssignError, Constant,
    ConversionError, IndexError, IndexValueError, Mode, Operand, TypeId,
};
use crate::syntax::ExprId;
use std::ops::Range;

impl RegionChecker<'_, '_> {
    pub(in crate::analysis::engine::check) fn check_assignment(
        &mut self,
        source: Operand,
        target: TypeId,
        expr: ExprId,
        context: &str,
    ) -> Option<()> {
        let int_bits = self.int_bits()?;
        let result = self.arena.check_assignment(source, target, int_bits);
        match result {
            Ok(()) => self.update_expr_type(expr, target, int_bits),
            Err(CheckError::Invalid(reason)) => {
                self.assignment_error(source, target, expr, context, reason);
            }
            Err(CheckError::Unknown) => {}
        }
        result.ok()
    }

    pub(in crate::analysis::engine::check) fn assignment_error(
        &mut self,
        source: Operand,
        target: TypeId,
        expr: ExprId,
        context: &str,
        reason: AssignmentError,
    ) {
        if !source.mode.is_value()
            || !self.body.diagnostics().is_empty()
            || self.arena.validate_type(source.ty).is_none()
            || self.arena.validate_type(target).is_none()
        {
            return;
        }
        let (code, suffix) = match reason {
            AssignmentError::ImplicitConstant(ConstAssignError::Overflow) => {
                (Code::NumericOverflow, " (overflows)")
            }
            AssignmentError::ImplicitConstant(ConstAssignError::Truncated) => {
                (Code::TruncatedFloat, " (truncated)")
            }
            AssignmentError::ImplicitConstant(ConstAssignError::Incompatible)
            | AssignmentError::TypeMismatch => (Code::IncompatibleAssign, ""),
            AssignmentError::InterfaceMismatch => {
                // Untyped operands that default to a method-incompatible type
                // need their own Go diagnostic wording.
                if self
                    .arena
                    .basic_kind(source.ty)
                    .is_some_and(|kind| kind.untyped() && kind != BasicKind::UntypedNil)
                {
                    return;
                }
                (Code::IncompatibleAssign, "")
            }
        };
        let Some(expression) = self.body.expression(expr) else {
            return;
        };
        let Some(from) = self.operand_description(expr, source) else {
            return;
        };
        let Some(to) = self.arena.render_presentation(target, self.file) else {
            return;
        };
        self.compiler_error(
            &expression.span,
            code,
            &format!("cannot use {from} as {to} value in {context}{suffix}"),
        );
    }

    pub(in crate::analysis::engine::check) fn operand_description(
        &mut self,
        id: ExprId,
        operand: Operand,
    ) -> Option<String> {
        let span = &self.body.expression(id)?.span;
        self.operand_description_at(span, operand)
    }

    fn operand_description_at(&mut self, span: &Range<usize>, operand: Operand) -> Option<String> {
        if span.len() > 1024 {
            return None;
        }
        let text = self.text(span);
        let kind = self.arena.basic_kind(operand.ty);
        if kind == Some(BasicKind::UntypedNil) {
            return Some("nil".to_owned());
        }
        let (prefix, suffix) = if let Some(kind) = kind.filter(|kind| kind.untyped()) {
            (format!("{} ", kind.name()), String::new())
        } else {
            let ty = self.arena.render_presentation(operand.ty, self.file)?;
            (String::new(), format!(" of type {ty}"))
        };
        let mode = match operand.mode {
            Mode::Constant => "constant",
            Mode::Variable => "variable",
            Mode::MapIndex => "map index expression",
            Mode::CommaOk => "comma, ok expression",
            Mode::Value => "value",
            _ => return None,
        };
        let value = match operand.value {
            Some(value @ (Constant::Int(_) | Constant::WideInt(_))) => {
                value.format_int().unwrap_or_default()
            }
            Some(Constant::Float { approximate, .. }) => approximate.to_string(),
            _ => String::new(),
        };
        let value = if value.is_empty() || value == text {
            String::new()
        } else {
            format!(" {value}")
        };
        Some(format!("{text} ({prefix}{mode}{value}{suffix})"))
    }

    pub(in crate::analysis::engine::check) fn index_value_error(
        &mut self,
        id: ExprId,
        mut operand: Operand,
        error: IndexValueError,
    ) {
        let Some(expr) = self.body.expression(id) else {
            return;
        };
        if matches!(error, IndexValueError::Negative)
            && self
                .arena
                .basic_kind(operand.ty)
                .is_some_and(BasicKind::untyped)
        {
            let Some(ty) = self.arena.builtin_type(self.file, "int") else {
                return;
            };
            operand.ty = ty;
        }
        let Some(value) = self.operand_description(id, operand) else {
            return;
        };
        let (code, message) = match error {
            IndexValueError::NonInteger => (
                Code::InvalidIndex,
                format!("invalid argument: index {value} must be integer"),
            ),
            IndexValueError::Negative => (
                Code::InvalidIndex,
                format!("invalid argument: index {value} must not be negative"),
            ),
            IndexValueError::Overflow => (
                Code::InvalidIndex,
                format!("invalid argument: index {value} overflows int"),
            ),
            IndexValueError::ImplicitConversion(error) => implicit_conversion(error, &value, "int"),
        };
        self.compiler_error(&expr.span, code, &message);
    }

    pub(in crate::analysis::engine::check) fn index_error(
        &mut self,
        container: (ExprId, Operand),
        index: (ExprId, Operand),
        error: IndexError,
    ) {
        let (id, key) = index;
        match error {
            IndexError::NonIndexable => {
                let (id, operand) = container;
                if let Some(expr) = self.body.expression(id)
                    && let Some(value) = self.operand_description(id, operand)
                {
                    self.compiler_error(
                        &expr.span,
                        Code::NonIndexableOperand,
                        &format!("cannot index {value}"),
                    );
                }
            }
            IndexError::Key { target, reason } => {
                self.assignment_error(key, target, id, "map index", reason)
            }
            IndexError::Value(error) => self.index_value_error(id, key, error),
            IndexError::Bounds { index, length } => {
                if let Some(expr) = self.body.expression(id) {
                    self.compiler_error(
                        &expr.span,
                        Code::InvalidIndex,
                        &format!("invalid argument: index {index} out of bounds [0:{length}]"),
                    );
                }
            }
        }
    }

    pub(in crate::analysis::engine::check) fn binary_error(
        &mut self,
        span: &Range<usize>,
        left: (ExprId, Operand),
        right: (ExprId, Operand),
        error: BinaryError,
    ) {
        match error {
            BinaryError::Overflow(result) => {
                let value = self.operand_description_at(span, result);
                let kind = self.arena.basic_kind(result.ty);
                if let (Some(value), Some(kind)) = (value, kind) {
                    self.compiler_error(
                        span,
                        Code::NumericOverflow,
                        &format!("{value} overflows {}", kind.name()),
                    );
                }
                return;
            }
            BinaryError::DivisionByZero => {
                if let Some(expr) = self.body.expression(right.0) {
                    self.compiler_error(
                        &expr.span,
                        Code::DivByZero,
                        "invalid operation: division by zero",
                    );
                }
                return;
            }
            _ => {}
        }
        let (id, operand) = if matches!(error, BinaryError::ShiftOperandType) {
            left
        } else {
            right
        };
        let Some(expr) = self.body.expression(id) else {
            return;
        };
        let Some(value) = self.operand_description(id, operand) else {
            return;
        };
        let (code, message) = match error {
            BinaryError::NegativeShift => (
                Code::InvalidShiftCount,
                format!("invalid operation: negative shift count {value}"),
            ),
            BinaryError::ShiftOperandType => (
                Code::InvalidShiftOperand,
                format!("invalid operation: shifted operand {value} must be integer"),
            ),
            BinaryError::ShiftCountType => (
                Code::InvalidShiftCount,
                format!("invalid operation: shift count {value} must be integer"),
            ),
            BinaryError::InvalidShiftCount => (
                Code::InvalidShiftCount,
                format!("invalid operation: invalid shift count {value}"),
            ),
            BinaryError::ShiftCountConversion(error) => implicit_conversion(error, &value, "uint"),
            _ => return,
        };
        self.compiler_error(&expr.span, code, &message);
    }

    pub(in crate::analysis::engine::check) fn conversion_error(
        &mut self,
        id: ExprId,
        operand: Operand,
        target: TypeId,
        error: ConversionError,
    ) {
        let Some(expr) = self.body.expression(id) else {
            return;
        };
        let Some(target) = self.arena.render_presentation(target, self.file) else {
            return;
        };
        let message = match error {
            ConversionError::Overflow(value) => format!("constant {value} overflows {target}"),
            ConversionError::InvalidConstant => {
                let Some(value) = self.operand_description(id, operand) else {
                    return;
                };
                format!("cannot convert {value} to type {target}")
            }
        };
        self.compiler_error(&expr.span, Code::InvalidConversion, &message);
    }
}

fn implicit_conversion(error: ConstAssignError, operand: &str, target: &str) -> (Code, String) {
    match error {
        ConstAssignError::Incompatible => (
            Code::InvalidConstVal,
            format!("cannot convert {operand} to type {target}"),
        ),
        ConstAssignError::Overflow => (
            Code::NumericOverflow,
            format!("{operand} overflows {target}"),
        ),
        ConstAssignError::Truncated => (
            Code::TruncatedFloat,
            format!("{operand} truncated to {target}"),
        ),
    }
}
