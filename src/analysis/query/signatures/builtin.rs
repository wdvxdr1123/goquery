//! Builtin signatures describe SDK type classes, with native shadow guards.
use super::HelpInformation;
use crate::analysis::check::{Inference, callable_signature::signature_label};
use crate::analysis::query::{Context, builtin, documentation};
use crate::analysis::scope::Locals;
use crate::syntax::{Expr, ExprId, ExprKind, SymbolKind};

pub(super) fn for_function(
    context: &Context<'_>,
    locals: &Locals,
    body: (&[Expr], usize),
    function: ExprId,
) -> Option<HelpInformation> {
    let inference = Inference::new(
        context.workspace(),
        context.file(),
        context.source(),
        locals,
        body.0,
        body.1,
    );
    let name = inference.builtin_name(function)?;
    // A parenthesized builtin has a specialized go/types signature rather than
    // an object template. That projection is not represented here yet.
    if !matches!(body.0.get(function.0)?.kind, ExprKind::Name)
        || inference.constant_builtin_call(function)
    {
        return None;
    }
    let declaration = builtin::declaration(context, name)?;
    if declaration.symbol.kind != SymbolKind::Function {
        return None;
    }
    let range = declaration.symbol.range;
    let header = declaration.source.get(range.clone())?;
    if header.len() > super::super::budget::MAX_BYTES {
        return None;
    }
    let parsed = crate::syntax::parse_signature(header);
    if !parsed.diagnostics.is_empty() {
        return None;
    }
    let information = signature_label(header, &parsed, name, |text| {
        Some(builtin::replace_placeholder_types(text))
    })?;
    let documentation = documentation::attached(
        &declaration.source,
        documentation::Target {
            start: range.start,
            end: range.end,
            declaration: None,
            trailing: false,
        },
    );
    Some(HelpInformation {
        callable: information,
        documentation,
    })
}
