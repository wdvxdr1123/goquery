//! Adapt expression operands to the shared structural call-inference core.
use super::{Inference, ScopedType};
use crate::analysis::types::TypeArena;
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
        let actuals = self.next().call_operands(&mut arena, arguments, variadic)?;
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
