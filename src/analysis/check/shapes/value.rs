//! Declaration slots and expression shapes are inspected only during the query.
use super::kinds as config;
use super::{Shapes, ValueShape};
use crate::index::IndexedSymbol as Symbol;
use crate::syntax::{ExprId, ExprKind, ExpressionTree, Lexer, StmtKind, TokenKind};
use crate::text::SourceMap;
use std::path::Path;

impl Shapes<'_> {
    pub(super) fn global_value(
        &mut self,
        file: &Path,
        symbol: &Symbol,
        depth: usize,
    ) -> ValueShape {
        let declaration = {
            let stored = (file != self.file)
                .then(|| self.workspace.source(file))
                .flatten();
            let source = if file == self.file {
                self.source
            } else {
                match stored.as_deref() {
                    Some(source) => source,
                    None => return ValueShape::default(),
                }
            };
            let map = SourceMap::new(source);
            let Some(start) = map.offset(symbol.range.start) else {
                return ValueShape::default();
            };
            let Some(end) = map.offset(symbol.range.end) else {
                return ValueShape::default();
            };
            if end - start > 16 * 1024 {
                return ValueShape::default();
            }
            (source[start..end].to_owned(), start)
        };
        let text = format!("{{var {}\n}}", declaration.0);
        let parsed = crate::syntax::parse_block(&text);
        if !parsed.diagnostics.is_empty() {
            return ValueShape::default();
        }
        for statement in &parsed.statements {
            if let StmtKind::Declaration {
                names,
                annotation,
                values,
                ..
            } = &statement.kind
            {
                let Some(index) = names
                    .iter()
                    .position(|name| declaration.1 + name.start - 5 == symbol.start)
                else {
                    continue;
                };
                if let Some(span) = annotation {
                    return self.annotation(
                        file,
                        &text[span.clone()],
                        declaration.1 + span.start - 5,
                        depth + 1,
                    );
                }
                if values.len() == names.len() {
                    let span = &parsed.expressions[values[index].0].span;
                    return self.value(
                        file,
                        &text[span.clone()],
                        declaration.1 + span.start - 5,
                        depth + 1,
                    );
                }
            }
        }
        ValueShape::default()
    }

    pub(super) fn value(
        &mut self,
        file: &Path,
        text: &str,
        offset: usize,
        depth: usize,
    ) -> ValueShape {
        self.value_slot(file, text, offset, 0, 1, depth)
    }

    pub(super) fn value_slot(
        &mut self,
        file: &Path,
        text: &str,
        offset: usize,
        result: usize,
        count: usize,
        depth: usize,
    ) -> ValueShape {
        if depth >= 32 || text.len() > 16 * 1024 {
            return ValueShape::default();
        }
        let parsed = crate::syntax::parse_expression(text);
        if !parsed.diagnostics.is_empty() {
            return ValueShape::default();
        }
        if file == self.file && depth <= 1 {
            let inference = crate::analysis::check::Inference::new(
                self.workspace,
                file,
                self.source,
                &self.analysis.locals,
                &parsed.nodes,
                offset,
            );
            if let Some((form, untyped)) = inference.scalar_form(parsed.root, result, count) {
                let shape = super::modifier(form);
                return if untyped {
                    ValueShape::untyped(shape.forms)
                } else {
                    shape
                };
            }
        }
        if result != 0
            || count != 1
            || matches!(
                parsed.nodes[parsed.root.0].kind,
                ExprKind::TypeAssertion { .. }
            )
        {
            return ValueShape::default();
        }
        self.expression_value(file, text, offset, &parsed, parsed.root, depth)
    }

    pub(super) fn expression_value(
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
        let root = &parsed.nodes[id.0];
        match &root.kind {
            ExprKind::Literal => match Lexer::new(&text[root.span.clone()])
                .next()
                .map(|token| token.kind)
            {
                Some(TokenKind::String) => ValueShape::untyped(config::STRING),
                Some(TokenKind::Int | TokenKind::Float | TokenKind::Imag | TokenKind::Char) => {
                    ValueShape::untyped(config::NUMBER)
                }
                _ => ValueShape::default(),
            },
            ExprKind::FunctionLiteral { .. } => ValueShape::typed(config::SIGNATURE),
            ExprKind::Composite {
                literal_type: Some(ty),
                ..
            } => {
                let span = &parsed.nodes[ty.0].span;
                self.annotation(file, &text[span.clone()], offset + span.start, depth + 1)
            }
            ExprKind::Parenthesized { expression } => {
                self.expression_value(file, text, offset, parsed, *expression, depth + 1)
            }
            ExprKind::Unary { operator, operand } => match &text[operator.clone()] {
                "&" => ValueShape::typed(config::POINTER),
                "+" | "-" | "^" | "!" => {
                    self.expression_value(file, text, offset, parsed, *operand, depth + 1)
                }
                _ => ValueShape::default(),
            },
            ExprKind::Binary {
                left,
                operator,
                right,
            } => {
                let operator = &text[operator.clone()];
                // Go comparisons produce an untyped bool regardless of the
                // operands' types; no operand expansion is needed here.
                if matches!(operator, "==" | "!=" | "<" | "<=" | ">" | ">=") {
                    return ValueShape::untyped(config::BOOL);
                }
                let left = self.expression_value(file, text, offset, parsed, *left, depth + 1);
                let right = self.expression_value(file, text, offset, parsed, *right, depth + 1);
                super::operations::binary(operator, left, right)
            }
            ExprKind::Call { .. } => self.builtin_call(file, text, offset, parsed, id, depth + 1),
            ExprKind::Name => {
                let name = &text[root.span.clone()];
                if file == self.file
                    && let Some(binding) = self
                        .analysis
                        .locals
                        .resolve_at(offset + root.span.start, name)
                {
                    return self.binding(binding, depth + 1);
                }
                if let Some((path, symbol)) = self.workspace.unqualified_symbol(file, name) {
                    return self.symbol(path, symbol, depth + 1);
                }
                if crate::analysis::scope::universe::imported_name(self.workspace, file, name) {
                    return ValueShape::default();
                }
                match name {
                    "true" | "false" => ValueShape::untyped(config::BOOL),
                    "iota" => ValueShape::untyped(config::NUMBER),
                    _ => ValueShape::default(),
                }
            }
            _ => ValueShape::default(),
        }
    }
}
