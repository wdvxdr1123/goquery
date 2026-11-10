//! Follow stored callable declarations and initializers with the inference budget.
use super::Inference;
use crate::analysis::scope::{self as locals, Binding};
use crate::syntax::{Lexer, TokenKind};

impl Inference<'_, '_> {
    pub(super) fn binding_result(
        &self,
        binding: &Binding,
        result: usize,
        count: usize,
    ) -> Option<String> {
        if self.depth >= 32 || binding.object_kind.is_type() {
            return None;
        }
        if let Some(annotation) = binding.annotation() {
            return self.next().function_type_result(
                self.source.get(annotation.clone())?,
                annotation.start,
                result,
                count,
            );
        }
        let (span, slot, arity) = binding.initializer()?;
        if slot != 0 || arity != 1 {
            return None;
        }
        let text = self.source.get(span.clone())?;
        if text.len() > 16 * 1024 {
            return None;
        }
        let parsed = crate::syntax::parse_expression(text);
        if !parsed.diagnostics.is_empty() {
            return None;
        }
        Inference {
            expressions: &parsed.nodes,
            base: span.start,
            depth: self.depth + 1,
            ..*self
        }
        .call_result(parsed.root, &[], false, result, count)?
        .render(self.workspace, self.file)
    }

    pub(super) fn function_type_result(
        &self,
        annotation: &str,
        offset: usize,
        result: usize,
        count: usize,
    ) -> Option<String> {
        if self.depth >= 32 || annotation.len() > 16 * 1024 {
            return None;
        }
        if !crate::syntax::parse_type(annotation).diagnostics.is_empty() {
            return None;
        }
        let first = Lexer::new(annotation).next()?;
        if first.kind == TokenKind::LParen {
            let last = Lexer::new(annotation)
                .filter(|token| token.kind != TokenKind::Semicolon)
                .last()?;
            if last.kind != TokenKind::RParen {
                return None;
            }
            return self.next().function_type_result(
                annotation.get(first.span.end..last.span.start)?,
                offset + first.span.end,
                result,
                count,
            );
        }
        if first.kind != TokenKind::Func {
            if first.kind == TokenKind::Mul {
                return None;
            }
            let name = locals::named_type(annotation)?;
            if let Some(binding) = self.locals.resolve_at(offset, locals::named_root(&name)) {
                if !binding.object_kind.is_type() || name != locals::named_root(&name) {
                    return None;
                }
                let span = binding.annotation()?;
                return self.next().function_type_result(
                    self.source.get(span.clone())?,
                    span.start,
                    result,
                    count,
                );
            }
            let underlying = self
                .workspace
                .instantiated_underlying_type(self.file, &name)?;
            return self
                .next()
                .function_type_result(&underlying, offset, result, count);
        }
        let signature = crate::syntax::parse_literal_signature(annotation);
        if !signature.diagnostics.is_empty() || signature.results.len() != count {
            return None;
        }
        let result_type = &annotation[signature.results.get(result)?.annotation.clone()];
        if locals::named_type(result_type).is_none() {
            return self
                .workspace
                .inference_value_type(self.file, self.file, result_type);
        }
        let name = locals::named_type(result_type)?;
        let root = locals::named_root(&name).split('.').next()?;
        if self.locals.resolve_at(offset, root).is_some() {
            return None;
        }
        self.workspace.result_type(self.file, self.file, &name)
    }
}
