//! Documentation belongs to the selected callable object, not its initializer.
use super::super::documentation::{self as doc, Target};
use crate::analysis::query::Context;
use crate::analysis::sema::{MemberDeclId, MethodOrigin, SemanticObject, SymbolOrigin, VarKind};
use crate::syntax::{Expr, ExprId, ExprKind};
use crate::text::SourceMap;
use std::borrow::Cow;

pub(super) fn for_function(
    context: &mut Context<'_>,
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
    let (doc, sema) = context.split();
    let workspace = doc.workspace();
    let file = doc.file();
    let source = doc.source();

    enum DocTarget<'a> {
        UniverseError,
        Local([u32; 2]),
        Global(&'a std::path::Path, &'a crate::index::IndexedSymbol),
        Member(MemberDeclId<'a>),
    }

    let doc_target = {
        let view = sema.region_at(
            file,
            source,
            ident_span[0],
            crate::analysis::sema::RegionMode::from_cursor(cursor_offset),
        )?;
        let obj = view.use_or_selection(ident_span)?;
        match obj {
            SemanticObject::Var {
                origin: SymbolOrigin::Local { span, .. },
                kind,
                ..
            } if matches!(expression.kind, ExprKind::Name) => {
                if matches!(
                    kind,
                    VarKind::Param | VarKind::ResultVar | VarKind::Receiver
                ) || doc::in_function_header(workspace, file, span[0] as usize)
                    || doc::multi_name_short_declaration(source, span[0] as usize)
                    || doc::control_header_statement(source, span[0] as usize)
                {
                    return None;
                }
                Some(DocTarget::Local(span))
            }
            SemanticObject::Func(decl)
            | SemanticObject::Method(MethodOrigin::Concrete(decl))
            | SemanticObject::Var {
                origin: SymbolOrigin::Package(decl),
                ..
            }
            | SemanticObject::Const(SymbolOrigin::Package(decl)) => {
                Some(DocTarget::Global(decl.file, decl.symbol))
            }
            SemanticObject::Method(MethodOrigin::Interface(member)) if member.builtin => {
                Some(DocTarget::UniverseError)
            }
            SemanticObject::Field(member)
            | SemanticObject::Method(MethodOrigin::Interface(member))
                if !member.builtin =>
            {
                Some(DocTarget::Member(member.indexed(workspace)?))
            }
            _ => None,
        }
    }?;

    match doc_target {
        DocTarget::UniverseError => Some("Error returns the error message.".to_owned()),
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
            let member = member.describe(workspace)?;
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
