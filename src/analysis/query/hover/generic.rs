//! Instantiated function hover shares native argument inference and proofs.
use super::{Context, import_qualifier, render};
use std::ops::Range;
use std::path::Path;

const MAX_GENERIC_HEADER: usize = 16 * 1024;

/// gopls' call-site instantiation note: the substituted signature followed by
/// the generic declaration signature as a comment.
pub(super) fn instantiated(
    context: &Context<'_>,
    scope: &crate::analysis::scope::CursorScope,
    span: &Range<usize>,
    path: &Path,
    symbol: &crate::index::IndexedSymbol,
) -> Option<String> {
    if !symbol.generic || !symbol.is_function() {
        return None;
    }
    let initializer;
    let (expressions, base) = if let Some(body) = scope.body.as_ref() {
        (body.tree.expressions.as_slice(), body.base)
    } else {
        let (base, text) = super::value_initializer(context, span.start)?;
        if text.len() > MAX_GENERIC_HEADER {
            return None;
        }
        initializer = crate::syntax::parse_expression(text);
        if !initializer.diagnostics.is_empty() {
            return None;
        }
        (initializer.nodes.as_slice(), base)
    };
    let call = expressions.iter().find_map(|entry| {
        let crate::syntax::ExprKind::Call {
            function,
            arguments,
            variadic,
        } = &entry.kind
        else {
            return None;
        };
        let selected = call_name_span(expressions, *function)?;
        ((base + selected.start..base + selected.end) == *span)
            .then_some((*function, Some((arguments.as_slice(), *variadic))))
    });
    let (function, call) = call.or_else(|| {
        expressions.iter().enumerate().find_map(|(index, entry)| {
            if !matches!(entry.kind, crate::syntax::ExprKind::Index { .. }) {
                return None;
            }
            let id = crate::syntax::ExprId(index);
            let selected = call_name_span(expressions, id)?;
            ((base + selected.start..base + selected.end) == *span).then_some((id, None))
        })
    })?;
    let source_text = render::declaration_source(context, path)?;
    let declaration = symbol.declaration()?;
    let end = symbol.body().map_or(declaration.end, |body| body.start);
    if end.checked_sub(declaration.start)? > MAX_GENERIC_HEADER {
        return None;
    }
    let header = source_text.text().get(declaration.start..end)?;
    let signature = crate::syntax::parse_signature(header);
    let inference = crate::analysis::check::Inference::new(
        context.workspace(),
        context.file(),
        context.source(),
        &scope.locals,
        expressions,
        base,
    );
    let instantiated = inference.instantiated_function_signature(
        (path, header, &signature),
        function,
        call,
        crate::analysis::check::SignaturePresentation::Display,
    )?;
    let code = format!(
        "func {}{}",
        import_qualifier(context, context.file(), path)?,
        render::normalize_signature(&instantiated.label),
    );
    let comment = generic_signature_comment(context, path, header, &signature, &symbol.name)?;
    let separator = if code.len() > 60 { "\n" } else { " " };
    Some(format!("{code}{separator}// {comment}"))
}

/// The selected name keeps its own span through qualification, explicit type
/// arguments and parenthesized callees.
fn call_name_span(
    expressions: &[crate::syntax::Expr],
    mut function: crate::syntax::ExprId,
) -> Option<Range<usize>> {
    for _ in 0..32 {
        let expression = expressions.get(function.0)?;
        match &expression.kind {
            crate::syntax::ExprKind::Name => return Some(expression.span.clone()),
            crate::syntax::ExprKind::Selector { field, .. } => return Some(field.clone()),
            crate::syntax::ExprKind::Index { operand, .. }
            | crate::syntax::ExprKind::Parenthesized {
                expression: operand,
            } => {
                function = *operand;
            }
            _ => return None,
        }
    }
    None
}

/// The generic declaration rendered as gopls' trailing comment.
fn generic_signature_comment(
    context: &Context<'_>,
    origin: &Path,
    header: &str,
    signature: &crate::syntax::Signature,
    name: &str,
) -> Option<String> {
    let mut type_parameters = Vec::new();
    let names = signature
        .type_parameters
        .iter()
        .map(|parameter| header.get(parameter.name.as_ref()?.clone()))
        .collect::<Option<Vec<_>>>()?;
    let mut bytes = 0usize;
    for (index, parameter) in signature.type_parameters.iter().enumerate() {
        let name = header.get(parameter.name.as_ref()?.clone())?.trim();
        let constraint = header.get(parameter.annotation.clone())?.trim();
        let grouped = signature
            .type_parameters
            .get(index + 1)
            .is_some_and(|next| {
                header.get(next.annotation.clone()).map(str::trim) == Some(constraint)
            });
        let constraint = context.workspace().signature_type_scoped(
            context.file(),
            origin,
            constraint,
            &names,
        )?;
        let parameter = if grouped {
            name.to_owned()
        } else {
            format!("{name} {}", render::normalize_type(&constraint))
        };
        bytes = bytes.checked_add(parameter.len() + 2)?;
        if bytes > MAX_GENERIC_HEADER {
            return None;
        }
        type_parameters.push(parameter);
    }
    let (declaration, _) =
        context
            .workspace()
            .declared_signature(context.file(), origin, header, signature, name)?;
    let declaration = render::normalize_signature(&declaration.label);
    let (_, parameters) = declaration.split_once('(')?;
    let comment = format!("func[{}]({parameters}", type_parameters.join(", "));
    (comment.len() <= MAX_GENERIC_HEADER).then_some(comment)
}
