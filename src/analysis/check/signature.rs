//! Function values retain type identities while their bodies stay opaque.
use super::{Inference, Selection};
use crate::analysis::scope as locals;
use crate::analysis::types::{Mode, Operand, TypeArena, TypeId};
use crate::syntax::{ExprId, ExprKind};

impl<'workspace> Inference<'workspace, '_> {
    /// Signature arity is independent of actual arguments. Read a resolved
    /// function header before native operand construction, which may need
    /// unavailable dependency types just to describe otherwise known inputs.
    pub(crate) fn format_parameter(&self, function: ExprId) -> Option<usize> {
        if let Some((origin, symbol)) = self.callee(function)
            && symbol.is_function()
        {
            let declaration = symbol.declaration()?;
            let end = symbol.body().map_or(declaration.end, |body| body.start);
            if end.checked_sub(declaration.start)? > 16 * 1024 {
                return None;
            }
            let header = self
                .workspace
                .source_slice(origin, declaration.start..end)?;
            let signature = crate::syntax::parse_signature(&header);
            return (signature.diagnostics.is_empty()
                && signature.parameters.last().is_some_and(|p| p.variadic))
            .then(|| signature.parameters.len().checked_sub(2))
            .flatten();
        }
        let mut arena = TypeArena::new(self.workspace);
        let callable = self.value_operand(&mut arena, function)?;
        arena.format_parameter(callable.ty)
    }

    fn method_receiver_type(&self, arena: &mut TypeArena<'_>, id: ExprId) -> Option<TypeId> {
        let receiver = self.unparenthesized(id);
        if matches!(receiver.kind, ExprKind::Name)
            && self
                .locals
                .resolve_at(self.base + receiver.span.start, self.text(&receiver.span))
                .is_some()
        {
            return None;
        }
        self.conversion_target(arena, id)
    }

    pub(super) fn method_expression_target(&self, id: ExprId) -> Option<Selection<'workspace>> {
        if self.depth >= 32 {
            return None;
        }
        let ExprKind::Selector { operand, field } = &self.unparenthesized(id).kind else {
            return None;
        };
        let mut arena = TypeArena::new(self.workspace);
        let receiver = self.method_receiver_type(&mut arena, *operand)?;
        if let Some((file, symbol)) =
            arena.method_declaration(receiver, self.file, self.text(field))
        {
            let file = self.workspace.files.get_key_value(file)?.0;
            return Some(Selection::Global(file, symbol));
        }
        // Interface members have metadata member declarations rather than
        // top-level symbols. Native lookup still proves the strict selection.
        arena.method_expression(receiver, self.file, self.text(field))?;
        let name = arena.render_resolved(receiver, self.file)?;
        match self.workspace.scoped_receiver_member(
            self.file,
            self.file,
            &name,
            self.text(field),
        )? {
            crate::analysis::method_set::ReceiverMember::Declaration(member)
                if member.kind == crate::syntax::MemberKind::Method =>
            {
                Some(Selection::Member(member))
            }
            _ => None,
        }
    }

    /// Fields and methods share one native receiver/lookup operation. Checked
    /// selector failures must not become a spelling-based callable fallback.
    pub(super) fn selector_operand(
        &self,
        arena: &mut TypeArena<'_>,
        id: ExprId,
    ) -> Option<Option<Operand>> {
        if self.depth >= 32 {
            return Some(None);
        }
        let ExprKind::Selector { operand, field } = &self.unparenthesized(id).kind else {
            return None;
        };
        if let Some(receiver) = self.method_receiver_type(arena, *operand) {
            return Some(arena.method_expression(receiver, self.file, self.text(field)));
        }
        let expression = self.unparenthesized(*operand);
        if matches!(expression.kind, ExprKind::Name)
            && self
                .locals
                .resolve_at(
                    self.base + expression.span.start,
                    self.text(&expression.span),
                )
                .is_none()
            && crate::analysis::scope::universe::imported_name(
                self.workspace,
                self.file,
                self.text(&expression.span),
            )
        {
            return None;
        }
        let input = self.next().value_operand(arena, *operand)?;
        arena.selector_operand(input, self.file, self.text(field))
    }

    pub(super) fn call_operands(
        &self,
        arena: &mut TypeArena<'_>,
        arguments: &[ExprId],
        spread: bool,
    ) -> Option<Vec<Operand>> {
        if self.depth >= 32 || arguments.len() > 256 {
            return None;
        }
        let actuals = arguments
            .iter()
            .map(|argument| self.next().value_operand(arena, *argument))
            .collect::<Option<Vec<_>>>();
        if actuals.is_some() || spread || arguments.len() != 1 {
            return actuals;
        }
        // Go permits a single multi-valued call as the complete argument list.
        // Result slots are values, never untyped constants or comma-ok tuples.
        let ExprKind::Call {
            function,
            arguments: inner,
            variadic,
        } = &self.unparenthesized(arguments[0]).kind
        else {
            return None;
        };
        let count = self.next().call_result_count(arena, *function)?;
        if !(2..=256).contains(&count) {
            return None;
        }
        let mut actuals = Vec::new();
        for slot in 0..count {
            let result = self
                .next()
                .call_result(*function, inner, *variadic, slot, count)?;
            actuals.push(Operand {
                mode: Mode::Value,
                ty: arena.parse(result.scope, &result.text)?,
                value: None,
            });
        }
        Some(actuals)
    }

    pub(super) fn call_result_count(
        &self,
        arena: &mut TypeArena<'_>,
        function: ExprId,
    ) -> Option<usize> {
        if self.depth >= 32 {
            return None;
        }
        if let Some(operand) = self.next().value_operand(arena, function) {
            return arena.result_count(operand.ty);
        }
        let (file, symbol) = self.next().callee(function)?;
        if !symbol.is_callable() {
            return None;
        }
        let declaration = symbol.declaration()?;
        let end = symbol.body().map_or(declaration.end, |body| body.start);
        let header = self.workspace.source_slice(file, declaration.start..end)?;
        let signature = crate::syntax::parse_signature(&header);
        signature
            .diagnostics
            .is_empty()
            .then_some(signature.results.len())
    }

    pub(super) fn function_operand(
        &self,
        arena: &mut TypeArena<'_>,
        id: ExprId,
    ) -> Option<Operand> {
        if self.depth >= 32 {
            return None;
        }
        let expression = self.unparenthesized(id);
        if let Some(operand) = self.selector_operand(arena, id) {
            return operand;
        }
        if let ExprKind::FunctionLiteral { signature, .. } = &expression.kind {
            let header = self.text(signature);
            if !self.concrete_spelling(header, self.base + signature.start) {
                return None;
            }
            let parsed = crate::syntax::parse_literal_signature(header);
            let ty = arena.signature(self.file, header, &parsed, &[], 0)?;
            return Some(Operand {
                mode: Mode::Value,
                ty,
                value: None,
            });
        }
        let (base, explicit) = if let ExprKind::Index { operand, arguments } = &expression.kind {
            (*operand, arguments.as_slice())
        } else {
            (id, &[][..])
        };
        let (file, symbol) = self.next().callee(base)?;
        if !symbol.is_callable() {
            return None;
        }
        if symbol.is_method() {
            self.next().scoped_receiver_type(base)?;
        }
        let start = symbol.declaration()?.start;
        let end = symbol
            .body()
            .map_or(symbol.declaration()?.end, |body| body.start);
        let header = self.workspace.source_slice(file, start..end)?;
        let signature = crate::syntax::parse_signature(&header);
        if signature
            .receiver
            .iter()
            .any(|receiver| !locals::receiver_parameters(&header, &receiver.annotation).is_empty())
        {
            return None;
        }
        if explicit.len() > 32 {
            return None;
        }
        let mut arguments = Vec::new();
        for id in explicit {
            let span = &self.expressions[id.0].span;
            let annotation = self.text(span);
            if !self.concrete_spelling(annotation, self.base + span.start) {
                return None;
            }
            arguments.push(arena.parse(self.file, annotation)?);
        }
        let ty = arena.instantiated_signature(file, &header, &signature, &arguments)?;
        Some(Operand {
            mode: Mode::Value,
            ty,
            value: None,
        })
    }
}
