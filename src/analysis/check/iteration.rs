//! Range bindings retain only source spans; iteration types remain query-local.
use super::{Inference, ScopedType};
use crate::analysis::scope::{Binding, Locals};
use crate::analysis::types::{Mode, Operand, TypeArena};
use crate::syntax::{ExprId, ExprKind};
use crate::workspace::Workspace;
use std::path::Path;

impl Workspace {
    pub(super) fn infer_range_binding(
        &self,
        file: &Path,
        source: &str,
        locals: &Locals,
        binding: &Binding,
        depth: usize,
    ) -> Option<String> {
        Inference {
            workspace: self,
            file,
            source,
            locals,
            expressions: &[],
            base: 0,
            depth,
            name_index: None,
            iota: None,
        }
        .scoped_range_binding(binding)?
        .render(self, file)
    }
}

/// Rendered receiver type and field name for a selector whose receiver is an
/// interface value. Highlighting uses this for method identities that have no
/// workspace declaration (the universe `error` interface), reusing the checked
/// receiver operand rather than a spelling heuristic.
pub(crate) fn interface_selector(
    workspace: &Workspace,
    file: &Path,
    source: &str,
    locals: &Locals,
    span: std::ops::Range<usize>,
) -> Option<(String, String)> {
    let text = source.get(span.clone())?;
    if text.len() > 16 * 1024 {
        return None;
    }
    let parsed = crate::syntax::parse_expression(text);
    if !parsed.diagnostics.is_empty() {
        return None;
    }
    let ExprKind::Selector { operand, field } = &parsed.nodes[parsed.root.0].kind else {
        return None;
    };
    let inference = Inference::new(workspace, file, source, locals, &parsed.nodes, span.start);
    let mut arena = TypeArena::new(workspace);
    let input =
        super::classify::receiver_operand(&inference, &mut arena, &parsed.nodes, *operand, 0)?;
    if !arena.is_interface(input.ty)? {
        return None;
    }
    Some((
        arena.render(input.ty, file)?,
        text.get(field.clone())?.to_owned(),
    ))
}

impl<'workspace> Inference<'workspace, '_> {
    /// Native range element projection for a written range expression span.
    /// Resolver-side field selectors use this instead of re-running legacy
    /// binding inference; no expression tree is retained.
    pub(crate) fn native_range_slot(
        &self,
        span: std::ops::Range<usize>,
        result: usize,
        count: usize,
    ) -> Option<ScopedType<'workspace>> {
        if self.depth >= 32 || span.len() > 16 * 1024 {
            return None;
        }
        let text = self.source.get(span.clone())?;
        let parsed = crate::syntax::parse_expression(text);
        if !parsed.diagnostics.is_empty() {
            return None;
        }
        let nested = Inference {
            workspace: self.workspace,
            file: self.file,
            source: self.source,
            locals: self.locals,
            expressions: &parsed.nodes,
            base: span.start,
            depth: self.depth + 1,
            name_index: None,
            iota: None,
        };
        let mut arena = TypeArena::new(self.workspace);
        let container =
            super::classify::receiver_operand(&nested, &mut arena, &parsed.nodes, parsed.root, 1)?;
        let ty = arena.range_slot(container, result, count, self.workspace.integer_bits()?)?;
        self.scoped_native_type(&mut arena, ty, self.file, span.start)
    }

    pub(super) fn scoped_range_binding(&self, binding: &Binding) -> Option<ScopedType<'workspace>> {
        let mut arena = TypeArena::new(self.workspace);
        let value = self.range_binding(&mut arena, binding)?;
        let (span, _, _) = binding.range_initializer()?;
        self.scoped_native_type(&mut arena, value.ty, self.file, span.start)
    }

    pub(super) fn range_binding(
        &self,
        arena: &mut TypeArena<'_>,
        binding: &Binding,
    ) -> Option<Operand> {
        if self.depth >= 32 {
            return None;
        }
        let (span, result, count) = binding.range_initializer()?;
        let text = self
            .source
            .get(span.clone())
            .filter(|text| text.len() <= 16 * 1024)?;
        let parsed = crate::syntax::parse_expression(text);
        if !parsed.diagnostics.is_empty() {
            return None;
        }
        let inference = Inference {
            expressions: &parsed.nodes,
            base: span.start,
            depth: self.depth + 1,
            ..*self
        };
        let input = inference
            .value_operand(arena, parsed.root)
            .or_else(|| inference.range_operand(arena, parsed.root))
            .or_else(|| inference.parameterized_operand(arena, parsed.root))?;
        let ty = arena.range_slot(input, result, count, self.workspace.integer_bits()?)?;
        Some(Operand {
            mode: Mode::Variable,
            ty,
            value: None,
        })
    }
    fn range_operand(&self, arena: &mut TypeArena<'_>, id: ExprId) -> Option<Operand> {
        let expression = self.unparenthesized(id);
        if matches!(expression.kind, ExprKind::Name)
            && self
                .locals
                .resolve_at(
                    self.base + expression.span.start,
                    self.text(&expression.span),
                )
                .is_none()
            && let Some(input) = self.function_operand(arena, id)
        {
            return Some(input);
        }
        let Some(scoped) = self.scoped_expression(id, 0, 1) else {
            return self.function_operand(arena, id);
        };
        if scoped.scope == self.file
            && !self.concrete_spelling(&scoped.text, self.base + expression.span.start)
        {
            return None;
        }
        Some(Operand {
            mode: Mode::Value,
            ty: arena.parse(scoped.scope, &scoped.text)?,
            value: None,
        })
    }
}
