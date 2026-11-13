//! Inferred array extents share the native constant/index checks.
use super::Inference;
use super::scope::AnnotationType;
use crate::analysis::query::budget::{MAX_BYTES, MAX_NODES};
use crate::analysis::scope::Locals;
use crate::analysis::types::{Mode, Operand, TypeArena};
use crate::syntax::{ExprId, ExprKind, Lexer, TokenKind};
use crate::workspace::Workspace;
use std::ops::Range;
use std::path::Path;

pub(crate) struct InferredArray {
    pub(crate) length: u64,
    pub(crate) text: String,
}

/// Package initializer entry point. Lexical expressions use the caller's own
/// inference so local constants, shadows and defined types retain identity.
/// Outer None is another expression; Some(None) must not fall back to `[...]`.
pub(crate) fn inferred_array_literal(
    workspace: &Workspace,
    file: &Path,
    text: &str,
) -> Option<Option<InferredArray>> {
    inferred_array_header(text)?;
    Some((|| {
        if text.len() > MAX_BYTES {
            return None;
        }
        let parsed = crate::syntax::parse_expression(text);
        if !parsed.diagnostics.is_empty() {
            return None;
        }
        let locals = Locals::empty();
        let inference = Inference::new(workspace, file, text, &locals, &parsed.nodes, 0);
        let mut arena = TypeArena::new(workspace);
        let (operand, length) = inference.inferred_array_operand(&mut arena, parsed.root)?;
        Some(InferredArray {
            length,
            text: arena.render_resolved(operand.ty, file)?,
        })
    })())
}

pub(crate) fn inferred_array_header(text: &str) -> Option<usize> {
    let mut tokens = Lexer::new(text);
    let mut first = tokens.next()?;
    for _ in 0..32 {
        if first.kind != TokenKind::LParen {
            break;
        }
        first = tokens.next()?;
    }
    (first.kind == TokenKind::LBrack).then_some(())?;
    (tokens.next()?.kind == TokenKind::Ellipsis).then_some(())?;
    let end = tokens.next()?;
    (end.kind == TokenKind::RBrack).then_some(end.span.end)
}

impl Inference<'_, '_> {
    pub(super) fn inferred_array_element(&self, literal: ExprId) -> Option<Range<usize>> {
        let span = &self.expressions[literal.0].span;
        Some(span.start + inferred_array_header(self.text(span))?..span.end)
    }

    pub(super) fn inferred_array_operand(
        &self,
        arena: &mut TypeArena<'_>,
        id: ExprId,
    ) -> Option<(Operand, u64)> {
        if self.depth >= 32 {
            return None;
        }
        let ExprKind::Composite {
            literal_type: Some(literal),
            elements,
        } = &self.unparenthesized(id).kind
        else {
            return None;
        };
        if elements.len() > MAX_NODES {
            return None;
        }
        let span = self.inferred_array_element(*literal)?;
        let AnnotationType::Resolved(element) =
            self.annotation_type(arena, self.text(&span), self.base + span.start)
        else {
            return None;
        };
        let int_bits = self.workspace.integer_bits()?;
        // Bounded to MAX_NODES entries; do not allocate by the largest index.
        let mut seen = Vec::new();
        let mut next = 0_u64;
        let mut length = 0_u64;
        for element in elements {
            let index = if let ExprKind::KeyValue { key, .. } = &self.expressions[element.0].kind {
                let operand = self.next().scalar_operand(arena, *key)?;
                (operand.mode == Mode::Constant).then_some(())?;
                arena.index_value(operand, int_bits)??
            } else {
                next
            };
            if seen.contains(&index) {
                return None;
            }
            seen.push(index);
            next = index.checked_add(1)?;
            length = length.max(next);
        }
        Some((
            Operand {
                mode: Mode::Value,
                ty: arena.array(element, length)?,
                value: None,
            },
            length,
        ))
    }
}
