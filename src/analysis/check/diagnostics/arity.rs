//! Count diagnostics from checked operand lists.
use super::{Code, RegionChecker};
use crate::analysis::engine::types::{BasicKind, Operand, TypeId};
use crate::syntax::{ExprId, ExprKind};
use std::ops::Range;

impl RegionChecker<'_, '_> {
    pub(in crate::analysis::engine::check) fn assignment_count(
        &mut self,
        rhs: &[ExprId],
        variables: usize,
        values: usize,
    ) {
        let Some(first) = rhs.first().and_then(|&id| self.body.expression(id)) else {
            return;
        };
        let variables = counted(variables, "variable");
        let values = counted(values, "value");
        let prefix = if rhs.len() == 1
            && let Some(expr) = self.body.expression(self.unwrap_parens(rhs[0]))
            && let ExprKind::Call { function, .. } = expr.kind
            && let Some(callee) = self.body.expression(function)
        {
            let name = self.text(&callee.span);
            if name.len() > 1024 {
                return;
            }
            format!("{name} returns ")
        } else {
            String::new()
        };
        self.compiler_error(
            &first.span,
            Code::WrongAssignCount,
            &format!("assignment mismatch: {variables} but {prefix}{values}"),
        );
    }

    pub(in crate::analysis::engine::check) fn declaration_count(
        &mut self,
        names: &[Range<usize>],
        values: &[ExprId],
    ) {
        if let Some(extra) = values
            .get(names.len())
            .and_then(|&id| self.body.expression(id))
        {
            let text = self.text(&extra.span);
            if text.len() <= 1024 {
                self.compiler_error(
                    &extra.span,
                    Code::WrongAssignCount,
                    &format!("extra init expr {text}"),
                );
            }
        } else if let Some(name) = names.get(values.len()) {
            let text = self.text(name);
            if text.len() <= 1024 {
                self.compiler_error(
                    name,
                    Code::WrongAssignCount,
                    &format!("missing init expr for {text}"),
                );
            }
        }
    }

    pub(in crate::analysis::engine::check) fn return_count(
        &mut self,
        statement: &Range<usize>,
        expressions: &[ExprId],
        actual: &[Operand],
    ) {
        let expected = self.env.result_types.len();
        let too_many = actual.len() > expected;
        let site = if expressions.len() == 1 {
            expressions.first()
        } else if too_many {
            expressions.get(expected)
        } else {
            expressions.last()
        };
        let span = site.and_then(|&id| self.body.expression(id)).map_or_else(
            || statement.start..statement.start + "return".len(),
            |expr| expr.span.clone(),
        );
        let Some(have) = self.type_summary(actual.iter().map(|op| Some(op.ty)), Ellipsis::None)
        else {
            return;
        };
        let Some(want) =
            self.type_summary(self.env.result_types.clone().into_iter(), Ellipsis::None)
        else {
            return;
        };
        let qualifier = if too_many { "too many" } else { "not enough" };
        self.compiler_error(
            &span,
            Code::WrongResultCount,
            &format!("{qualifier} return values\n\thave {have}\n\twant {want}"),
        );
    }

    pub(in crate::analysis::engine::check) fn no_call_value(
        &mut self,
        function: ExprId,
        arguments: &[ExprId],
        spread: bool,
        span: &Range<usize>,
    ) {
        let Some(call) = self.call_description(function, arguments, spread, span) else {
            return;
        };
        self.compiler_error(
            span,
            Code::TooManyValues,
            &format!("{call} (no value) used as value"),
        );
    }

    pub(in crate::analysis::engine::check) fn call_description(
        &self,
        function: ExprId,
        arguments: &[ExprId],
        spread: bool,
        span: &Range<usize>,
    ) -> Option<String> {
        if span.len() > 1024 {
            return None;
        }
        let function = self.body.expression(function)?;
        let mut call = self.text(&function.span).to_owned();
        call.push('(');
        for (index, &argument) in arguments.iter().enumerate() {
            let argument = self.body.expression(argument)?;
            if index != 0 {
                call.push_str(", ");
            }
            call.push_str(self.text(&argument.span));
        }
        if spread {
            call.push_str("...");
        }
        call.push(')');
        Some(call)
    }

    #[allow(clippy::too_many_arguments)]
    pub(in crate::analysis::engine::check) fn check_argument_count(
        &mut self,
        function: ExprId,
        arguments: &[ExprId],
        call_span: &Range<usize>,
        actual: &[Operand],
        params: &[TypeId],
        spread: bool,
        variadic: bool,
    ) -> bool {
        let min = params
            .len()
            .saturating_sub(usize::from(variadic && !spread));
        let too_many = (!variadic || spread) && actual.len() > params.len();
        if (variadic || !spread) && actual.len() >= min && !too_many {
            return true;
        }
        let Some(callee) = self.body.expression(function) else {
            return false;
        };
        let callee = self.text(&callee.span);
        if callee.len() > 1024 {
            return false;
        }
        if spread && !variadic {
            self.compiler_error(
                call_span,
                Code::NonVariadicDotDotDot,
                &format!("cannot use ... in call to non-variadic {callee}"),
            );
            return false;
        }
        if !actual.iter().all(|operand| operand.mode.is_value()) {
            return false;
        }
        let Some(have) = self.type_summary(
            actual.iter().map(|operand| Some(operand.ty)),
            if spread {
                Ellipsis::After
            } else {
                Ellipsis::None
            },
        ) else {
            return false;
        };
        let Some(want) = self.type_summary(
            params.iter().copied().map(Some),
            if variadic {
                Ellipsis::BeforeElement
            } else {
                Ellipsis::None
            },
        ) else {
            return false;
        };
        let (span, qualifier) = if too_many {
            let Some(extra) = arguments
                .get(min)
                .or_else(|| arguments.last())
                .and_then(|&id| self.body.expression(id))
            else {
                return false;
            };
            (extra.span.clone(), "too many")
        } else {
            let close = call_span.end.saturating_sub(1);
            (close..close, "not enough")
        };
        self.compiler_error(
            &span,
            Code::WrongArgCount,
            &format!("{qualifier} arguments in call to {callee}\n\thave {have}\n\twant {want}"),
        );
        false
    }

    // Bound message construction independently of the parsed operand list.
    fn type_summary(
        &mut self,
        types: impl ExactSizeIterator<Item = Option<TypeId>>,
        ellipsis: Ellipsis,
    ) -> Option<String> {
        let last = types.len().checked_sub(1);
        let mut text = String::from("(");
        for (index, ty) in types.enumerate() {
            if index != 0 {
                text.push_str(", ");
            }
            let ty = ty?;
            let kind = self.arena.basic_kind(ty);
            if kind.is_some_and(|kind| kind.untyped() && kind.numeric_rank().is_some()) {
                text.push_str("number");
            } else if kind == Some(BasicKind::UntypedNil) {
                text.push_str("nil");
            } else {
                let rendered = self.arena.render_presentation(ty, self.file)?;
                let rendered = rendered.strip_prefix("untyped ").unwrap_or(&rendered);
                if last == Some(index) && matches!(ellipsis, Ellipsis::BeforeElement) {
                    text.push_str("...");
                    text.push_str(rendered.strip_prefix("[]")?);
                } else {
                    text.push_str(rendered);
                }
            }
            if last == Some(index) && matches!(ellipsis, Ellipsis::After) {
                text.push_str("...");
            }
            if text.len() > 4096 {
                return None;
            }
        }
        text.push(')');
        Some(text)
    }
}

enum Ellipsis {
    None,
    BeforeElement,
    After,
}

fn counted(count: usize, unit: &str) -> String {
    let suffix = if count == 1 { "" } else { "s" };
    format!("{count} {unit}{suffix}")
}
