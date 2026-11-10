//! Goal-directed basic operands. Untyped values default only at binding boundaries.
use super::Inference;
use crate::analysis::scope::DeclarationKind;
use crate::analysis::types::{BasicKind, Constant, Mode, Operand, TypeArena, TypeForm};
use crate::syntax::{ExprId, ExprKind};

impl Inference<'_, '_> {
    #[cfg(test)]
    pub(crate) fn test_operand(&self, arena: &mut TypeArena<'_>, id: ExprId) -> Option<Operand> {
        self.value_operand(arena, id)
    }

    pub(super) fn scalar_form(
        &self,
        id: ExprId,
        result: usize,
        count: usize,
    ) -> Option<(TypeForm, bool)> {
        let mut arena = TypeArena::new(self.workspace);
        let operand = if matches!(
            self.unparenthesized(id).kind,
            ExprKind::TypeAssertion { .. }
        ) {
            self.assertion_operand(&mut arena, id, result, count)?
        } else if let ExprKind::Unary { operator, .. } = &self.unparenthesized(id).kind
            && self.text(operator) == "<-"
        {
            self.receive_operand(&mut arena, id, result, count)?
        } else if result == 0 && count == 1 {
            if matches!(self.unparenthesized(id).kind, ExprKind::Call { .. }) {
                self.value_operand(&mut arena, id)?
            } else {
                self.scalar_operand(&mut arena, id)?
            }
        } else {
            return None;
        };
        let form = arena.form(operand.ty)?;
        let untyped = arena.basic_kind(operand.ty).is_some_and(BasicKind::untyped);
        Some((form, untyped))
    }
    pub(super) fn scalar_type(&self, id: ExprId) -> Option<String> {
        let mut arena = TypeArena::new(self.workspace);
        let operand = self.scalar_operand(&mut arena, id)?;
        let ty = arena.default_operand(operand, self.workspace.integer_bits()?)?;
        arena.render_resolved(ty, self.file)
    }

    pub(super) fn scalar_operand(&self, arena: &mut TypeArena<'_>, id: ExprId) -> Option<Operand> {
        if self.depth >= 32 {
            return None;
        }
        let expression = self.unparenthesized(id);
        match &expression.kind {
            ExprKind::Unary { operator, .. } if self.text(operator) == "<-" => {
                self.receive_operand(arena, id, 0, 1)
            }
            ExprKind::TypeAssertion { .. } => self.assertion_operand(arena, id, 0, 1),
            ExprKind::Slice { .. } => self.sliced_operand(arena, id),
            ExprKind::Index { .. } => self.indexed_operand(arena, id, 0, 1),
            ExprKind::Literal => arena.literal(self.text(&expression.span)),
            ExprKind::Name => {
                let name = self.text(&expression.span);
                if let Some(binding) = self
                    .locals
                    .resolve_at(self.base + expression.span.start, name)
                {
                    if self.locals.switch_case(binding).is_some() {
                        return self.switch_operand(arena, binding);
                    }
                    if let Some(value) = self.local_binding(arena, binding) {
                        return value;
                    }
                    if binding.declaration == DeclarationKind::Constant {
                        let (span, result, count) = binding.initializer()?;
                        if result != 0 || count != 1 {
                            return None;
                        }
                        let source = self.source.get(span.clone())?;
                        if source.len() > 16 * 1024 {
                            return None;
                        }
                        let parsed = crate::syntax::parse_expression(source);
                        if !parsed.diagnostics.is_empty() {
                            return None;
                        }
                        let operand = Inference {
                            expressions: &parsed.nodes,
                            base: span.start,
                            depth: self.depth + 1,
                            ..*self
                        }
                        .scalar_operand(arena, parsed.root)?;
                        if operand.mode != Mode::Constant {
                            return None;
                        }
                        if let Some(annotation) = binding.annotation() {
                            let text = self.source.get(annotation.clone())?;
                            if !self.concrete_spelling(text, annotation.start) {
                                return None;
                            }
                            let ty = arena.parse(self.file, text)?;
                            return arena.assign_basic(operand, ty, self.workspace.integer_bits()?);
                        }
                        return Some(operand);
                    }
                    if binding.declared_type.is_none() && binding.annotation().is_none() {
                        let scoped = self.next().scoped_expression(id, 0, 1)?;
                        if scoped.scope == self.file
                            && !self
                                .concrete_spelling(&scoped.text, self.base + expression.span.start)
                        {
                            return None;
                        }
                        return Some(Operand {
                            mode: Mode::Variable,
                            ty: arena.parse(scoped.scope, &scoped.text)?,
                            value: None,
                        });
                    }
                    let text = self.workspace.infer_binding(
                        self.file,
                        self.source,
                        self.locals,
                        binding,
                        self.depth + 1,
                    )?;
                    if !self.concrete_spelling(&text, self.base + expression.span.start) {
                        return None;
                    }
                    let ty = arena.parse(self.file, &text)?;
                    return Some(Operand {
                        mode: Mode::Variable,
                        ty,
                        value: None,
                    });
                }
                if self.global_symbol(id).is_some() {
                    return self.global_operand(arena, id);
                }
                if !crate::analysis::scope::universe::visible(
                    self.workspace,
                    self.file,
                    None,
                    0,
                    name,
                ) {
                    return None;
                }
                if name == "iota" {
                    return Some(Operand {
                        mode: Mode::Constant,
                        ty: arena.basic(BasicKind::UntypedInt)?,
                        value: Some(Constant::Int(i128::from(self.iota?))),
                    });
                }
                if matches!(name, "true" | "false") {
                    return Some(Operand {
                        mode: Mode::Constant,
                        ty: arena.basic(BasicKind::UntypedBool)?,
                        value: Some(Constant::Boolean),
                    });
                }
                if name == "nil" {
                    return Some(Operand {
                        mode: Mode::Value,
                        ty: arena.basic(BasicKind::UntypedNil)?,
                        value: None,
                    });
                }
                None
            }
            ExprKind::Selector { .. } => {
                if self.global_symbol(id).is_some() {
                    self.global_operand(arena, id)
                } else {
                    self.global_field(arena, id)
                }
            }
            ExprKind::Call {
                function,
                arguments,
                variadic,
            } => {
                if let Some(name) = self.builtin_name(*function) {
                    return self.builtin_operand(arena, name, arguments, *variadic);
                }
                let target = self.conversion_target(arena, *function)?;
                if *variadic || arguments.len() != 1 {
                    return None;
                }
                let operand = self.next().value_operand(arena, arguments[0])?;
                arena.convert(operand, target, self.workspace.integer_bits()?)
            }
            ExprKind::Unary { operator, operand }
                if matches!(self.text(operator), "+" | "-" | "^" | "!") =>
            {
                let operand = self.next().scalar_operand(arena, *operand)?;
                arena.unary(self.text(operator), operand, self.workspace.integer_bits()?)
            }
            ExprKind::Binary {
                left,
                operator,
                right,
            } => {
                let left = self.next().scalar_operand(arena, *left)?;
                let right = self.next().scalar_operand(arena, *right)?;
                arena.binary(
                    self.text(operator),
                    left,
                    right,
                    self.workspace.integer_bits()?,
                )
            }
            _ => None,
        }
    }
}
