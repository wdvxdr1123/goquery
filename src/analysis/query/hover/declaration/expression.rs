//! Operator spacing for the existing expression arena, following Go's normal
//! and compact precedence modes without rewriting operands or literals.
use crate::analysis::query::budget::{MAX_DEPTH, MAX_NODES};
use crate::syntax::{Expr, ExprId, ExprKind, Lexer};

pub(super) fn spaces(text: &str, expressions: &[Expr]) -> Option<Vec<(usize, bool)>> {
    let mut children = vec![false; expressions.len()];
    for expression in expressions {
        each_child(&expression.kind, |id| children[id.0] = true);
    }
    let mut walker = Spacing {
        text,
        expressions,
        steps: MAX_NODES,
        output: Vec::new(),
    };
    for (index, child) in children.into_iter().enumerate() {
        if !child {
            walker.visit(ExprId(index), 1, 0)?;
        }
    }
    walker.output.sort_unstable_by_key(|entry| entry.0);
    Some(walker.output)
}

struct Spacing<'a> {
    text: &'a str,
    expressions: &'a [Expr],
    steps: usize,
    output: Vec<(usize, bool)>,
}

impl Spacing<'_> {
    fn step(&mut self, level: usize) -> Option<()> {
        self.steps = self.steps.checked_sub(1)?;
        (level < MAX_DEPTH).then_some(())
    }

    fn rank(&self, id: ExprId) -> Option<u8> {
        let ExprKind::Binary { operator, .. } = &self.expressions[id.0].kind else {
            return None;
        };
        Some(
            Lexer::new(self.text.get(operator.clone())?)
                .next()?
                .kind
                .precedence(),
        )
    }

    fn chain(&mut self, id: ExprId, level: usize) -> Option<(bool, bool, u8)> {
        self.step(level)?;
        let (left, right, operator) = match &self.expressions[id.0].kind {
            ExprKind::Binary {
                left,
                right,
                operator,
            } => (*left, *right, operator.clone()),
            _ => return None,
        };
        let rank = self.rank(id)?;
        let mut a = rank == 4;
        let mut b = rank == 5;
        let mut clash = 0;
        for (child, include) in [
            (left, self.rank(left).is_some_and(|r| r >= rank)),
            (right, self.rank(right).is_some_and(|r| r > rank)),
        ] {
            if include {
                let (ca, cb, cc) = self.chain(child, level + 1)?;
                a |= ca;
                b |= cb;
                clash = clash.max(cc);
            }
        }
        if let ExprKind::Unary {
            operator: unary, ..
        } = &self.expressions[right.0].kind
        {
            let pair = format!("{}{}", &self.text[operator], &self.text[unary.clone()]);
            clash = clash.max(match pair.as_str() {
                "/*" | "&&" | "&^" => 5,
                "++" | "--" => 4,
                _ => 0,
            });
        }
        Some((a, b, clash))
    }

    fn visit(&mut self, id: ExprId, depth: usize, level: usize) -> Option<()> {
        self.step(level)?;
        let expressions = self.expressions;
        let expression = &expressions[id.0];
        match &expression.kind {
            ExprKind::Binary {
                left,
                right,
                operator,
            } => {
                let (left, right, operator) = (*left, *right, operator.clone());
                let rank = self.rank(id)?;
                let (a, b, clash) = self.chain(id, level)?;
                let cutoff = if clash > 0 {
                    clash + 1
                } else if depth == 1 {
                    if a && b { 5 } else { 6 }
                } else {
                    4
                };
                self.output.push((operator.start, rank < cutoff));
                let different = self.rank(left) != Some(rank);
                self.visit(left, depth + usize::from(different), level + 1)?;
                self.visit(right, depth + 1, level + 1)?;
            }
            ExprKind::Parenthesized { expression } => {
                self.visit(*expression, depth.saturating_sub(1).max(1), level + 1)?
            }
            ExprKind::Index { operand, arguments } => {
                self.visit(*operand, 1, level + 1)?;
                for argument in arguments {
                    self.visit(*argument, depth + 1, level + 1)?;
                }
            }
            ExprKind::Composite {
                literal_type,
                elements,
            } => {
                if let Some(ty) = literal_type {
                    self.visit(*ty, depth, level + 1)?;
                }
                for element in elements {
                    self.visit(*element, 1, level + 1)?;
                }
            }
            ExprKind::KeyValue { key, value } => {
                self.visit(*key, 1, level + 1)?;
                self.visit(*value, 1, level + 1)?;
            }
            ExprKind::Unary { operator, operand } => {
                let next = if &self.text[operator.clone()] == "*" {
                    1
                } else {
                    depth
                };
                self.visit(*operand, next, level + 1)?;
            }
            ExprKind::Slice { .. } | ExprKind::FunctionLiteral { .. } => return None,
            ExprKind::Call {
                function,
                arguments,
                ..
            } => {
                self.visit(*function, depth, level + 1)?;
                for argument in arguments {
                    self.visit(*argument, depth, level + 1)?;
                }
            }
            ExprKind::Selector { operand, .. }
            | ExprKind::TypeAssertion { operand, .. }
            | ExprKind::TypeSwitchGuard { operand } => self.visit(*operand, depth, level + 1)?,
            _ => {}
        }
        Some(())
    }
}

fn each_child(kind: &ExprKind, mut visit: impl FnMut(ExprId)) {
    match kind {
        ExprKind::Unary { operand, .. }
        | ExprKind::Selector { operand, .. }
        | ExprKind::TypeAssertion { operand, .. }
        | ExprKind::TypeSwitchGuard { operand } => visit(*operand),
        ExprKind::Parenthesized { expression } => visit(*expression),
        ExprKind::Binary { left, right, .. } => {
            visit(*left);
            visit(*right);
        }
        ExprKind::KeyValue { key, value } => {
            visit(*key);
            visit(*value);
        }
        ExprKind::Call {
            function,
            arguments,
            ..
        } => {
            visit(*function);
            for id in arguments {
                visit(*id);
            }
        }
        ExprKind::Index { operand, arguments } => {
            visit(*operand);
            for id in arguments {
                visit(*id);
            }
        }
        ExprKind::Composite {
            literal_type,
            elements,
        } => {
            if let Some(id) = literal_type {
                visit(*id);
            }
            for id in elements {
                visit(*id);
            }
        }
        ExprKind::Slice {
            operand,
            low,
            high,
            max,
            ..
        } => {
            visit(*operand);
            for id in low.iter().chain(high).chain(max) {
                visit(*id);
            }
        }
        _ => {}
    }
}
