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

    /// Hover and result inference share native unification, constraint proofs
    /// and assignment checks; signature labels only add declaration names.
    pub(crate) fn instantiated_call_signature(
        &self,
        declaration: (&Path, &str, &Signature),
        function: ExprId,
        arguments: &[ExprId],
        variadic: bool,
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
        let (actuals, explicit) =
            self.generic_call_operands(&mut arena, function, arguments, variadic)?;
        let inferred = arena.infer_call_arguments(
            origin,
            header,
            signature,
            &actuals,
            &explicit,
            self.workspace.integer_bits()?,
            variadic,
        )?;
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

    fn generic_call_operands(
        &self,
        arena: &mut TypeArena<'_>,
        function: ExprId,
        arguments: &[ExprId],
        variadic: bool,
    ) -> Option<(Vec<Operand>, Vec<TypeId>)> {
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
