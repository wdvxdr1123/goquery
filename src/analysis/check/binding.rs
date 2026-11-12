//! Query-local variable initialization facts shared by semantic consumers.
use super::scope::AnnotationType;
use super::{Inference, ScopedType};
use crate::analysis::scope::{Binding, DeclarationKind, Locals};
use crate::analysis::types::{Mode, Operand, TypeArena};
use crate::syntax::{ExprId, ExprKind};
use crate::workspace::Workspace;
use std::path::Path;

const MAX_SCOPED_BINDINGS: usize = 4_096;

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

    /// Memoized successful native type of a lexical binding, when the resolver
    /// shares one request-local map. Only `Type` entries are returned; a
    /// represented failure must keep its caller-specific fallback handling.
    pub(super) fn cached_scoped_binding(
        &self,
        binding: &Binding,
    ) -> Option<ScopedType<'workspace>> {
        let cache = self.scoped_bindings?;
        let key = (binding.selection.start, binding.selection.end);
        match cache.borrow().get(&key) {
            Some(super::CachedBinding::Type(scoped)) => Some(scoped.clone()),
            _ => None,
        }
    }

    /// Record the rendered type of a variable operand so a later inference
    /// instance can reparse it instead of re-walking the initializer.
    pub(super) fn memoize_scoped_binding(
        &self,
        arena: &mut TypeArena<'_>,
        binding: &Binding,
        ty: crate::analysis::types::TypeId,
    ) {
        let Some(cache) = self.scoped_bindings else {
            return;
        };
        if cache.borrow().len() >= MAX_SCOPED_BINDINGS {
            return;
        }
        let Some(text) = arena.render_resolved(ty, self.file) else {
            return;
        };
        if let Some(scoped) = self.scoped(text) {
            cache.borrow_mut().insert(
                (binding.selection.start, binding.selection.end),
                super::CachedBinding::Type(scoped),
            );
        }
    }

    pub(super) fn scoped_binding(
        &self,
        binding: &Binding,
    ) -> Option<Option<ScopedType<'workspace>>> {
        let key = (binding.selection.start, binding.selection.end);
        if let Some(cache) = self.scoped_bindings
            && let Some(entry) = cache.borrow().get(&key)
        {
            match entry {
                super::CachedBinding::Type(scoped) => return Some(Some(scoped.clone())),
                // A represented failure at no greater depth stays a failure:
                // inference budgets only tighten as depth grows.
                super::CachedBinding::Unknown(depth) if self.depth >= *depth => {
                    return Some(None);
                }
                super::CachedBinding::Unknown(_) => {}
            }
        }
        let mut arena = TypeArena::new(self.workspace);
        let value = self.local_binding(&mut arena, binding).map(|value| {
            let value = value?;
            self.scoped_native_type(&mut arena, value.ty, self.file, binding.selection.start)
        });
        if let Some(cache) = self.scoped_bindings
            && cache.borrow().len() < MAX_SCOPED_BINDINGS
        {
            let entry = match &value {
                Some(Some(scoped)) => Some(super::CachedBinding::Type(scoped.clone())),
                // Only a represented failure may be memoized; outer `None`
                // marks an unrepresented type and must keep its fallbacks.
                Some(None) => Some(super::CachedBinding::Unknown(self.depth)),
                None => None,
            };
            if let Some(entry) = entry {
                cache.borrow_mut().insert(key, entry);
            }
        }
        value
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
