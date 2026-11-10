//! Bounded local formal facts and structural projections at declaration scope.
use super::Inference;
use crate::analysis::scope::{Binding, DeclarationKind};
use crate::analysis::types::{LocalFormal, LocalTypeDefinition, Operand, TypeArena, TypeId};
use crate::syntax::{ExprId, ExprKind};

/// Written annotation syntax resolved in one lexical scope. Phase-2 local type
/// facts stay separately distinguished so navigation fallbacks are not lost.
pub(super) enum AnnotationType {
    Resolved(TypeId),
    /// A local type/generic fact this increment does not model; never guess one.
    UnsupportedLocal,
    /// Other unrepresented syntax, shadow or budget failure.
    Unknown,
}

impl Inference<'_, '_> {
    pub(super) fn parameterized_type(
        &self,
        arena: &mut TypeArena<'_>,
        annotation: &str,
        annotation_offset: usize,
    ) -> Option<TypeId> {
        if annotation.len() > 16 * 1024 {
            return None;
        }
        let parsed = crate::syntax::parse_type(annotation);
        if !parsed.diagnostics.is_empty() {
            return None;
        }
        let mut parameters: Vec<&Binding> = Vec::new();
        for reference in &parsed.references {
            let span = reference.qualifier.as_ref().unwrap_or(&reference.name);
            let name = &annotation[span.clone()];
            if let Some(local) = self.locals.resolve_at(annotation_offset, name) {
                if local.declaration != DeclarationKind::TypeParameter {
                    return None;
                }
                if !parameters
                    .iter()
                    .any(|parameter| parameter.selection == local.selection)
                {
                    parameters.push(local);
                }
            }
        }
        if parameters.is_empty() {
            return None;
        }
        let mut formals = Vec::new();
        let mut receiver = None;
        let mut index = 0;
        let mut bytes = 0usize;
        while index < parameters.len() {
            if parameters.len() > 32 {
                return None;
            }
            let local = parameters[index];
            let name = local.name.as_str();
            let bound = if let Some(span) = self.locals.type_parameter_constraint(local) {
                let text = self.source.get(span.clone())?;
                bytes = bytes.checked_add(text.len())?;
                if bytes > 16 * 1024 {
                    return None;
                }
                let wrapped = format!("interface{{{text}}}");
                let parsed = crate::syntax::parse_type(&wrapped);
                if !parsed.diagnostics.is_empty() {
                    return None;
                }
                for reference in &parsed.references {
                    let reference = reference.qualifier.as_ref().unwrap_or(&reference.name);
                    let name = &wrapped[reference.clone()];
                    if let Some(local) = self.locals.resolve_at(span.start, name) {
                        if local.declaration != DeclarationKind::TypeParameter {
                            return None;
                        }
                        if !parameters
                            .iter()
                            .any(|parameter| parameter.selection == local.selection)
                        {
                            parameters.push(local);
                        }
                    }
                }
                Some(text)
            } else {
                if let Some(span) = self.locals.type_parameter_receiver(local) {
                    if receiver.is_some_and(|previous| previous != span) {
                        return None;
                    }
                    receiver = Some(span);
                    for selection in
                        crate::analysis::scope::signature::receiver_parameters(self.source, span)
                    {
                        let name = self.source.get(selection)?;
                        let local = self.locals.resolve_at(span.start, name)?;
                        if local.declaration != DeclarationKind::TypeParameter {
                            return None;
                        }
                        if !parameters
                            .iter()
                            .any(|parameter| parameter.selection == local.selection)
                        {
                            parameters.push(local);
                        }
                    }
                }
                None
            };
            formals.push(LocalFormal {
                name,
                selection: local.selection.start,
                bound,
            });
            index += 1;
        }
        let receiver = match receiver {
            Some(span) => Some(self.source.get(span.clone())?),
            None => None,
        };
        arena.parse_parameterized(self.file, annotation, &formals, receiver)
    }

    /// Resolve written type syntax at its lexical position: local formals, local
    /// defined types/aliases and package declarations share one bounded scope.
    /// No source or tree is retained; the arena owns only the definition facts.
    pub(super) fn annotation_type(
        &self,
        arena: &mut TypeArena<'_>,
        annotation: &str,
        annotation_offset: usize,
    ) -> AnnotationType {
        if annotation.len() > 16 * 1024 || self.depth >= 32 {
            return AnnotationType::Unknown;
        }
        let parsed = crate::syntax::parse_type(annotation);
        if !parsed.diagnostics.is_empty() {
            return AnnotationType::Unknown;
        }
        let mut parameterized = false;
        let mut pending: Vec<&Binding> = Vec::new();
        for reference in &parsed.references {
            let span = reference.qualifier.as_ref().unwrap_or(&reference.name);
            let name = &annotation[span.clone()];
            if let Some(binding) = self.locals.resolve_at(annotation_offset + span.start, name) {
                if binding.declaration == DeclarationKind::TypeParameter {
                    parameterized = true;
                    continue;
                }
                if !binding.object_kind.is_type() {
                    return AnnotationType::Unknown;
                }
                if !pending
                    .iter()
                    .any(|previous| previous.selection == binding.selection)
                {
                    pending.push(binding);
                }
                continue;
            }
            // Never resolve a shadowed/ambiguous unqualified name as a package type.
            if reference.qualifier.is_none()
                && self
                    .workspace
                    .unqualified_symbol(self.file, name)
                    .is_some_and(|(_, symbol)| !symbol.is_type())
            {
                return AnnotationType::Unknown;
            }
        }
        if pending.is_empty() {
            if parameterized {
                return match self.parameterized_type(arena, annotation, annotation_offset) {
                    Some(ty) => AnnotationType::Resolved(ty),
                    None => AnnotationType::Unknown,
                };
            }
            return match arena.parse(self.file, annotation) {
                Some(ty) => AnnotationType::Resolved(ty),
                None => AnnotationType::Unknown,
            };
        }
        if parameterized {
            // A local type whose RHS mixes an enclosing formal with local types
            // needs the phase-2 environment; do not approximate it.
            return AnnotationType::UnsupportedLocal;
        }
        let Some(definitions) = self.local_type_definitions(&mut pending) else {
            return AnnotationType::UnsupportedLocal;
        };
        match arena.declared_scope_type(self.file, annotation, &definitions) {
            Some(ty) => AnnotationType::Resolved(ty),
            None => AnnotationType::UnsupportedLocal,
        }
    }

    /// Transitive closure of locally declared types referenced by one annotation.
    /// Each fact keeps only name/selection/RHS/alias; 64 entries and 16 KiB bound it.
    fn local_type_definitions<'b>(
        &'b self,
        pending: &mut Vec<&'b Binding>,
    ) -> Option<Vec<LocalTypeDefinition<'b>>> {
        let mut definitions: Vec<LocalTypeDefinition<'b>> = Vec::new();
        let mut bytes = 0usize;
        let mut index = 0;
        while index < pending.len() {
            let binding = pending[index];
            index += 1;
            if definitions
                .iter()
                .any(|definition| definition.selection == binding.selection)
            {
                continue;
            }
            if definitions.len() >= 64 {
                return None;
            }
            let annotation_span = binding.annotation()?;
            let annotation = self.source.get(annotation_span.clone())?;
            let name = self.source.get(binding.selection.clone())?;
            if annotation.is_empty() || annotation.len() > 16 * 1024 {
                return None;
            }
            bytes = bytes
                .checked_add(name.len())?
                .checked_add(annotation.len())?;
            if bytes > 16 * 1024 {
                return None;
            }
            // Only `type A = B` in the gap is a transparent alias; a type parameter
            // list makes the local type generic and is phase 2.
            let gap = self
                .source
                .get(binding.selection.end..annotation_span.start)?;
            let mut alias = false;
            for token in crate::syntax::Lexer::new(gap) {
                match token.kind {
                    crate::syntax::TokenKind::Assign => alias = true,
                    _ => return None,
                }
            }
            let parsed = crate::syntax::parse_type(annotation);
            if !parsed.diagnostics.is_empty() {
                return None;
            }
            for reference in &parsed.references {
                let span = reference.qualifier.as_ref().unwrap_or(&reference.name);
                let name = &annotation[span.clone()];
                if let Some(local) = self
                    .locals
                    .resolve_at(binding.selection.start + span.start, name)
                {
                    if local.declaration == DeclarationKind::TypeParameter {
                        return None;
                    }
                    if !local.object_kind.is_type() {
                        return None;
                    }
                    if !pending
                        .iter()
                        .any(|previous| previous.selection == local.selection)
                    {
                        pending.push(local);
                    }
                    continue;
                }
                if reference.qualifier.is_none()
                    && self
                        .workspace
                        .unqualified_symbol(self.file, name)
                        .is_some_and(|(_, symbol)| !symbol.is_type())
                {
                    return None;
                }
            }
            definitions.push(LocalTypeDefinition {
                name,
                selection: binding.selection.clone(),
                annotation,
                alias,
            });
        }
        Some(definitions)
    }

    /// A variadic parameter's Go type is the slice of its written element type.
    /// The enclosing declaration header is read as one bounded slice, without a
    /// retained tree; receiver/parameter spans stay absolute declaration offsets.
    pub(super) fn variadic_type(
        &self,
        arena: &mut TypeArena<'_>,
        binding: &Binding,
    ) -> Option<TypeId> {
        if self.depth >= 32 {
            return None;
        }
        let stub = self.workspace.files.get(self.file)?;
        for symbol in &stub.symbols {
            let (Some(declaration), Some(body)) = (symbol.declaration(), symbol.body.as_ref())
            else {
                continue;
            };
            if declaration.start > binding.selection.start
                || binding.selection.start > body.start
                || body.start.saturating_sub(declaration.start) > 16 * 1024
            {
                continue;
            }
            let header = self.source.get(declaration.start..body.start)?;
            let signature = crate::syntax::parse_signature(header);
            for parameter in signature.receiver.iter().chain(&signature.parameters) {
                let Some(name) = parameter.name.as_ref().filter(|_| parameter.variadic) else {
                    continue;
                };
                let selection = declaration.start + name.start..declaration.start + name.end;
                if selection != binding.selection {
                    continue;
                }
                let annotation = declaration.start + parameter.annotation.start
                    ..declaration.start + parameter.annotation.end;
                let text = self.source.get(annotation.clone())?;
                if text.is_empty() || text.len() > 16 * 1024 {
                    return None;
                }
                let wrapped = format!("[]{text}");
                return match self.annotation_type(arena, &wrapped, annotation.start) {
                    AnnotationType::Resolved(ty) => Some(ty),
                    AnnotationType::UnsupportedLocal | AnnotationType::Unknown => None,
                };
            }
            return None;
        }
        None
    }
    pub(super) fn parameterized_operand(
        &self,
        arena: &mut TypeArena<'_>,
        id: ExprId,
    ) -> Option<Operand> {
        if self.depth >= 32 {
            return None;
        }
        let expression = self.unparenthesized(id);
        match &expression.kind {
            ExprKind::Unary { operator, .. } if self.text(operator) == "<-" => {
                self.receive_operand(arena, id, 0, 1)
            }
            ExprKind::Name => self.parameterized_name_operand(arena, id),
            ExprKind::Call {
                function,
                arguments,
                variadic,
            } => {
                let name = self.builtin_name(*function)?;
                self.builtin_operand(arena, name, arguments, *variadic)
            }
            ExprKind::Selector { operand, field } => {
                let input = self.next().parameterized_operand(arena, *operand)?;
                arena.field_operand(input, self.file, self.text(field))
            }
            ExprKind::Unary { operator, operand } if self.text(operator) == "*" => {
                let input = self.next().parameterized_operand(arena, *operand)?;
                arena.indirect(input)
            }
            ExprKind::Index { operand, arguments } if arguments.len() == 1 => {
                let input = self.next().parameterized_operand(arena, *operand)?;
                let index = self
                    .next()
                    .parameterized_operand(arena, arguments[0])
                    .or_else(|| self.next().value_operand(arena, arguments[0]))?;
                arena.index(input, index, 0, 1, self.workspace.integer_bits()?)
            }
            ExprKind::Slice {
                operand,
                low,
                high,
                max,
                full,
            } => {
                let input = self.next().parameterized_operand(arena, *operand)?;
                let mut indices = [None; 3];
                for (slot, id) in indices.iter_mut().zip([low, high, max]) {
                    if let Some(id) = id {
                        *slot = Some(
                            self.next()
                                .parameterized_operand(arena, *id)
                                .or_else(|| self.next().value_operand(arena, *id))?,
                        );
                    }
                }
                arena.slice(input, indices, *full, self.workspace.integer_bits()?)
            }
            _ => None,
        }
    }

    fn parameterized_name_operand(&self, arena: &mut TypeArena<'_>, id: ExprId) -> Option<Operand> {
        let expression = self.unparenthesized(id);
        if !matches!(expression.kind, ExprKind::Name) {
            return None;
        }
        let offset = self.base + expression.span.start;
        let binding = self
            .locals
            .resolve_at(offset, self.text(&expression.span))?;
        self.local_binding(arena, binding)?
    }
}
