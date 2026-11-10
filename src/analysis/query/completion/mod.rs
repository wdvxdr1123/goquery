//! Completion semantic facts remain independent of snippets and wire items.
mod presentation;
use super::Context;
use crate::analysis::scope::CursorScope;
use crate::syntax::{Expr, ExprId, ExprKind, Lexer, StmtKind, TokenKind};
use crate::text::SourceMap;

use crate::analysis::check::callable_signature::CallableSignature;

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

impl Context<'_> {
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
                            .workspace
                            .callable_type(file, &source[symbol.end..assignment])
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
                    if self.workspace.callable_annotation(
                        file,
                        source,
                        &scope.locals,
                        body.base + annotation.start..body.base + annotation.end,
                    ) {
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
                            .workspace
                            .binding_signature(file, source, &scope.locals, binding)
                            .is_some()
                    {
                        return true;
                    }
                }
                _ => {}
            }
        }
        self.expects_callable_argument(offset, &scope.locals, (&body.tree.expressions, body.base))
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
