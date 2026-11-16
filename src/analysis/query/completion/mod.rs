//! Completion semantic facts remain independent of snippets and wire items.
mod presentation;
mod signatures;
mod type_position;
use super::Context;
pub(crate) use crate::analysis::obj::{MemberDeclaration, ReceiverMember};
pub(crate) use crate::analysis::query::signatures::label::{CallableSignature, ParameterLabel};
use crate::analysis::scope::CursorScope;
pub(crate) use crate::analysis::scope::{Binding, Locals};
use crate::syntax::{Expr, ExprId, ExprKind, Lexer, StmtKind, TokenKind};
use crate::text::SourceMap;
pub(crate) use signatures::{PreparedMethods, SignatureSource};
pub(in crate::analysis::query) use type_position::type_position;

#[derive(Clone)]
pub(crate) struct CompletionReceiver<'a> {
    pub(crate) scope: &'a std::path::Path,
    pub(crate) text: String,
}

pub(crate) enum Kind {
    Struct,
    Interface,
    TypeName,
}
pub(crate) struct Presentation {
    pub(crate) detail: Option<String>,
    pub(crate) signature: Option<(CallableSignature, bool)>,
    pub(crate) type_parameters: Option<Vec<String>>,
    pub(crate) kind: Option<Kind>,
}

impl<'a> Context<'a> {
    pub(crate) fn trace_queries(&self) -> bool {
        self.workspace.trace.queries
    }

    pub(crate) fn imported_symbols(
        &self,
        alias: &str,
    ) -> impl Iterator<Item = (&'a std::path::PathBuf, &'a crate::index::IndexedSymbol)> + use<'a>
    {
        crate::analysis::resolve::imported_symbol_iter(self.workspace, self.file(), alias)
    }

    pub(crate) fn file_symbols_in_scope(
        &self,
    ) -> impl Iterator<Item = (&'a std::path::PathBuf, &'a crate::index::IndexedSymbol)> + use<'a>
    {
        crate::analysis::resolve::file_symbol_iter(self.workspace, self.file())
    }

    pub(crate) fn function_value_context(&self, offset: usize, scope: &CursorScope) -> bool {
        let file = self.file();
        let source = self.source();
        let Some(body) = &scope.body else {
            let map = SourceMap::new(source);
            return self.workspace.files[file]
                .symbols
                .iter()
                .filter(|symbol| symbol.is_var())
                .any(|symbol| {
                    let Some(end) = map.offset(symbol.range.end) else {
                        return false;
                    };
                    if !(symbol.end..=end).contains(&offset) {
                        return false;
                    }
                    let Some(assignment) = Lexer::new(&source[symbol.end..end])
                        .find(|token| token.kind == TokenKind::Assign)
                    else {
                        return false;
                    };
                    let assignment = symbol.end + assignment.span.start;
                    offset > assignment
                        && self
                            .with_sema(|sema| {
                                let arena = sema.arena_mut();
                                let ty =
                                    arena.parse(file, source[symbol.end..assignment].trim())?;
                                arena.is_callable(ty)
                            })
                            .unwrap_or(false)
                });
        };
        for statement in &body.tree.statements {
            if !(body.base + statement.span.start..=body.base + statement.span.end)
                .contains(&offset)
            {
                continue;
            }
            match &statement.kind {
                StmtKind::Declaration {
                    annotation: Some(annotation),
                    values,
                    ..
                } if direct_value(&body.tree.expressions, values, body.base, offset) => {
                    let ann_slice =
                        source[body.base + annotation.start..body.base + annotation.end].trim();
                    let is_callable = self
                        .with_sema(|sema| {
                            if let (Ok(off), Ok(ann_pos)) = (
                                u32::try_from(offset),
                                u32::try_from(body.base + annotation.start),
                            ) && let Some((region, arena)) =
                                sema.region_and_arena_at(file, source, off, Some(offset))
                            {
                                let env =
                                    super::signatures::visible_local_env(region, source, ann_pos);
                                let ty = arena.parse_in(file, ann_slice, &env)?;
                                return arena.is_callable(ty);
                            }
                            let arena = sema.arena_mut();
                            let ty = arena.parse(file, ann_slice)?;
                            arena.is_callable(ty)
                        })
                        .unwrap_or(false);
                    if is_callable {
                        return true;
                    }
                }
                StmtKind::Assignment {
                    left,
                    operator,
                    right,
                } if left.len() == 1
                    && direct_value(&body.tree.expressions, right, body.base, offset)
                    && &source[body.base + operator.start..body.base + operator.end] == "=" =>
                {
                    let expression = &body.tree.expressions[left[0].0];
                    if matches!(expression.kind, ExprKind::Name)
                        && let Some(binding) = scope.locals.resolve_at(
                            offset,
                            &source[body.base + expression.span.start
                                ..body.base + expression.span.end],
                        )
                        && self
                            .local_binding_signature(
                                &binding.selection,
                                binding.scope_start(),
                                &binding.name,
                            )
                            .is_some()
                    {
                        return true;
                    }
                }
                _ => {}
            }
        }
        self.expects_callable_argument(offset, (&body.tree.expressions, body.base))
    }
}

fn direct_value(expressions: &[Expr], values: &[ExprId], base: usize, offset: usize) -> bool {
    values.iter().any(|value| {
        let mut expression = &expressions[value.0];
        if !(base + expression.span.start..=base + expression.span.end).contains(&offset) {
            return false;
        }
        while let ExprKind::Parenthesized { expression: inner } = expression.kind {
            expression = &expressions[inner.0];
        }
        matches!(
            expression.kind,
            ExprKind::Name | ExprKind::Selector { .. } | ExprKind::Missing
        )
    })
}
