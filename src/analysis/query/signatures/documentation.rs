//! Documentation belongs to the selected callable object, not its initializer.
use super::super::documentation::{self as doc, Target};
use crate::analysis::check::{Inference, Selection};
use crate::analysis::query::Context;
use crate::analysis::scope::{Locals, ObjectKind};
use crate::syntax::{Expr, ExprId, ExprKind};
use crate::text::SourceMap;
use std::borrow::Cow;

pub(super) fn for_function(
    context: &Context<'_>,
    locals: &Locals,
    body: (&[Expr], usize),
    function: ExprId,
) -> Option<String> {
    let (expressions, base) = body;
    let expression = expressions.get(function.0)?;
    // Compound expressions have no go/types object documentation.
    if !matches!(expression.kind, ExprKind::Name | ExprKind::Selector { .. }) {
        return None;
    }
    let workspace = context.workspace();
    let source = context.source();
    if matches!(expression.kind, ExprKind::Name) {
        let span = base + expression.span.start..base + expression.span.end;
        if let Some(binding) = locals.resolve_at(span.start, source.get(span.clone())?) {
            if binding.object_kind == ObjectKind::Parameter
                || doc::in_function_header(context, binding.selection.start)
                || doc::multi_name_short_declaration(source, binding.selection.start)
                || doc::control_header_statement(source, binding.selection.start)
            {
                return None;
            }
            return doc::attached(
                source,
                Target {
                    start: doc::line_start_token(source, binding.selection.start),
                    end: binding.selection.end,
                    declaration: None,
                    trailing: true,
                },
            );
        }
    }
    let inference = Inference::new(workspace, context.file(), source, locals, expressions, base);
    let selection = inference.selection(function)?;
    let origin = match selection {
        Selection::Global(file, _) => file,
        Selection::Member(member) => member.file,
    };
    let source = if origin == context.file() {
        Cow::Borrowed(source)
    } else {
        Cow::Owned(workspace.source(origin)?)
    };
    let target = match selection {
        Selection::Global(_, symbol) => {
            let declaration = symbol.declaration()?;
            Target {
                start: if symbol.is_callable() {
                    declaration.start
                } else {
                    symbol.start
                },
                end: declaration.end,
                declaration: doc::declaration_keyword(&source, symbol.start),
                trailing: !symbol.is_callable(),
            }
        }
        Selection::Member(member) => {
            let map = SourceMap::new(&source);
            let base = member.owner.end;
            let declaration = source.get(base..map.offset(member.owner.range.end)?)?;
            let entries = crate::syntax::aggregate_members(declaration);
            let entry = member.entry(declaration, &entries)?;
            Target {
                start: doc::field_start(declaration, entry.selection.start).map_or_else(
                    || doc::line_start_token(&source, base + entry.selection.start),
                    |start| base + start,
                ),
                end: base + entry.annotation.end,
                declaration: None,
                trailing: true,
            }
        }
    };
    doc::attached(&source, target)
}
