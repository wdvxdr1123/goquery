//! Operators and universe calls preserve typed versus untyped basic values.
use super::kinds as config;
use super::{Shapes, Typedness, ValueShape};
use crate::syntax::{ExprId, ExprKind, ExpressionTree};
use std::path::Path;

impl Shapes<'_> {
    pub(super) fn builtin_call(
        &mut self,
        file: &Path,
        text: &str,
        offset: usize,
        parsed: &ExpressionTree,
        id: ExprId,
        depth: usize,
    ) -> ValueShape {
        if depth >= 32 {
            return ValueShape::default();
        }
        let nodes = &parsed.nodes;
        let ExprKind::Call {
            function,
            arguments,
            ..
        } = &nodes[id.0].kind
        else {
            return ValueShape::default();
        };
        let mut function = *function;
        while let ExprKind::Parenthesized { expression } = nodes[function.0].kind {
            function = expression;
        }
        let callee = &nodes[function.0];
        if !matches!(callee.kind, ExprKind::Name) {
            return ValueShape::default();
        }
        let name = &text[callee.span.clone()];
        if !matches!(
            name,
            "len" | "cap" | "copy" | "real" | "imag" | "min" | "max" | "complex"
        ) {
            return ValueShape::default();
        }
        if !crate::analysis::scope::universe::visible(
            self.workspace,
            file,
            (file == self.file).then_some(&self.analysis.locals),
            offset + callee.span.start,
            name,
        ) {
            return ValueShape::default();
        }
        match name {
            "len" | "cap" if arguments.len() == 1 => ValueShape::typed(config::NUMBER),
            "copy" if arguments.len() == 2 => ValueShape::typed(config::NUMBER),
            "real" | "imag" if arguments.len() == 1 => {
                let value =
                    self.expression_value(file, text, offset, parsed, arguments[0], depth + 1);
                if value.forms == config::NUMBER {
                    value
                } else {
                    ValueShape::default()
                }
            }
            "min" | "max" | "complex" if !arguments.is_empty() => {
                if name == "complex" && arguments.len() != 2 {
                    return ValueShape::default();
                }
                let mut values = arguments
                    .iter()
                    .map(|id| self.expression_value(file, text, offset, parsed, *id, depth + 1));
                let value = values.next().unwrap_or_default();
                let value = values.fold(value, combine);
                if value.forms == config::NUMBER
                    || name != "complex" && value.forms == config::STRING
                {
                    value
                } else {
                    ValueShape::default()
                }
            }
            _ => ValueShape::default(),
        }
    }
}

pub(super) fn binary(operator: &str, left: ValueShape, right: ValueShape) -> ValueShape {
    match operator {
        "<<" | ">>" if left.forms == config::NUMBER => left,
        _ => {
            let value = combine(left, right);
            if matches!(operator, "&&" | "||") && value.forms == config::BOOL
                || operator == "+" && value.forms == config::STRING
                || matches!(
                    operator,
                    "+" | "-" | "*" | "/" | "%" | "&" | "|" | "^" | "&^"
                ) && value.forms == config::NUMBER
            {
                value
            } else {
                ValueShape::default()
            }
        }
    }
}

fn combine(left: ValueShape, right: ValueShape) -> ValueShape {
    if left.forms != right.forms {
        return ValueShape::default();
    }
    let typedness = if left.typedness == Typedness::Untyped && right.typedness == Typedness::Untyped
    {
        Typedness::Untyped
    } else {
        Typedness::Typed
    };
    ValueShape {
        forms: left.forms,
        typedness,
    }
}
