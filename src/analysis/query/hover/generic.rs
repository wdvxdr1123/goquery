//! Instantiated function hover shares native argument inference and proofs.
use super::{Context, import_qualifier, render};
use std::ops::Range;
use std::path::Path;

const MAX_GENERIC_HEADER: usize = 16 * 1024;

/// gopls' call-site instantiation note: the substituted signature followed by
/// the generic declaration signature as a comment.
pub(super) fn instantiated(
    context: &mut Context<'_>,
    span: &Range<usize>,
    path: &Path,
    symbol: &crate::index::IndexedSymbol,
) -> Option<String> {
    if !symbol.generic || !symbol.is_function() {
        return None;
    }
    let decl_file = context
        .workspace()
        .files
        .get_key_value(path)
        .map(|(p, _)| p.as_path())?;
    let s0 = u32::try_from(span.start).ok()?;
    let s1 = u32::try_from(span.end).ok()?;
    let checked = {
        let (doc, sema) = context.split();
        if let Some(mut view) = sema.region_at(
            doc.file(),
            doc.source(),
            s0,
            crate::analysis::sema::RegionMode::Full,
        ) && let Some(inst) = view.instance_by_span([s0, s1])
            && matches!(
                inst.target,
                crate::analysis::sema::SemanticObject::Func(d)
                    if d.file == decl_file && std::ptr::eq(d.symbol, symbol)
            )
        {
            crate::analysis::query::signatures::instantiated::format_instantiated_signature(
                doc.file(),
                &mut view,
                &symbol.name,
                inst.instantiated,
            )
        } else {
            None
        }
    };
    let instantiated = match checked {
        Some(res) => Some(res),
        None => {
            let (base, text) = super::value_initializer(context, span.start)?;
            if text.len() > MAX_GENERIC_HEADER {
                return None;
            }
            let r0 = u32::try_from(span.start.checked_sub(base)?).ok()?;
            let r1 = u32::try_from(span.end.checked_sub(base)?).ok()?;
            let text = text.to_owned();
            let file = context.file();
            context.with_sema(|sema| sema.with_expression(file, &text, |view, _| {
                let inst = view.instance_by_span([r0, r1])?;
                if !matches!(inst.target, crate::analysis::sema::SemanticObject::Func(d) if d.file == decl_file && std::ptr::eq(d.symbol, symbol)) {
                    return None;
                }
                crate::analysis::query::signatures::instantiated::format_instantiated_signature(
                    file, view, &symbol.name, inst.instantiated,
                )
            }))
        }
    }?;
    let source_text = render::declaration_source(context, path)?;
    let declaration = symbol.declaration()?;
    let end = symbol.body().map_or(declaration.end, |body| body.start);
    if end.checked_sub(declaration.start)? > MAX_GENERIC_HEADER {
        return None;
    }
    let header = source_text.text().get(declaration.start..end)?;
    let signature = crate::syntax::parse_signature(header);
    let code = format!(
        "func {}{}",
        import_qualifier(context, context.file(), path)?,
        render::normalize_signature(&instantiated.0.label),
    );
    let comment = generic_signature_comment(context, path, header, &signature, &symbol.name)?;
    let separator = if code.len() > 60 { "\n" } else { " " };
    Some(format!("{code}{separator}// {comment}"))
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
        let constraint = crate::analysis::query::signatures::label::signature_type_scoped(
            context.workspace(),
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
    let (declaration, _) = crate::analysis::query::signatures::label::declared_signature(
        context.workspace(),
        context.file(),
        origin,
        header,
        signature,
        name,
    )?;
    let declaration = render::normalize_signature(&declaration.label);
    let (_, parameters) = declaration.split_once('(')?;
    let comment = format!("func[{}]({parameters}", type_parameters.join(", "));
    (comment.len() <= MAX_GENERIC_HEADER).then_some(comment)
}
