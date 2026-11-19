//! Builtin call arity and dispatch. Structural checks live in the type arena.
use std::{num::NonZeroU32, ops::Range};

use super::{ExprOutcome, RegionChecker, diagnostics::Code};
use crate::analysis::{
    obj::{BuiltinFunction, ObjectRef},
    types::{
        AppendError, BasicKind, CheckError, ComplexError, CopyError, MakeError, MinMaxError, Mode,
        Operand, ProcedureError,
    },
};
use crate::syntax::{BlockTree, ExprId, ExprKind};

struct Call<'a> {
    builtin: BuiltinFunction,
    function: ExprId,
    arguments: &'a [ExprId],
    span: &'a Range<usize>,
    spread: bool,
}

impl Call<'_> {
    /// A single written call can supply multiple operands.
    fn argument(&self, position: usize) -> ExprId {
        if self.arguments.len() == 1 {
            self.arguments[0]
        } else {
            self.arguments[position]
        }
    }
}

impl<'a, 'b> RegionChecker<'a, 'b> {
    pub(super) fn check_builtin_call(
        &mut self,
        builtin: BuiltinFunction,
        function: ExprId,
        arguments: &[ExprId],
        ellipsis: Option<NonZeroU32>,
        span: &Range<usize>,
    ) -> ExprOutcome<'a, 'b> {
        let call = Call {
            builtin,
            function,
            arguments,
            span,
            spread: ellipsis.is_some(),
        };
        let result = if let Some(dots) = ellipsis
            && builtin != BuiltinFunction::Append
        {
            self.check_exprs(arguments);
            let offset = dots.get() as usize;
            self.compiler_error(
                &(offset..offset + 3),
                Code::InvalidDotDotDot,
                &format!(
                    "invalid operation: invalid use of ... with built-in {}",
                    builtin.name()
                ),
            );
            None
        } else {
            self.evaluate_builtin(&call)
        };
        let info = self.build_call_site_info(
            span,
            function,
            arguments,
            Some(ObjectRef::BuiltinFunction(builtin)),
            None,
            false,
            result.is_some_and(|op| op.mode == Mode::Constant),
        );
        self.region.calls.push(info);
        result.map_or_else(ExprOutcome::invalid, ExprOutcome::Value)
    }

    fn evaluate_builtin(&mut self, call: &Call<'_>) -> Option<Operand> {
        use BuiltinFunction::*;
        let int_bits = self.int_bits()?;
        // Unlike the value builtins, these constructors may receive a type.
        match call.builtin {
            Make => return self.check_make(call, int_bits),
            New => return self.check_new(call, int_bits),
            _ => {}
        }
        let arguments = call.arguments;
        // The common single argument case needs no temporary operand vector.
        let single;
        let pack;
        let args = match arguments {
            [] => &[][..],
            [id] if call.spread || !is_call(self.body, *id) => {
                single = [self.check_expr(*id, None)?];
                single[0].mode.is_value().then_some(single.as_slice())?
            }
            _ => {
                pack = self.eval_call_arg_operands(arguments, call.spread, None)?;
                &pack
            }
        };
        if !self.builtin_count(call, args.len()) {
            return None;
        }
        match call.builtin {
            Len | Cap => {
                let arg = args[0];
                let has_call = self.expr_has_call(call.argument(0));
                let capacity = call.builtin == Cap;
                match self
                    .arena
                    .checked_builtin_length(arg, capacity, has_call, int_bits)
                {
                    Ok(result) => Some(result),
                    Err(error) => {
                        if let CheckError::Invalid(()) = error
                            && let Some(value) = self.operand_description(call.argument(0), arg)
                        {
                            self.arg_error(
                                call,
                                0,
                                if capacity {
                                    Code::InvalidCap
                                } else {
                                    Code::InvalidLen
                                },
                                &format!(
                                    "invalid argument: {value} for built-in {}",
                                    call.builtin.name()
                                ),
                            );
                        }
                        None
                    }
                }
            }
            Append => match self.arena.builtin_append(args, call.spread, int_bits) {
                Ok(result) => Some(result),
                Err(error) => {
                    match error {
                        CheckError::Invalid(AppendError::NotSlice) => {
                            self.slice_argument_error(call, 0, args[0], Code::InvalidAppend)
                        }
                        CheckError::Invalid(AppendError::SpreadCount { trailing_slice }) => {
                            self.check_argument_count(
                                call.function,
                                call.arguments,
                                call.span,
                                args,
                                &[args[0].ty, trailing_slice],
                                call.spread,
                                true,
                            );
                        }
                        CheckError::Invalid(AppendError::Argument {
                            index,
                            target,
                            reason,
                        }) => {
                            self.assignment_error(
                                args[index],
                                target,
                                call.argument(index),
                                "argument to append",
                                reason,
                            );
                        }
                        CheckError::Unknown => {}
                    }
                    None
                }
            },
            Copy => match self.arena.builtin_copy(args[0], args[1], int_bits) {
                Ok(result) => Some(result),
                Err(error) => {
                    match error {
                        CheckError::Invalid(CopyError::NotSlice(index)) => {
                            self.slice_argument_error(call, index, args[index], Code::InvalidCopy);
                        }
                        CheckError::Invalid(CopyError::DifferentElements(dst, src)) => {
                            if let (Some(a), Some(b), Some(dst), Some(src)) = (
                                self.operand_description(call.argument(0), args[0]),
                                self.operand_description(call.argument(1), args[1]),
                                self.arena.render_presentation(dst, self.file),
                                self.arena.render_presentation(src, self.file),
                            ) {
                                self.arg_error(call, 0, Code::InvalidCopy, &format!("invalid copy: arguments {a} and {b} have different element types {dst} and {src}"));
                            }
                        }
                        CheckError::Unknown => {}
                    }
                    None
                }
            },
            Complex => self.check_complex(call, args, int_bits),
            Real | Imag => {
                let imaginary = call.builtin == Imag;
                match self.arena.checked_builtin_component(args[0], imaginary)? {
                    Ok(result) => Some(result),
                    Err(()) => {
                        let ty = self.arena.render_presentation(args[0].ty, self.file)?;
                        let code = if imaginary {
                            Code::InvalidImag
                        } else {
                            Code::InvalidReal
                        };
                        self.arg_error(
                            call,
                            0,
                            code,
                            &format!(
                                "invalid argument: argument has type {ty}, expected complex type"
                            ),
                        );
                        None
                    }
                }
            }
            Min | Max => self.check_minmax(call, args, int_bits),
            Recover => Some(Operand {
                mode: Mode::Value,
                ty: self.arena.builtin_type(self.file, "any")?,
                value: None,
            }),
            Clear | Close | Delete => self.check_procedure(call, args),
            Panic | Print | Println => Some(Operand::non_value(Mode::NoValue)),
            Make | New => unreachable!("constructors take syntactic arguments"),
        }
    }

    fn check_complex(
        &mut self,
        call: &Call<'_>,
        args: &[Operand],
        int_bits: u32,
    ) -> Option<Operand> {
        let message = match self.arena.checked_builtin_complex(args, int_bits)? {
            Ok(result) => return Some(result),
            Err(ComplexError::Mismatch(a, b)) => {
                let text =
                    self.call_description(call.function, call.arguments, call.spread, call.span)?;
                let a = self.arena.render_presentation(a, self.file)?;
                let b = self.arena.render_presentation(b, self.file)?;
                format!("invalid operation: {text} (mismatched types {a} and {b})")
            }
            Err(ComplexError::NonFloating(ty)) => {
                let ty = self.arena.render_presentation(ty, self.file)?;
                format!("invalid argument: arguments have type {ty}, expected floating-point")
            }
        };
        self.arg_error(call, 0, Code::InvalidComplex, &message);
        None
    }

    fn check_minmax(
        &mut self,
        call: &Call<'_>,
        args: &[Operand],
        int_bits: u32,
    ) -> Option<Operand> {
        let (index, code, message) = match self.arena.checked_builtin_minmax(
            args,
            call.builtin == BuiltinFunction::Max,
            int_bits,
        )? {
            Ok(result) => return Some(result),
            Err(MinMaxError::Unordered(index)) => {
                let value = self.operand_description(call.argument(index), args[index])?;
                (
                    index,
                    Code::InvalidMinMaxOperand,
                    format!("invalid argument: {value} cannot be ordered"),
                )
            }
            Err(MinMaxError::Mismatch { index, previous }) => {
                let previous = self.arena.render_presentation(previous, self.file)?;
                let ty = self.arena.render_presentation(args[index].ty, self.file)?;
                let span = &self.body.expression(call.argument(index))?.span;
                if span.len() > 1024 {
                    return None;
                }
                (
                    index,
                    Code::MismatchedTypes,
                    format!(
                        "invalid argument: mismatched types {previous} (previous argument) and {ty} (type of {})",
                        self.text(span)
                    ),
                )
            }
        };
        self.arg_error(call, index, code, &message);
        None
    }

    fn builtin_count(&mut self, call: &Call<'_>, actual: usize) -> bool {
        use BuiltinFunction::*;
        let (expected, accepts_more) = match call.builtin {
            Print | Println => return true,
            Recover => (0, false),
            Complex | Copy | Delete => (2, false),
            Append | Make | Min | Max => (1, true),
            Cap | Clear | Close | Imag | Len | New | Panic | Real => (1, false),
        };
        let qualifier = if actual < expected {
            "not enough"
        } else if actual > expected && !accepts_more {
            "too many"
        } else {
            return true;
        };
        if let Some(text) =
            self.call_description(call.function, call.arguments, call.spread, call.span)
        {
            self.compiler_error(call.span, Code::WrongArgCount,
                &format!("invalid operation: {qualifier} arguments for {text} (expected {expected}, found {actual})"));
        }
        false
    }

    fn check_make(&mut self, call: &Call<'_>, int_bits: u32) -> Option<Operand> {
        if !self.builtin_count(call, call.arguments.len()) {
            return None;
        }
        let ty = self.check_expr(call.argument(0), None);
        let mut sizes = [Operand::non_value(Mode::Invalid); 2];
        let size_arguments = &call.arguments[1..];
        for (index, &id) in size_arguments.iter().enumerate() {
            let op = self.check_expr(id, None);
            if let Some(slot) = sizes.get_mut(index)
                && let Some(op) = op
            {
                *slot = op;
            }
        }
        let ty = ty?;
        if ty.mode != Mode::TypeExpr {
            if let Some(expr) = self.body.expression(call.argument(0))
                && expr.span.len() <= 1024
            {
                self.compiler_error(
                    &expr.span,
                    Code::NotAType,
                    &format!(
                        "{}{category} is not a type",
                        self.text(&expr.span),
                        category = type_argument_category(&expr.kind, ty.mode)
                    ),
                );
            }
            return None;
        }
        match self
            .arena
            .builtin_make(ty.ty, sizes, size_arguments.len(), int_bits)
        {
            Ok(result) => {
                for (index, error) in result.size_errors.into_iter().enumerate() {
                    if let Some(error) = error {
                        self.index_value_error(size_arguments[index], sizes[index], error);
                    }
                }
                if result.swapped {
                    self.arg_error(
                        call,
                        1,
                        Code::SwappedMakeArgs,
                        "invalid argument: length and capacity swapped",
                    );
                }
                result.operand
            }
            Err(CheckError::Invalid(error)) => {
                match error {
                    MakeError::NotMakeable => {
                        let expr = self.body.expression(call.argument(0))?;
                        if expr.span.len() <= 1024 {
                            self.compiler_error(&expr.span, Code::InvalidMake, &format!("invalid argument: cannot make {}: type must be slice, map, or channel", self.text(&expr.span)));
                        }
                    }
                    MakeError::Count { minimum } => {
                        let text = self.call_description(
                            call.function,
                            call.arguments,
                            call.spread,
                            call.span,
                        )?;
                        self.compiler_error(call.span, Code::WrongArgCount,
                            &format!("invalid operation: {text} expects {minimum} or {} arguments; found {}", minimum + 1, call.arguments.len()));
                    }
                }
                None
            }
            Err(CheckError::Unknown) => None,
        }
    }

    fn check_new(&mut self, call: &Call<'_>, int_bits: u32) -> Option<Operand> {
        if !self.builtin_count(call, call.arguments.len()) {
            self.check_exprs(call.arguments);
            return None;
        }
        let arg = self.check_expr(call.argument(0), None)?;
        let ty = if arg.mode == Mode::TypeExpr {
            arg.ty
        } else if self.arena.basic_kind(arg.ty) == Some(BasicKind::UntypedNil) {
            self.arg_error(
                call,
                0,
                Code::UntypedNilUse,
                "use of untyped nil in argument to new",
            );
            return None;
        } else {
            self.arena.default_operand(arg, int_bits)?
        };
        self.arena.builtin_new(ty)
    }

    fn check_procedure(&mut self, call: &Call<'_>, args: &[Operand]) -> Option<Operand> {
        match self.arena.builtin_procedure(call.builtin, args[0].ty) {
            Ok(key) => {
                if let Some(target) = key {
                    self.check_assignment(args[1], target, call.argument(1), "argument to delete")?;
                }
                Some(Operand::non_value(Mode::NoValue))
            }
            Err(CheckError::Invalid(error)) => {
                let value = self.operand_description(call.argument(0), args[0])?;
                let (code, message) = match error {
                    ProcedureError::NotClearable => (
                        Code::InvalidClear,
                        format!(
                            "invalid argument: cannot clear {value}: argument must be (or constrained by) map or slice"
                        ),
                    ),
                    ProcedureError::NotChannel => (
                        Code::InvalidClose,
                        format!("invalid operation: cannot close non-channel {value}"),
                    ),
                    ProcedureError::ReceiveOnly => (
                        Code::InvalidClose,
                        format!("invalid operation: cannot close receive-only channel {value}"),
                    ),
                    ProcedureError::NotMap => (
                        Code::InvalidDelete,
                        format!("invalid argument: {value} is not a map"),
                    ),
                    ProcedureError::DifferentKeys => (
                        Code::InvalidDelete,
                        format!("invalid argument: maps of {value} must have identical key types"),
                    ),
                };
                self.arg_error(call, 0, code, &message);
                None
            }
            Err(CheckError::Unknown) => None,
        }
    }

    fn slice_argument_error(
        &mut self,
        call: &Call<'_>,
        index: usize,
        argument: Operand,
        code: Code,
    ) {
        let value = if self.arena.basic_kind(argument.ty) == Some(BasicKind::UntypedNil) {
            "untyped nil".to_owned()
        } else {
            let Some(value) = self.operand_description(call.argument(index), argument) else {
                return;
            };
            value
        };
        self.arg_error(
            call,
            index,
            code,
            &format!(
                "invalid {}: argument must be a slice; have {value}",
                call.builtin.name()
            ),
        );
    }

    fn arg_error(&mut self, call: &Call<'_>, index: usize, code: Code, message: &str) {
        if let Some(expr) = self.body.expression(call.argument(index)) {
            self.compiler_error(&expr.span, code, message);
        }
    }
}

fn is_call(body: &BlockTree, id: ExprId) -> bool {
    body.expression(id)
        .is_some_and(|expr| matches!(expr.kind, ExprKind::Call { .. }))
}

fn type_argument_category(kind: &ExprKind, mode: Mode) -> &'static str {
    match (kind, mode) {
        (ExprKind::Name, Mode::Variable) => " (variable)",
        (ExprKind::Name, Mode::Constant) => " (constant)",
        _ => "",
    }
}
