//! Inferred types keep their declaration scope independently of display spelling.
use super::Inference;
use crate::analysis::member::MemberDeclaration;
use crate::analysis::method_set::ReceiverMember;
use crate::analysis::scope as locals;
use crate::analysis::types::{TypeArena, TypeId};
use crate::syntax::{ExprId, ExprKind};
use crate::workspace::Workspace;
use std::path::Path;

pub(crate) struct ScopedType<'a> {
    pub(crate) scope: &'a Path,
    pub(crate) text: String,
}

impl ScopedType<'_> {
    pub(crate) fn render(self, workspace: &Workspace, file: &Path) -> Option<String> {
        if self.scope == file {
            Some(self.text)
        } else {
            workspace.requalified_type(file, self.scope, &self.text)
        }
    }
}

impl<'workspace> Inference<'workspace, '_> {
    pub(super) fn scoped(&self, text: String) -> Option<ScopedType<'workspace>> {
        Some(ScopedType {
            scope: self.workspace.files.get_key_value(self.file)?.0,
            text,
        })
    }

    pub(super) fn scoped_native_type(
        &self,
        arena: &mut TypeArena<'workspace>,
        ty: TypeId,
        preferred: &Path,
        offset: usize,
    ) -> Option<ScopedType<'workspace>> {
        if let Some(text) = arena.render_resolved(ty, self.file) {
            if !self.concrete_spelling(&text, offset) {
                let alternative = arena.render_unaliased(ty, self.file)?;
                return self
                    .concrete_spelling(&alternative, offset)
                    .then(|| self.scoped(alternative))
                    .flatten();
            }
            return self.scoped(text);
        }
        // Formal spellings must resolve back to the same declaration identities.
        if let Some(text) = arena.render(ty, self.file)
            && let Some(projected) = self.parameterized_type(arena, &text, offset)
            && arena.identical(ty, projected) == Some(true)
        {
            return self.scoped(text);
        }
        let (scope, text) = arena.render_declared(ty, preferred, self.file)?;
        Some(ScopedType { scope, text })
    }

    pub(super) fn scoped_value_type(
        &self,
        origin: &Path,
        annotation: &str,
    ) -> Option<ScopedType<'workspace>> {
        if let Some(text) = self
            .workspace
            .inference_value_type(self.file, origin, annotation)
        {
            return self.scoped(text);
        }
        // A resolved named declaration remains usable even when its package
        // has no import spelling in the consumer. Do not invent an alias.
        if annotation.len() > 16 * 1024 {
            return None;
        }
        let name = locals::named_type(annotation)?;
        self.workspace.type_declaration(origin, &name)?;
        Some(ScopedType {
            scope: self.workspace.files.get_key_value(origin)?.0,
            text: annotation.to_owned(),
        })
    }

    pub(crate) fn scoped_receiver_type(&self, id: ExprId) -> Option<ScopedType<'workspace>> {
        if self.depth >= 32 {
            return None;
        }
        let ExprKind::Selector { operand, .. } = self.unparenthesized(id).kind else {
            return None;
        };
        let expression = self.unparenthesized(operand);
        if matches!(expression.kind, ExprKind::Name)
            && self
                .locals
                .resolve_at(
                    self.base + expression.span.start,
                    self.text(&expression.span),
                )
                .is_none()
            && self.global_symbol(operand).is_none()
        {
            return None;
        }
        let mut ty = self.next().scoped_expression(operand, 0, 1)?;
        ty.text = locals::named_type(&ty.text)?;
        if ty.scope == self.file
            && !self.workspace.receiver_in_scope(
                self.file,
                &ty.text,
                self.locals,
                self.base + expression.span.start,
            )
        {
            return None;
        }
        Some(ty)
    }

    fn scoped_field_type(
        &self,
        receiver: ScopedType<'workspace>,
        member: MemberDeclaration<'workspace>,
        offset: usize,
    ) -> Option<ScopedType<'workspace>> {
        if member.kind != crate::syntax::MemberKind::Field {
            return None;
        }
        let data = self.workspace.member_data(member)?;
        let text = self.workspace.instantiated_member_type(
            receiver.scope,
            &receiver.text,
            member,
            data.annotation.as_deref()?,
        )?;
        if receiver.scope == self.file
            && let Some(name) = locals::named_type(&text)
            && self
                .locals
                .resolve_at(offset, locals::named_root(&name).split('.').next()?)
                .is_some()
        {
            return None;
        }
        self.scoped_value_type(receiver.scope, &text)
    }

    pub(super) fn scoped_expression(
        &self,
        id: ExprId,
        result: usize,
        count: usize,
    ) -> Option<ScopedType<'workspace>> {
        if self.depth >= 32 {
            return None;
        }
        let expression = self.unparenthesized(id);
        if self.global_symbol(id).is_some() {
            return self.scoped_global(id);
        }
        if result == 0 && count == 1 && matches!(expression.kind, ExprKind::Selector { .. }) {
            let mut arena = TypeArena::new(self.workspace);
            if let Some(input) = self.selector_operand(&mut arena, id) {
                let input = input?;
                return self.scoped_native_type(
                    &mut arena,
                    input.ty,
                    self.file,
                    self.base + expression.span.start,
                );
            }
        }
        if let ExprKind::Selector { operand, .. } = &expression.kind
            && self.global_root(*operand)
        {
            let mut arena = TypeArena::new(self.workspace);
            let field = self
                .function_operand(&mut arena, id)
                .or_else(|| self.global_field(&mut arena, id))?;
            return self.scoped_native_type(
                &mut arena,
                field.ty,
                self.file,
                self.base + expression.span.start,
            );
        }
        match &expression.kind {
            ExprKind::Call {
                function,
                arguments,
                variadic,
            } => self.call_result(*function, arguments, *variadic, result, count),
            ExprKind::Index { .. } => self.scoped_index(id, result, count),
            ExprKind::TypeAssertion { .. } => self.scoped_assertion(id, result, count),
            ExprKind::Slice { .. } => self.scoped_slice(id, result, count),
            ExprKind::Unary { operator, .. } if self.text(operator) == "<-" => {
                self.scoped_receive(id, result, count)
            }
            ExprKind::Name => {
                let Some(binding) = self.locals.resolve_at(
                    self.base + expression.span.start,
                    self.text(&expression.span),
                ) else {
                    return self.scoped(self.next().expression(id, result, count)?);
                };
                if self.locals.switch_case(binding).is_some() {
                    return self.scoped_switch_binding(binding);
                }
                if let Some(value) = self.scoped_binding(binding) {
                    return value;
                }
                if binding.range_initializer().is_some() {
                    return self.scoped_range_binding(binding);
                }
                if binding.declared_type.is_some() || binding.annotation().is_some() {
                    return self.scoped(self.next().expression(id, result, count)?);
                }
                let (span, result, count) = binding.initializer()?;
                let parsed = crate::syntax::parse_expression(self.source.get(span.clone())?);
                if !parsed.diagnostics.is_empty() {
                    return None;
                }
                Inference {
                    expressions: &parsed.nodes,
                    base: span.start,
                    depth: self.depth + 1,
                    ..*self
                }
                .scoped_expression(parsed.root, result, count)
            }
            ExprKind::Selector { operand, field } if result == 0 && count == 1 => {
                let operand = self.unparenthesized(*operand);
                if matches!(operand.kind, ExprKind::Name)
                    && self
                        .locals
                        .resolve_at(self.base + operand.span.start, self.text(&operand.span))
                        .is_none()
                {
                    return self.scoped(self.next().expression(id, result, count)?);
                }
                let receiver = self.scoped_receiver_type(id)?;
                match self.workspace.scoped_receiver_member(
                    self.file,
                    receiver.scope,
                    &receiver.text,
                    self.text(field),
                )? {
                    ReceiverMember::Declaration(member) => {
                        self.scoped_field_type(receiver, member, self.base + expression.span.start)
                    }
                    ReceiverMember::Method(_, _) => {
                        self.scoped(self.next().expression(id, result, count)?)
                    }
                }
            }
            ExprKind::Unary { operator, .. } if matches!(self.text(operator), "&" | "*") => {
                if result != 0 || count != 1 {
                    return None;
                }
                let mut arena = TypeArena::new(self.workspace);
                let input = self.value_operand(&mut arena, id)?;
                self.scoped_native_type(
                    &mut arena,
                    input.ty,
                    self.file,
                    self.base + expression.span.start,
                )
            }
            _ => self.scoped(self.next().expression(id, result, count)?),
        }
    }
}
