//! Resolve conversion syntax without confusing value calls or shadowed types.
use super::Inference;
use crate::analysis::scope::DeclarationKind;
use crate::analysis::types::{Mode, Operand, TypeArena, TypeForm, TypeId};
use crate::syntax::{ExprId, ExprKind};

impl Inference<'_, '_> {
    pub(super) fn conversion_target(
        &self,
        arena: &mut TypeArena<'_>,
        function: ExprId,
    ) -> Option<TypeId> {
        if self.depth >= 32 {
            return None;
        }
        let expression = self.unparenthesized(function);
        self.type_target(arena, &expression.span)
    }

    /// Resolve written type syntax at its lexical position. Values and local
    /// declarations without native identity cannot fall back to package names.
    pub(super) fn type_target(
        &self,
        arena: &mut TypeArena<'_>,
        span: &std::ops::Range<usize>,
    ) -> Option<TypeId> {
        if self.depth >= 32 {
            return None;
        }
        let text = self.text(span);
        if text.len() > 16 * 1024 {
            return None;
        }
        let parsed = crate::syntax::parse_type(text);
        if !parsed.diagnostics.is_empty() {
            return None;
        }
        let offset = self.base + span.start;
        let mut parameterized = false;
        for reference in &parsed.references {
            let span = reference.qualifier.as_ref().unwrap_or(&reference.name);
            let name = &text[span.clone()];
            if let Some(binding) = self.locals.resolve_at(offset + span.start, name) {
                if binding.declaration != DeclarationKind::TypeParameter {
                    return None;
                }
                parameterized = true;
                continue;
            }
            if reference.qualifier.is_none()
                && self
                    .workspace
                    .unqualified_symbol(self.file, name)
                    .is_some_and(|(_, symbol)| !symbol.is_type())
            {
                return None;
            }
        }
        if parameterized {
            self.parameterized_type(arena, text, offset)
        } else {
            arena.parse(self.file, text)
        }
    }

    pub(super) fn value_operand(&self, arena: &mut TypeArena<'_>, id: ExprId) -> Option<Operand> {
        if self.depth >= 32 {
            return None;
        }
        let expression = self.unparenthesized(id);
        match &expression.kind {
            ExprKind::Composite {
                literal_type: Some(literal),
                elements,
            } if elements.is_empty() => {
                let ty = self.type_target(arena, &self.expressions[literal.0].span)?;
                if !matches!(
                    arena.form(ty)?,
                    TypeForm::Struct | TypeForm::Array | TypeForm::Slice | TypeForm::Map
                ) {
                    return None;
                }
                arena.constructor_map_keys(ty)?;
                Some(Operand {
                    mode: Mode::Value,
                    ty,
                    value: None,
                })
            }
            ExprKind::Name
                if self
                    .locals
                    .resolve_at(
                        self.base + expression.span.start,
                        self.text(&expression.span),
                    )
                    .is_none() =>
            {
                self.function_operand(arena, id)
                    .or_else(|| self.scalar_operand(arena, id))
            }
            ExprKind::FunctionLiteral { .. } | ExprKind::Selector { .. } => {
                if let Some(operand) = self.selector_operand(arena, id) {
                    return operand;
                }
                if self.global_symbol(id).is_some() {
                    return self.global_operand(arena, id);
                }
                if let Some(operand) = self.function_operand(arena, id) {
                    return Some(operand);
                }
                if let ExprKind::Selector { operand, .. } = &expression.kind
                    && self.global_root(*operand)
                {
                    return self.global_field(arena, id);
                }
                let scoped = self.next().scoped_expression(id, 0, 1)?;
                Some(Operand {
                    mode: Mode::Value,
                    ty: arena.parse(scoped.scope, &scoped.text)?,
                    value: None,
                })
            }
            ExprKind::Slice { .. } => self.sliced_operand(arena, id),
            ExprKind::TypeAssertion { .. } => self.assertion_operand(arena, id, 0, 1),
            ExprKind::Index { .. } => self
                .function_operand(arena, id)
                .or_else(|| self.indexed_operand(arena, id, 0, 1)),
            ExprKind::Literal | ExprKind::Name | ExprKind::Binary { .. } => {
                self.scalar_operand(arena, id)
            }
            ExprKind::Unary { operator, operand } if self.text(operator) == "*" => {
                let input = self.next().value_operand(arena, *operand)?;
                arena.indirect(input)
            }
            ExprKind::Unary { operator, .. } if self.text(operator) == "<-" => {
                self.receive_operand(arena, id, 0, 1)
            }
            ExprKind::Unary { operator, operand } if self.text(operator) == "&" => {
                let input = self.next().value_operand(arena, *operand)?;
                if input.mode != Mode::Variable
                    && !matches!(
                        self.unparenthesized(*operand).kind,
                        ExprKind::Composite { .. }
                    )
                {
                    return None;
                }
                Some(Operand {
                    mode: Mode::Value,
                    ty: arena.pointer(input.ty)?,
                    value: None,
                })
            }
            ExprKind::Unary { operator, .. }
                if matches!(self.text(operator), "+" | "-" | "^" | "!") =>
            {
                self.scalar_operand(arena, id)
            }
            ExprKind::Call {
                function,
                arguments,
                variadic,
            } => {
                if let Some(name) = self.builtin_name(*function) {
                    return self.builtin_operand(arena, name, arguments, *variadic);
                }
                if let Some(target) = self.conversion_target(arena, *function) {
                    if *variadic || arguments.len() != 1 {
                        return None;
                    }
                    let operand = self.next().value_operand(arena, arguments[0])?;
                    return arena.convert(operand, target, self.workspace.integer_bits()?);
                }
                if let Some(result) = self.native_call(arena, *function, arguments, *variadic, 0, 1)
                {
                    return result;
                }
                let scoped = self.next().scoped_expression(id, 0, 1)?;
                Some(Operand {
                    mode: Mode::Value,
                    ty: arena.parse(scoped.scope, &scoped.text)?,
                    value: None,
                })
            }
            _ => {
                let scoped = self.next().scoped_expression(id, 0, 1)?;
                Some(Operand {
                    mode: Mode::Value,
                    ty: arena.parse(scoped.scope, &scoped.text)?,
                    value: None,
                })
            }
        }
    }
}
