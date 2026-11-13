//! Named results are inferred from instantiated headers, without parsing bodies.
use super::{Inference, ScopedType, Selection};
use crate::analysis::scope as locals;
use crate::analysis::types::TypeArena;
use crate::syntax::{ExprId, ExprKind};

impl<'workspace> Inference<'workspace, '_> {
    pub(super) fn call_result(
        &self,
        function: ExprId,
        arguments: &[ExprId],
        variadic: bool,
        result: usize,
        count: usize,
    ) -> Option<ScopedType<'workspace>> {
        if self.depth >= 32 {
            return None;
        }
        let callee = self.unparenthesized(function);
        let mut arena = TypeArena::new(self.workspace);
        if let Some(name) = self.builtin_name(function) {
            if count != 1 || result != 0 {
                return None;
            }
            let operand = self.builtin_operand(&mut arena, name, arguments, variadic)?;
            let ty = arena.default_operand(operand, self.workspace.integer_bits()?)?;
            return self.scoped_native_type(
                &mut arena,
                ty,
                self.file,
                self.base + callee.span.start,
            );
        }
        if let Some(target) = self.conversion_target(&mut arena, function) {
            if count != 1 || result != 0 || arguments.len() != 1 || variadic {
                return None;
            }
            let operand = self.next().value_operand(&mut arena, arguments[0])?;
            let converted = arena.convert(operand, target, self.workspace.integer_bits()?)?;
            return self.scoped(arena.render_resolved(converted.ty, self.file)?);
        }
        if (matches!(callee.kind, ExprKind::Selector { .. })
            || self.global_symbol(function).is_some()
            || self.global_root(function))
            && let Some(input) =
                self.native_call(&mut arena, function, arguments, variadic, result, count)
        {
            let input = input?;
            return self.scoped_native_type(
                &mut arena,
                input.ty,
                self.file,
                self.base + callee.span.start,
            );
        }
        let local = matches!(callee.kind, ExprKind::Name)
            && self
                .locals
                .resolve_at(self.base + callee.span.start, self.text(&callee.span))
                .is_some();
        let function_value = local
            || matches!(
                callee.kind,
                ExprKind::FunctionLiteral { .. }
                    | ExprKind::Index { .. }
                    | ExprKind::Call { .. }
                    | ExprKind::Unary { .. }
                    | ExprKind::TypeAssertion { .. }
            )
            || matches!(callee.kind, ExprKind::Selector { .. })
                && self.next().callee(function).is_none();
        if function_value
            && let Some(callable) = self.next().value_operand(&mut arena, function)
            && arena.is_callable(callable.ty) == Some(true)
        {
            let actuals = self.next().call_operands(&mut arena, arguments, variadic)?;
            let result = arena.call_signature(
                callable.ty,
                &actuals,
                variadic,
                result,
                count,
                self.workspace.integer_bits()?,
            )?;
            return self.scoped_native_type(
                &mut arena,
                result,
                self.file,
                self.base + callee.span.start,
            );
        }
        if matches!(callee.kind, ExprKind::Name)
            && let Some(binding) = self
                .locals
                .resolve_at(self.base + callee.span.start, self.text(&callee.span))
        {
            return self.scoped(self.next().binding_result(binding, result, count)?);
        }
        if let ExprKind::FunctionLiteral { signature, .. } = &callee.kind {
            return self.scoped(self.next().function_type_result(
                self.text(signature),
                self.base + signature.start,
                result,
                count,
            )?);
        }
        let Some((file, symbol)) = self.next().callee(function) else {
            return self.member_result(function, result, count);
        };
        if symbol.is_type() {
            return None;
        }
        if !symbol.is_callable() {
            return None;
        }
        let start = symbol.declaration()?.start;
        let end = symbol
            .body()
            .map(|body| body.start)
            .unwrap_or(symbol.declaration()?.end);
        let header = self.workspace.source_slice(file, start..end)?;
        let signature = crate::syntax::parse_signature(&header);
        if !signature.diagnostics.is_empty() || signature.results.len() != count {
            return None;
        }
        if !signature.type_parameters.is_empty() {
            return self.generic_result(
                file, &header, &signature, function, arguments, variadic, result,
            );
        }
        let annotation = &signature.results.get(result)?.annotation;
        let names: Vec<_> = signature
            .receiver
            .iter()
            .flat_map(|receiver| locals::receiver_parameters(&header, &receiver.annotation))
            .map(|span| header[span].to_owned())
            .collect();
        if !names.is_empty() {
            let receiver = self.next().scoped_receiver_type(function)?;
            let types =
                self.workspace
                    .method_types(receiver.scope, &receiver.text, file, symbol, names)?;
            let instantiated = types.render(&header[annotation.clone()])?;
            if locals::named_type(&instantiated).is_none() {
                return self.scoped_value_type(receiver.scope, &instantiated);
            }
            let name = locals::named_type(&instantiated)?;
            let qualifier = locals::named_root(&name).split('.').next()?;
            if receiver.scope == self.file
                && self
                    .locals
                    .resolve_at(
                        self.base + self.expressions[function.0].span.start,
                        qualifier,
                    )
                    .is_some()
            {
                return None;
            }
            return self.scoped_value_type(receiver.scope, &instantiated);
        }
        // Keep the declaration's scope when its result has no spelling in the
        // importing file. Argument checking must not turn it into a local name.
        if (symbol.is_function() || self.next().scoped_receiver_type(function).is_some())
            && let Some(callable) = arena.signature(file, &header, &signature, &[], 0)
        {
            let actuals = self.next().call_operands(&mut arena, arguments, variadic)?;
            let result = arena.call_signature(
                callable,
                &actuals,
                variadic,
                result,
                count,
                self.workspace.integer_bits()?,
            )?;
            if let Some(annotation) = arena.render_resolved(result, self.file) {
                return self.scoped(annotation);
            }
            let annotation = arena.render_resolved(result, file)?;
            return self.scoped_value_type(file, &annotation);
        }
        self.scoped_value_type(file, &header[annotation.clone()])
    }

    fn member_result(
        &self,
        function: ExprId,
        result: usize,
        count: usize,
    ) -> Option<ScopedType<'workspace>> {
        let Selection::Member(member) = self.next().selection(function)? else {
            return None;
        };
        let receiver = self.next().scoped_receiver_type(function)?;
        let data = self.workspace.member_data(member)?;
        let annotation = data.annotation.as_deref()?;
        let header = if member.kind == crate::syntax::MemberKind::Method {
            format!("func {annotation}")
        } else {
            annotation.to_owned()
        };
        let header = self.workspace.instantiated_member_type(
            receiver.scope,
            &receiver.text,
            member,
            &header,
        )?;
        let parsed = crate::syntax::parse_literal_signature(&header);
        if !parsed.diagnostics.is_empty() || parsed.results.len() != count {
            return None;
        }
        let annotation = &header[parsed.results.get(result)?.annotation.clone()];
        if locals::named_type(annotation).is_none() {
            return self.scoped_value_type(receiver.scope, annotation);
        }
        let name = locals::named_type(annotation)?;
        let qualifier = locals::named_root(&name).split('.').next()?;
        if receiver.scope == self.file
            && self
                .locals
                .resolve_at(
                    self.base + self.expressions[function.0].span.start,
                    qualifier,
                )
                .is_some()
        {
            return None;
        }
        self.scoped_value_type(receiver.scope, annotation)
    }
}
