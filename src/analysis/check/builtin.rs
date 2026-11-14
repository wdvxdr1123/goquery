//! Builtin recognition is scope-aware; native operands never cross a spelling boundary.
use super::Inference;
use crate::analysis::types::{Mode, Operand, TypeArena, TypeId};
use crate::syntax::{ExprId, ExprKind};

impl Inference<'_, '_> {
    /// go/types does not give a callable signature to a folded builtin call.
    /// Suppress help only when the existing operand evaluator proves it constant.
    pub(crate) fn constant_builtin_call(&self, function: ExprId) -> bool {
        let Some(name) = self.builtin_name(function) else {
            return false;
        };
        if !matches!(
            name,
            "len" | "cap" | "complex" | "real" | "imag" | "min" | "max"
        ) {
            return false;
        }
        let selected = self.unparenthesized(function);
        self.expressions.iter().any(|expression| {
            let ExprKind::Call {
                function,
                arguments,
                variadic,
            } = &expression.kind
            else {
                return false;
            };
            if self.unparenthesized(*function).span != selected.span {
                return false;
            }
            let mut arena = TypeArena::new(self.workspace);
            self.builtin_operand(&mut arena, name, arguments, *variadic)
                .is_some_and(|operand| operand.mode == Mode::Constant)
        })
    }

    pub(crate) fn builtin_name(&self, function: ExprId) -> Option<&str> {
        let expression = self.unparenthesized(function);
        if !matches!(expression.kind, ExprKind::Name) {
            return None;
        }
        let name = self.text(&expression.span);
        if !crate::analysis::scope::universe::builtin_function(name) {
            return None;
        }
        if !crate::analysis::scope::universe::visible(
            self.workspace,
            self.file,
            Some(self.locals),
            self.base + expression.span.start,
            name,
        ) {
            return None;
        }
        Some(name)
    }

    pub(super) fn builtin_operand(
        &self,
        arena: &mut TypeArena<'_>,
        name: &str,
        arguments: &[ExprId],
        spread: bool,
    ) -> Option<Operand> {
        if self.depth >= 32 || arguments.len() > 256 || spread && name != "append" {
            return None;
        }
        let int_bits = self.workspace.integer_bits()?;
        match name {
            "complex" => {
                let actuals = self.builtin_arguments(arena, arguments, false)?;
                arena.builtin_complex(&actuals, int_bits)
            }
            "real" | "imag" if arguments.len() == 1 => {
                let input = self.builtin_argument(arena, arguments[0])?;
                arena.builtin_component(input, name == "imag")
            }
            "new" if arguments.len() == 1 => {
                let ty = if let Some(ty) = self.builtin_type(arena, arguments[0]) {
                    ty
                } else {
                    let input = self.builtin_argument(arena, arguments[0])?;
                    arena.default_operand(input, int_bits)?
                };
                arena.builtin_new(ty)
            }
            "make" if (1..=3).contains(&arguments.len()) => {
                let ty = self.builtin_type(arena, arguments[0])?;
                let sizes = arguments[1..]
                    .iter()
                    .map(|id| self.builtin_argument(arena, *id))
                    .collect::<Option<Vec<_>>>()?;
                arena.builtin_make(ty, &sizes, int_bits)
            }
            "len" | "cap" if arguments.len() == 1 => {
                let input = self.builtin_argument(arena, arguments[0])?;
                let has_call = self.length_has_call(arena, arguments[0])?;
                arena.builtin_length(input, name == "cap", has_call, int_bits)
            }
            "append" => {
                let actuals = self.builtin_arguments(arena, arguments, spread)?;
                arena.builtin_append(&actuals, spread, int_bits)
            }
            "min" | "max" => {
                let actuals = self.builtin_arguments(arena, arguments, false)?;
                arena.builtin_minmax(&actuals, name == "max", int_bits)
            }
            "copy" if arguments.len() == 2 => {
                let destination = self.builtin_argument(arena, arguments[0])?;
                let source = self.builtin_argument(arena, arguments[1])?;
                arena.builtin_copy(destination, source, int_bits)
            }
            "recover" if arguments.is_empty() => Some(Operand {
                mode: Mode::Value,
                ty: arena.parse(self.file, "interface{}")?,
                value: None,
            }),
            // No-value builtins and unsupported numeric builtins cannot produce
            // a fabricated scalar or fall through to an unrelated value call.
            _ => None,
        }
    }

    fn builtin_type(&self, arena: &mut TypeArena<'_>, id: ExprId) -> Option<TypeId> {
        let expression = self.unparenthesized(id);
        let text = self.text(&expression.span);
        let offset = self.base + expression.span.start;
        // Builtin type operands share the lexical annotation resolver, including
        // local defined-type identities, rather than requiring a package spelling.
        match self.annotation_type(arena, text, offset) {
            super::scope::AnnotationType::Resolved(ty) => Some(ty),
            _ => None,
        }
    }

    fn builtin_arguments(
        &self,
        arena: &mut TypeArena<'_>,
        arguments: &[ExprId],
        spread: bool,
    ) -> Option<Vec<Operand>> {
        arguments
            .iter()
            .map(|id| self.builtin_argument(arena, *id))
            .collect::<Option<Vec<_>>>()
            .or_else(|| {
                if !spread
                    && arguments.len() == 1
                    && matches!(
                        self.unparenthesized(arguments[0]).kind,
                        ExprKind::Call { .. }
                    )
                {
                    self.next().call_operands(arena, arguments, spread)
                } else {
                    None
                }
            })
    }

    fn builtin_argument(&self, arena: &mut TypeArena<'_>, id: ExprId) -> Option<Operand> {
        self.next()
            .value_operand(arena, id)
            .or_else(|| self.next().parameterized_operand(arena, id))
    }

    /// Array len/cap is constant only without value calls/receives. Conversions
    /// and constant builtin calls do not count as calls in go/types.
    fn length_has_call(&self, arena: &mut TypeArena<'_>, id: ExprId) -> Option<bool> {
        if self.expressions.len() > 4096 {
            return None;
        }
        let span = &self.unparenthesized(id).span;
        for expression in self.expressions {
            if expression.span.start < span.start || expression.span.end > span.end {
                continue;
            }
            match &expression.kind {
                ExprKind::Unary { operator, .. } if self.text(operator) == "<-" => {
                    return Some(true);
                }
                ExprKind::Call {
                    function,
                    arguments,
                    variadic,
                } => {
                    if self.conversion_target(arena, *function).is_some() {
                        continue;
                    }
                    if let Some(name) = self.builtin_name(*function)
                        && self
                            .next()
                            .builtin_operand(arena, name, arguments, *variadic)?
                            .mode
                            == Mode::Constant
                    {
                        continue;
                    }
                    return Some(true);
                }
                _ => {}
            }
        }
        Some(false)
    }
}
