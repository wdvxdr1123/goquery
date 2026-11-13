//! Type-definition queries consume shared scopes and native declaration identities.
use super::Inference;
use crate::analysis::query;
use crate::analysis::scope::{Binding, DeclarationKind, Locals};
use crate::analysis::types::{LocalTypeName, TypeArena, TypeDeclaration};
use crate::syntax::{ExprId, ExprKind, Lexer, TokenKind, TypeReference};
use crate::workspace::Workspace;
use std::ops::Range;
use std::path::Path;

type Targets<'a> = Option<Result<Vec<TypeDeclaration<'a>>, (i32, String)>>;
fn projected<'a>(arena: &mut TypeArena<'a>, ty: crate::analysis::types::TypeId) -> Targets<'a> {
    let names = arena.type_declarations(ty)?;
    Some(if names.is_empty() {
        if let Some(display) = arena.diagnostic_type(ty) {
            Err((0, format!("cannot find type name(s) from type {display}")))
        } else {
            Ok(names)
        }
    } else {
        Ok(names)
    })
}

pub(crate) fn declarations<'a>(
    workspace: &'a Workspace,
    file: &Path,
    source: &str,
    offset: usize,
    arena: &mut TypeArena<'a>,
) -> Result<Vec<TypeDeclaration<'a>>, (i32, String)> {
    let file = workspace
        .files
        .get_key_value(file)
        .expect("admitted document")
        .0
        .as_path();
    let mut cursor = query::cursor_scope(workspace, file, source, offset)?;
    let mut previous = TokenKind::Illegal;
    let mut selected = None;
    for token in Lexer::new(source).take_while(|token| token.span.start <= offset) {
        if token.kind == TokenKind::Ident && token.span.start <= offset && offset <= token.span.end
        {
            selected = Some((token.span.clone(), previous == TokenKind::Period));
            break;
        }
        previous = token.kind;
    }
    // Header parameters have scope metadata even though the body is not active.
    if cursor.body.is_none() {
        for symbol in &workspace.files[file].symbols {
            let Some(declaration) = symbol.declaration() else {
                continue;
            };
            let Some(body) = symbol.body() else {
                continue;
            };
            if declaration.start <= offset && offset <= body.start {
                let header = source
                    .get(declaration.start..body.start)
                    .ok_or((crate::error::INVALID_PARAMS, "stale function header".into()))?;
                let parsed = crate::syntax::parse_signature(header);
                cursor.locals =
                    Locals::from_header(source, declaration.start, body.clone(), &parsed);
                let inference = Inference::new(workspace, file, source, &cursor.locals, &[], 0);
                if let Some((selection, _)) = &selected
                    && selection.start == symbol.start
                    && selection.end == symbol.end
                {
                    return arena
                        .declaration_signature(file, header, &parsed)
                        .and_then(|ty| projected(arena, ty))
                        .unwrap_or_else(|| Ok(Vec::new()));
                }
                if let Some((selection, member)) = &selected {
                    if !member
                        && let Some(binding) = cursor
                            .locals
                            .definition_at(selection, &source[selection.clone()])
                    {
                        return inference
                            .binding_declarations(arena, binding)
                            .unwrap_or_else(|| Ok(Vec::new()));
                    }
                    if let Some(found) = reference_declarations(
                        &inference,
                        arena,
                        &parsed.type_references,
                        declaration.start,
                        selection,
                    ) {
                        return found;
                    }
                }
                return Ok(Vec::new());
            }
        }
    }
    let expressions = cursor
        .body
        .as_ref()
        .map_or(&[][..], |body| body.tree.expressions.as_slice());
    let base = cursor.body.as_ref().map_or(0, |body| body.base);
    let inference = Inference::new(workspace, file, source, &cursor.locals, expressions, base);
    if let Some((selection, member)) = &selected {
        if !member {
            if let Some(binding) = cursor
                .locals
                .definition_at(selection, &source[selection.clone()])
            {
                if *selection == binding.selection && cursor.locals.switch_case(binding).is_some() {
                    return inference
                        .switch_input(arena, binding)
                        .and_then(|input| projected(arena, input.ty))
                        .unwrap_or_else(|| Ok(Vec::new()));
                }
                return inference
                    .binding_declarations(arena, binding)
                    .unwrap_or_else(|| Ok(Vec::new()));
            }
            if let Some(found) =
                inference.spelling_declarations(arena, &source[selection.clone()], selection.start)
            {
                return found;
            }
        }
        if let Some(body) = &cursor.body
            && let Some(found) = reference_declarations(
                &inference,
                arena,
                &body.tree.type_references,
                base,
                selection,
            )
        {
            return found;
        }
    }
    if cursor.body.is_some() {
        return expression_declarations(&inference, arena, offset)
            .unwrap_or_else(|| Ok(Vec::new()));
    }
    // Package declarations retain only compact spans. Parse just the selected
    // initializer/header, never a package AST or a retained closed source.
    for symbol in &workspace.files[file].symbols {
        let Some(span) = symbol.declaration() else {
            continue;
        };
        if !(span.start <= offset && offset <= span.end) {
            continue;
        }
        if let Some((selection, _)) = &selected
            && symbol.start == selection.start
            && symbol.end == selection.end
        {
            let text = source.get(selection.clone()).unwrap_or("");
            let parsed = crate::syntax::parse_expression(text);
            let inference = Inference::new(
                workspace,
                file,
                source,
                &cursor.locals,
                &parsed.nodes,
                selection.start,
            );
            return inference
                .expression_declarations(arena, parsed.root)
                .unwrap_or_else(|| Ok(Vec::new()));
        }
        if symbol.is_value() && span.len() <= 16 * 1024 {
            let text = format!("{{var {}\n}}", &source[span.clone()]);
            let parsed = crate::syntax::parse_block(&text);
            let Some(base) = span.start.checked_sub(5) else {
                continue;
            };
            let inference = Inference::new(
                workspace,
                file,
                source,
                &cursor.locals,
                &parsed.expressions,
                base,
            );
            if let Some((selection, _)) = &selected
                && let Some(found) = reference_declarations(
                    &inference,
                    arena,
                    &parsed.type_references,
                    base,
                    selection,
                )
            {
                return found;
            }
            return expression_declarations(&inference, arena, offset)
                .unwrap_or_else(|| Ok(Vec::new()));
        }
        if symbol.is_type()
            && let Some((selection, _)) = &selected
        {
            let text = source.get(symbol.end..span.end).unwrap_or("");
            if let Some(body) = crate::analysis::types::declaration_body(text) {
                let base = span.end - body.len();
                let parsed = crate::syntax::parse_type(body);
                return reference_declarations(
                    &inference,
                    arena,
                    &parsed.references,
                    base,
                    selection,
                )
                .unwrap_or_else(|| Ok(Vec::new()));
            }
        }
    }
    Ok(Vec::new())
}

fn reference_declarations<'a>(
    inference: &Inference<'a, '_>,
    arena: &mut TypeArena<'a>,
    references: &[TypeReference],
    base: usize,
    selection: &Range<usize>,
) -> Targets<'a> {
    let reference = references.iter().find(|reference| {
        base + reference.name.start == selection.start && base + reference.name.end == selection.end
    })?;
    let start = base
        + reference
            .qualifier
            .as_ref()
            .unwrap_or(&reference.name)
            .start;
    inference.spelling_declarations(arena, inference.source.get(start..selection.end)?, start)
}
fn expression_declarations<'a>(
    inference: &Inference<'a, '_>,
    arena: &mut TypeArena<'a>,
    offset: usize,
) -> Targets<'a> {
    let mut candidates = inference
        .expressions
        .iter()
        .enumerate()
        .filter(|(_, expr)| {
            let span = &expr.span;
            inference.base + span.start <= offset && offset <= inference.base + span.end
        })
        .take(257)
        .collect::<Vec<_>>();
    if candidates.len() > 256 {
        return None;
    }
    candidates.sort_by_key(|(_, expr)| expr.span.len());
    for (index, _) in candidates {
        if let Some(found) = inference.expression_declarations(arena, ExprId(index)) {
            return Some(found);
        }
    }
    None
}
impl<'a> Inference<'a, '_> {
    fn spelling_declarations(
        &self,
        arena: &mut TypeArena<'a>,
        text: &str,
        offset: usize,
    ) -> Targets<'a> {
        if text.len() > 16 * 1024 {
            return None;
        }
        let parsed = crate::syntax::parse_type(text);
        if !parsed.diagnostics.is_empty() {
            return None;
        }
        let mut names = Vec::new();
        for reference in &parsed.references {
            let span = reference.qualifier.as_ref().unwrap_or(&reference.name);
            let name = &text[span.clone()];
            if let Some(binding) = self.locals.resolve_at(offset, name) {
                if !binding.object_kind.is_type() {
                    return Some(Ok(Vec::new()));
                }
                if names
                    .iter()
                    .any(|local: &LocalTypeName<'_>| local.name == name)
                {
                    continue;
                }
                names.push(LocalTypeName {
                    name,
                    selection: binding.selection.clone(),
                    formal: binding.declaration == DeclarationKind::TypeParameter,
                });
            }
        }
        let ty = arena.declaration_type(self.file, text, &names)?;
        projected(arena, ty)
    }
    fn binding_declarations(&self, arena: &mut TypeArena<'a>, binding: &Binding) -> Targets<'a> {
        if self.depth >= 32 {
            return None;
        }
        if self.locals.switch_case(binding).is_some() {
            let value = self.switch_operand(arena, binding)?;
            return projected(arena, value.ty);
        }
        let native = self.local_binding(arena, binding);
        if let Some(Some(value)) = native {
            return projected(arena, value.ty);
        }
        if matches!(native, Some(None)) && binding.annotation().is_some() {
            return None;
        }
        if binding.declaration == DeclarationKind::TypeParameter {
            return self.spelling_declarations(
                arena,
                self.source.get(binding.selection.clone())?,
                binding.selection.start,
            );
        }
        if binding.object_kind.is_type() {
            return Some(Ok(if binding.declaration == DeclarationKind::Type {
                vec![TypeDeclaration::Local(
                    self.workspace.files.get_key_value(self.file)?.0,
                    binding.selection.clone(),
                )]
            } else {
                Vec::new()
            }));
        }
        if let Some(span) = binding.annotation() {
            return self.spelling_declarations(arena, self.source.get(span.clone())?, span.start);
        }
        if let Some((span, result, count)) = binding.initializer() {
            let text = self
                .source
                .get(span.clone())
                .filter(|text| text.len() <= 16 * 1024)?;
            let parsed = crate::syntax::parse_expression(text);
            if !parsed.diagnostics.is_empty() {
                return None;
            }
            // Opaque local type declarations still support syntactic navigation
            // from constructors/signatures, without proving value legality.
            if matches!(native, Some(None))
                && !matches!(
                    parsed.nodes[parsed.root.0].kind,
                    ExprKind::Composite { .. } | ExprKind::FunctionLiteral { .. }
                )
            {
                return None;
            }
            let inference = Inference {
                expressions: &parsed.nodes,
                base: span.start,
                depth: self.depth + 1,
                ..*self
            };
            if result == 0 && count == 1 {
                return inference.expression_declarations(arena, parsed.root);
            }
            if matches!(
                parsed.nodes[parsed.root.0].kind,
                ExprKind::TypeAssertion { .. }
            ) {
                let value = inference.assertion_operand(arena, parsed.root, result, count)?;
                return projected(arena, value.ty);
            }
            if let ExprKind::Unary { operator, .. } = &inference.unparenthesized(parsed.root).kind
                && inference.text(operator) == "<-"
            {
                let value = inference.receive_operand(arena, parsed.root, result, count)?;
                return projected(arena, value.ty);
            }
            if let ExprKind::Call {
                function,
                arguments,
                variadic,
            } = &parsed.nodes[parsed.root.0].kind
            {
                let scoped =
                    inference.call_result(*function, arguments, *variadic, result, count)?;
                let ty = arena.parse(scoped.scope, &scoped.text)?;
                return projected(arena, ty);
            }
            return None;
        }
        let scoped = self.scoped_range_binding(binding)?;
        let ty = arena.parse(scoped.scope, &scoped.text)?;
        projected(arena, ty)
    }
    fn expression_declarations(&self, arena: &mut TypeArena<'a>, id: ExprId) -> Targets<'a> {
        if self.depth >= 32 {
            return None;
        }
        let expression = self.unparenthesized(id);
        if let ExprKind::Name = expression.kind
            && let Some(binding) = self.locals.resolve_at(
                self.base + expression.span.start,
                self.text(&expression.span),
            )
        {
            return self.binding_declarations(arena, binding);
        }
        if let ExprKind::Composite {
            literal_type: Some(span),
            ..
        } = &expression.kind
        {
            let span = &self.expressions[span.0].span;
            return self.spelling_declarations(arena, self.text(span), self.base + span.start);
        }
        if let ExprKind::FunctionLiteral { signature, .. } = &expression.kind {
            return self.spelling_declarations(
                arena,
                self.text(signature),
                self.base + signature.start,
            );
        }
        if let ExprKind::Selector { operand, field } = &expression.kind
            && let Some(input) = self.value_operand(arena, *operand)
            && let Some(value) = arena.field_operand(input, self.file, self.text(field))
        {
            return projected(arena, value.ty);
        }
        let ty = self
            .conversion_target(arena, id)
            .or_else(|| self.value_operand(arena, id).map(|operand| operand.ty));
        if let Some(ty) = ty {
            return projected(arena, ty);
        }
        if let ExprKind::Call {
            function,
            arguments,
            variadic,
        } = &expression.kind
        {
            let count = self.call_result_count(arena, *function)?;
            if count > 256 {
                return None;
            }
            let mut result = Vec::new();
            for slot in 0..count {
                let ty = if let Some(value) =
                    self.native_call(arena, *function, arguments, *variadic, slot, count)
                {
                    value?.ty
                } else {
                    let scoped = self.call_result(*function, arguments, *variadic, slot, count)?;
                    arena.parse(scoped.scope, &scoped.text)?
                };
                result.extend(arena.type_declarations(ty)?);
                if result.len() > 256 {
                    return None;
                }
            }
            return Some(Ok(result));
        }
        // Generic/method declaration schemas carry opaque header formals only;
        // they are not used to prove a value call or constraint satisfaction.
        let (file, symbol) = self.callee(id)?;
        if !symbol.is_callable() {
            return None;
        }
        let declaration = symbol.declaration()?;
        let end = symbol.body().map_or(declaration.end, |body| body.start);
        let header = self.workspace.source_slice(file, declaration.start..end)?;
        let parsed = crate::syntax::parse_signature(&header);
        let ty = arena.declaration_signature(file, &header, &parsed)?;
        projected(arena, ty)
    }
}
