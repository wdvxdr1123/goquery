//! Documentation belongs to the selected callable object, not its initializer.
use super::super::documentation::{self as doc, Target};
use crate::analysis::obj::{MethodOrigin, ObjectRef, SymbolOrigin};
use crate::analysis::query::Context;
use crate::analysis::scope::tree::LocalBindingKind;
use crate::syntax::{Expr, ExprId, ExprKind};
use crate::text::SourceMap;
use std::borrow::Cow;

pub(super) fn for_function(
    context: &Context<'_>,
    body: (&[Expr], usize),
    function: ExprId,
    cursor_offset: Option<usize>,
) -> Option<String> {
    let (expressions, base) = body;
    let expression = expressions.get(function.0)?;
    // Compound expressions have no go/types object documentation.
    let ident_range = match &expression.kind {
        ExprKind::Name => &expression.span,
        ExprKind::Selector { field, .. } => field,
        _ => return None,
    };
    let ident_span = [
        u32::try_from(base + ident_range.start).ok()?,
        u32::try_from(base + ident_range.end).ok()?,
    ];
    let workspace = context.workspace();
    let file = context.file();
    let source = context.source();

    enum DocTarget<'a> {
        Local([u32; 2]),
        Global(&'a std::path::Path, &'a crate::index::IndexedSymbol),
        Member(crate::analysis::obj::MemberDeclaration<'a>),
    }

    let doc_target = context.with_sema(|sema| {
        let (region, _) = sema.region_and_arena_at(file, source, ident_span[0], cursor_offset)?;
        let obj = region
            .use_by_span(ident_span)
            .or_else(|| region.selection_by_span(ident_span).map(|sel| sel.obj))?;
        match obj {
            ObjectRef::Var {
                origin: SymbolOrigin::Local { id, span, .. },
                ..
            } if matches!(expression.kind, ExprKind::Name) => {
                let binding = region.scopes.bindings.get(id.0 as usize)?;
                if matches!(
                    binding.kind,
                    LocalBindingKind::Param
                        | LocalBindingKind::NamedResult
                        | LocalBindingKind::Receiver
                ) || doc::in_function_header(context, span[0] as usize)
                    || doc::multi_name_short_declaration(source, span[0] as usize)
                    || doc::control_header_statement(source, span[0] as usize)
                {
                    return None;
                }
                Some(DocTarget::Local(span))
            }
            ObjectRef::Func(decl)
            | ObjectRef::Method(MethodOrigin::Concrete(decl))
            | ObjectRef::Var {
                origin: SymbolOrigin::Package(decl),
                ..
            }
            | ObjectRef::Const(SymbolOrigin::Package(decl)) => {
                Some(DocTarget::Global(decl.file, decl.symbol))
            }
            ObjectRef::Field(member) | ObjectRef::Method(MethodOrigin::Interface(member))
                if !member.builtin =>
            {
                Some(DocTarget::Member(member.to_member_declaration(workspace)?))
            }
            _ => None,
        }
    })?;

    match doc_target {
        DocTarget::Local(span) => doc::attached(
            source,
            Target {
                start: doc::line_start_token(source, span[0] as usize),
                end: span[1] as usize,
                declaration: None,
                trailing: true,
            },
        ),
        DocTarget::Global(origin, symbol) => {
            let origin_source = if origin == file {
                Cow::Borrowed(source)
            } else {
                Cow::Owned(workspace.source(origin)?)
            };
            let declaration = symbol.declaration()?;
            doc::attached(
                &origin_source,
                Target {
                    start: if symbol.is_callable() {
                        declaration.start
                    } else {
                        symbol.start
                    },
                    end: declaration.end,
                    declaration: doc::declaration_keyword(&origin_source, symbol.start),
                    trailing: !symbol.is_callable(),
                },
            )
        }
        DocTarget::Member(member) => {
            let origin_source = if member.file == file {
                Cow::Borrowed(source)
            } else {
                Cow::Owned(workspace.source(member.file)?)
            };
            let map = SourceMap::new(&origin_source);
            let owner_base = member.owner.end;
            let declaration = origin_source.get(owner_base..map.offset(member.owner.range.end)?)?;
            let entries = crate::syntax::aggregate_members(declaration);
            let entry = member.entry(declaration, &entries)?;
            doc::attached(
                &origin_source,
                Target {
                    start: doc::field_start(declaration, entry.selection.start).map_or_else(
                        || {
                            doc::line_start_token(
                                &origin_source,
                                owner_base + entry.selection.start,
                            )
                        },
                        |start| owner_base + start,
                    ),
                    end: owner_base + entry.annotation.end,
                    declaration: None,
                    trailing: true,
                },
            )
        }
    }
}
