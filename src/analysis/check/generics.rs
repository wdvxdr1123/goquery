//! Adapt expression operands to the shared structural call-inference core.
use super::callable_signature::{CallableSignature, signature_label};
use super::{Inference, ScopedType};
use crate::analysis::types::{Operand, TypeArena, TypeId};
use crate::syntax::{ExprId, ExprKind, Signature};
use std::path::Path;

impl<'workspace> Inference<'workspace, '_> {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn generic_result(
        &self,
        origin: &Path,
        header: &str,
        signature: &Signature,
        function: ExprId,
        arguments: &[ExprId],
        variadic: bool,
        result: usize,
    ) -> Option<ScopedType<'workspace>> {
        let mut arena = TypeArena::new(self.workspace);
        let (actuals, explicit) =
            self.generic_call_operands(&mut arena, function, arguments, variadic)?;
        let ty = arena.infer_call(
            origin,
            header,
            signature,
            &actuals,
            &explicit,
            result,
            self.workspace.integer_bits()?,
            variadic,
        )?;
        self.scoped_native_type(
            &mut arena,
            ty,
            origin,
            self.base + self.expressions[function.0].span.start,
        )
    }

    /// Presentation reuses native instantiation proofs; names stay out of the
    /// compact structural type arena. Calls additionally infer missing arguments.
    pub(crate) fn instantiated_function_signature(
        &self,
        declaration: (&Path, &str, &Signature),
        function: ExprId,
        call: Option<(&[ExprId], bool)>,
    ) -> Option<CallableSignature> {
        let (origin, header, signature) = declaration;
        let (resolved, symbol) = self.callee(function)?;
        if resolved != origin || !symbol.is_function() || !symbol.generic {
            return None;
        }
        if header.len() > 16 * 1024 || !signature.diagnostics.is_empty() {
            return None;
        }
        let mut arena = TypeArena::new(self.workspace);
        let inferred = match call {
            Some((arguments, variadic)) => {
                let (actuals, explicit) =
                    self.generic_call_operands(&mut arena, function, arguments, variadic)?;
                arena.infer_call_arguments(
                    origin,
                    header,
                    signature,
                    &actuals,
                    &explicit,
                    self.workspace.integer_bits()?,
                    variadic,
                )?
            }
            None => {
                let explicit = self.explicit_type_arguments(&mut arena, function)?;
                arena.instantiated_signature(origin, header, signature, &explicit)?;
                explicit
            }
        };
        let environment: Vec<_> = signature
            .type_parameters
            .iter()
            .map(|parameter| header.get(parameter.name.as_ref()?.clone()))
            .collect::<Option<Vec<_>>>()?
            .into_iter()
            .zip(inferred)
            .collect();
        signature_label(header, signature, &symbol.name, |annotation| {
            let ty = arena.parse_in(origin, annotation, &environment)?;
            arena.render_resolved(ty, self.file)
        })
        .map(|(signature, _)| signature)
    }

    /// Fully explicit function values have no call operands to infer from.
    pub(crate) fn explicit_function_signature(
        &self,
        function: ExprId,
    ) -> Option<(CallableSignature, bool)> {
        let (origin, symbol) = self.callee(function)?;
        if !symbol.is_function() || !symbol.generic {
            return None;
        }
        let declaration = symbol.declaration()?;
        let end = symbol.body().map_or(declaration.end, |body| body.start);
        if end.checked_sub(declaration.start)? > 16 * 1024 {
            return None;
        }
        let header = self
            .workspace
            .source_slice(origin, declaration.start..end)?;
        let signature = crate::syntax::parse_signature(&header);
        let label =
            self.instantiated_function_signature((origin, &header, &signature), function, None)?;
        Some((
            label,
            signature.parameters.last().is_some_and(|p| p.variadic),
        ))
    }

    fn generic_call_operands(
        &self,
        arena: &mut TypeArena<'_>,
        function: ExprId,
        arguments: &[ExprId],
        variadic: bool,
    ) -> Option<(Vec<Operand>, Vec<TypeId>)> {
        let explicit = self.explicit_type_arguments(arena, function)?;
        let actuals = self.next().call_operands(arena, arguments, variadic)?;
        for (index, operand) in actuals.iter().enumerate() {
            let argument = arguments.get(index).or_else(|| arguments.first())?;
            let expression = self.unparenthesized(*argument);
            if !arena
                .basic_kind(operand.ty)
                .is_some_and(|kind| kind.untyped())
                && let Some(text) = arena.render(operand.ty, self.file)
                && !self.concrete_spelling(&text, self.base + expression.span.start)
            {
                return None;
            }
        }
        Some((actuals, explicit))
    }

    fn explicit_type_arguments(
        &self,
        arena: &mut TypeArena<'_>,
        function: ExprId,
    ) -> Option<Vec<TypeId>> {
        let mut explicit = Vec::new();
        if let ExprKind::Index { arguments, .. } = &self.unparenthesized(function).kind {
            if arguments.len() > 32 {
                return None;
            }
            for argument in arguments {
                let span = &self.expressions[argument.0].span;
                let annotation = self.text(span);
                if !self.concrete_spelling(annotation, self.base + span.start) {
                    return None;
                }
                explicit.push(arena.parse(self.file, annotation)?);
            }
        }
        Some(explicit)
    }

    pub(super) fn concrete_spelling(&self, text: &str, offset: usize) -> bool {
        let parsed = crate::syntax::parse_type(text);
        parsed.diagnostics.is_empty()
            && parsed.references.iter().all(|reference| {
                let span = reference.qualifier.as_ref().unwrap_or(&reference.name);
                self.locals
                    .resolve_at(offset, &text[span.clone()])
                    .is_none()
            })
    }
}
