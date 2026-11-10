//! One checked assertion projection for values, tuple bindings and navigation.
use super::{Inference, ScopedType};
use crate::analysis::scope::{Binding, universe};
use crate::analysis::types::{Operand, TypeArena};
use crate::syntax::{ExprId, ExprKind, Lexer, TokenKind};
use std::ops::Range;

impl<'workspace> Inference<'workspace, '_> {
    pub(super) fn switch_input(
        &self,
        arena: &mut TypeArena<'_>,
        binding: &Binding,
    ) -> Option<Operand> {
        if self.depth >= 32 {
            return None;
        }
        let case = self.locals.switch_case(binding)?;
        let text = self
            .source
            .get(case.operand.clone())
            .filter(|text| text.len() <= 16 * 1024)?;
        let parsed = crate::syntax::parse_expression(text);
        if !parsed.diagnostics.is_empty() {
            return None;
        }
        let inference = Inference {
            expressions: &parsed.nodes,
            base: case.operand.start,
            depth: self.depth + 1,
            ..*self
        };
        let input = inference.value_operand(arena, parsed.root)?;
        arena.is_interface(input.ty)?.then_some(())?;
        Some(input)
    }

    pub(super) fn switch_operand(
        &self,
        arena: &mut TypeArena<'_>,
        binding: &Binding,
    ) -> Option<Operand> {
        let input = self.switch_input(arena, binding)?;
        let case = self.locals.switch_case(binding)?;
        // A truncated group cannot prove the absence of duplicate cases.
        if self.locals.switch_clauses(case).count() != case.clauses {
            return None;
        }
        let mut targets = Vec::new();
        let mut selected = 0..0;
        let mut defaults = 0;
        for clause in self.locals.switch_clauses(case) {
            let start = targets.len();
            if clause.default {
                defaults += 1;
            }
            if clause.types.is_empty() && !clause.default {
                return None;
            }
            for span in &clause.types {
                let target = if self.nil_case(span)? {
                    None
                } else {
                    Some(Inference { base: 0, ..*self }.type_target(arena, span)?)
                };
                targets.push(target);
            }
            if std::ptr::eq(clause, case) {
                selected = start..targets.len();
            }
        }
        arena.type_switch(
            input,
            &targets,
            selected,
            defaults,
            self.workspace.integer_bits()?,
        )
    }

    /// Like go/types isNil, recognize only the universe nil value after
    /// parentheses are removed. A same-spelled type remains a type case.
    fn nil_case(&self, span: &Range<usize>) -> Option<bool> {
        let text = self
            .source
            .get(span.clone())
            .filter(|text| text.len() <= 16 * 1024)?;
        let token = Lexer::new(text).next()?;
        let selection = if token.kind == TokenKind::LParen {
            let parsed = crate::syntax::parse_expression(text);
            if !parsed.diagnostics.is_empty() {
                return Some(false);
            }
            let mut id = parsed.root;
            for _ in 0..32 {
                let expression = &parsed.nodes[id.0];
                if let ExprKind::Parenthesized { expression } = expression.kind {
                    id = expression;
                    continue;
                }
                if !matches!(expression.kind, ExprKind::Name) {
                    return Some(false);
                }
                return Some(
                    &text[expression.span.clone()] == "nil"
                        && universe::visible(
                            self.workspace,
                            self.file,
                            Some(self.locals),
                            span.start + expression.span.start,
                            "nil",
                        ),
                );
            }
            return None;
        } else if token.kind == TokenKind::Ident {
            if &text[token.span.clone()] != "nil"
                || Lexer::new(text)
                    .skip(1)
                    .any(|token| token.kind != TokenKind::Semicolon || !token.inserted)
            {
                return Some(false);
            }
            token.span
        } else {
            return Some(false);
        };
        Some(
            &text[selection.clone()] == "nil"
                && universe::visible(
                    self.workspace,
                    self.file,
                    Some(self.locals),
                    span.start + selection.start,
                    "nil",
                ),
        )
    }

    pub(super) fn scoped_switch_binding(
        &self,
        binding: &Binding,
    ) -> Option<ScopedType<'workspace>> {
        let mut arena = TypeArena::new(self.workspace);
        let value = self.switch_operand(&mut arena, binding)?;
        self.scoped_native_type(&mut arena, value.ty, self.file, binding.scope_start())
    }

    pub(super) fn assertion_operand(
        &self,
        arena: &mut TypeArena<'_>,
        id: ExprId,
        result: usize,
        count: usize,
    ) -> Option<Operand> {
        if self.depth >= 32 {
            return None;
        }
        let ExprKind::TypeAssertion {
            operand,
            asserted_type,
        } = &self.unparenthesized(id).kind
        else {
            return None;
        };
        let target = self.type_target(arena, asserted_type)?;
        let source = self.next().value_operand(arena, *operand)?;
        arena.assertion(
            source,
            target,
            result,
            count,
            self.workspace.integer_bits()?,
        )
    }

    pub(super) fn scoped_assertion(
        &self,
        id: ExprId,
        result: usize,
        count: usize,
    ) -> Option<ScopedType<'workspace>> {
        let mut arena = TypeArena::new(self.workspace);
        let value = self.assertion_operand(&mut arena, id, result, count)?;
        self.scoped_native_type(
            &mut arena,
            value.ty,
            self.file,
            self.base + self.unparenthesized(id).span.start,
        )
    }
}
