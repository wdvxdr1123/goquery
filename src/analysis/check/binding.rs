//! Query-local variable initialization facts shared by semantic consumers.
use super::scope::AnnotationType;
use super::{Inference, ScopedType};
use crate::analysis::scope::{Binding, DeclarationKind, Locals};
use crate::analysis::types::{Mode, Operand, TypeArena};
use crate::syntax::{ExprId, ExprKind};
use crate::workspace::Workspace;
use std::path::Path;

pub(crate) fn binding_detail(
    workspace: &Workspace,
    file: &Path,
    source: &str,
    locals: &Locals,
    binding: &Binding,
) -> Option<String> {
    let inference = Inference::new(workspace, file, source, locals, &[], 0);
    // One arena serves the native check and its scoped projection, so detail
    // presentation does not repeat the same binding check in a second arena.
    let mut arena = TypeArena::new(workspace);
    match inference.local_binding(&mut arena, binding) {
        Some(Some(value)) => {
            if let Some(scoped) =
                inference.scoped_native_type(&mut arena, value.ty, file, binding.selection.start)
            {
                return scoped.render(workspace, file);
            }
            // A local defined type has no import spelling to prove by reparsing;
            // its same-file declaration rendering is display-only, never identity.
            if arena.has_local_declaration(value.ty)? {
                return arena.render(value.ty, file);
            }
            None
        }
        // A represented failure must not fall back to a legacy spelling.
        Some(None) => None,
        None => inference
            .scoped_switch_binding(binding)?
            .render(workspace, file),
    }
}

impl<'workspace> Inference<'workspace, '_> {
    /// Outer None marks unrepresented local types. A represented but failed
    /// initializer never falls back to text.
    pub(super) fn local_binding(
        &self,
        arena: &mut TypeArena<'_>,
        binding: &Binding,
    ) -> Option<Option<Operand>> {
        arena.local_value(self.file, binding.selection.start, |arena| {
            self.check_local_binding(arena, binding)
        })
    }

    fn check_local_binding(
        &self,
        arena: &mut TypeArena<'_>,
        binding: &Binding,
    ) -> Option<Option<Operand>> {
        if self.locals.switch_case(binding).is_some() {
            return None;
        }
        if !matches!(
            binding.declaration,
            DeclarationKind::Value | DeclarationKind::Variadic
        ) {
            return None;
        }
        if self.depth >= 32 {
            return Some(None);
        }
        if binding.range_initializer().is_some() {
            return Some(self.range_binding(arena, binding));
        }
        let target = if let Some(span) = binding.annotation() {
            let text = self.source.get(span.clone())?;
            match self.annotation_type(arena, text, span.start) {
                AnnotationType::Resolved(ty) => Some(ty),
                // An unsupported local type keeps its syntactic navigation path
                // instead of publishing a fabricated value type.
                AnnotationType::UnsupportedLocal => return None,
                AnnotationType::Unknown => return Some(None),
            }
        } else if binding.declaration == DeclarationKind::Variadic {
            match self.variadic_type(arena, binding) {
                Some(ty) => Some(ty),
                None => return Some(None),
            }
        } else {
            None
        };
        Some((|| {
            let ty = if let Some((span, result, count)) = binding.initializer() {
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
                let input = if result == 0 && count == 1 {
                    inference.value_operand(arena, parsed.root)?
                } else {
                    inference.tuple_operand(arena, parsed.root, result, count)?
                };
                if let Some(target) = target {
                    arena
                        .assign_value(input, target, self.workspace.integer_bits()?)?
                        .then_some(())?;
                    target
                } else {
                    arena.default_operand(input, self.workspace.integer_bits()?)?
                }
            } else {
                target?
            };
            arena.constructor_map_keys(ty)?;
            Some(Operand {
                mode: Mode::Variable,
                ty,
                value: None,
            })
        })())
    }

    #[cfg(test)]
    pub(crate) fn test_binding(
        &self,
        arena: &mut TypeArena<'_>,
        binding: &Binding,
    ) -> Option<Option<Operand>> {
        self.local_binding(arena, binding)
    }

    pub(super) fn scoped_binding(
        &self,
        binding: &Binding,
    ) -> Option<Option<ScopedType<'workspace>>> {
        let mut arena = TypeArena::new(self.workspace);
        self.local_binding(&mut arena, binding).map(|value| {
            let value = value?;
            self.scoped_native_type(&mut arena, value.ty, self.file, binding.selection.start)
        })
    }

    pub(super) fn tuple_operand(
        &self,
        arena: &mut TypeArena<'_>,
        id: ExprId,
        result: usize,
        count: usize,
    ) -> Option<Operand> {
        match &self.unparenthesized(id).kind {
            ExprKind::Call {
                function,
                arguments,
                variadic,
            } => {
                if let Some(value) =
                    self.native_call(arena, *function, arguments, *variadic, result, count)
                {
                    return value;
                }
                let value = self.call_result(*function, arguments, *variadic, result, count)?;
                Some(Operand {
                    mode: Mode::Value,
                    ty: arena.parse(value.scope, &value.text)?,
                    value: None,
                })
            }
            ExprKind::Index { .. } => self.indexed_operand(arena, id, result, count),
            ExprKind::TypeAssertion { .. } => self.assertion_operand(arena, id, result, count),
            ExprKind::Unary { operator, .. } if self.text(operator) == "<-" => {
                self.receive_operand(arena, id, result, count)
            }
            _ => None,
        }
    }
}
