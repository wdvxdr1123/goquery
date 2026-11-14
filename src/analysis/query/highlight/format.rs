//! Signature-based format calls, shared by tokens and document highlights.
use super::{FormatDirective, MAX_FORMAT_BYTES, MAX_PRINTF_CALLS, PrintfCall};
use crate::analysis::check::Inference;
use crate::analysis::query::budget::{MEMO_BYTES, MEMO_ENTRIES};
use crate::analysis::scope::Locals;
use crate::syntax::{Expr, ExprKind, Lexer, Token, TokenKind};
use crate::workspace::Workspace;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::Path;

// Only one active body's expressions and locals survive between candidates.
// Nested closures replace that scope; no whole-file AST or type cache is kept.
const MAX_SCOPES: usize = 512;
const MAX_ARGUMENT_TOKEN_VISITS: usize = super::MAX_STATEMENTS * super::MAX_NESTED_SCOPES;

struct CallScope {
    locals: Locals,
    expressions: Vec<Expr>,
    base: usize,
    arities: RefCell<HashMap<(usize, String), Option<usize>>>,
    arity_bytes: Cell<usize>,
}

impl CallScope {
    fn at(workspace: &Workspace, file: &Path, source: &str, offset: usize) -> Option<Self> {
        let scope = super::super::cursor_scope(workspace, file, source, offset).ok()?;
        if let Some(body) = scope.body {
            return Some(Self {
                locals: scope.locals,
                expressions: body.tree.expressions,
                base: body.base,
                arities: RefCell::new(HashMap::new()),
                arity_bytes: Cell::new(0),
            });
        }
        // Package-level initializers have no enclosing statement body.
        let declaration = workspace
            .files
            .get(file)?
            .symbols
            .iter()
            .find_map(|symbol| {
                let declaration = symbol.declaration()?;
                (symbol.is_value() && declaration.contains(&offset)).then_some(declaration)
            })?;
        let text = source.get(declaration.clone())?;
        let base = declaration.start + super::top_level_assign(text)? + 1;
        let parsed = crate::syntax::parse_expression(source.get(base..declaration.end)?);
        parsed.diagnostics.is_empty().then_some(Self {
            locals: scope.locals,
            expressions: parsed.nodes,
            base,
            arities: RefCell::new(HashMap::new()),
            arity_bytes: Cell::new(0),
        })
    }

    fn call(&self, source: &str, open: usize, end: usize) -> Option<&Expr> {
        self.expressions.iter().find(|expression| {
            let ExprKind::Call { function, .. } = &expression.kind else {
                return false;
            };
            let function = &self.expressions[function.0];
            self.base + expression.span.end == end
                && source
                    .get(self.base + function.span.end..open)
                    .is_some_and(|gap| Lexer::new(gap).next().is_none())
        })
    }

    /// Repeated calls through one lexical binding have the same signature arity.
    /// Cache only name/selector chains: evaluated receivers keep full inference.
    fn format_parameter(
        &self,
        workspace: &Workspace,
        file: &Path,
        source: &str,
        function: crate::syntax::ExprId,
    ) -> Option<usize> {
        let mut root = function;
        let mut binding = None;
        for _ in 0..32 {
            match &self.expressions[root.0].kind {
                ExprKind::Name => {
                    let span = &self.expressions[root.0].span;
                    let offset = self.base + span.start;
                    binding = self
                        .locals
                        .resolve_at(offset, source.get(offset..self.base + span.end)?);
                    break;
                }
                ExprKind::Selector { operand, .. } => root = *operand,
                ExprKind::Parenthesized { expression } => root = *expression,
                _ => break,
            }
        }
        let span = &self.expressions[function.0].span;
        let text = source.get(self.base + span.start..self.base + span.end)?;
        let key = binding
            .filter(|_| text.len() <= MEMO_BYTES)
            .map(|binding| (binding.selection.start, text.to_owned()));
        if let Some(key) = &key
            && let Some(value) = self.arities.borrow().get(key)
        {
            return *value;
        }
        let value = Inference::new(
            workspace,
            file,
            source,
            &self.locals,
            &self.expressions,
            self.base,
        )
        .format_parameter(function);
        if let Some(key) = key
            && self.arities.borrow().len() < MEMO_ENTRIES
            && let Some(bytes) = self
                .arity_bytes
                .get()
                .checked_add(key.1.len() + std::mem::size_of_val(&value))
            && bytes <= MEMO_BYTES
        {
            self.arity_bytes.set(bytes);
            self.arities.borrow_mut().insert(key, value);
        }
        value
    }
}

pub(super) fn calls_at(
    workspace: &Workspace,
    file: &Path,
    source: &str,
    offset: usize,
) -> Vec<PrintfCall> {
    calls(workspace, file, source, Some(offset))
}

fn calls(
    workspace: &Workspace,
    file: &Path,
    source: &str,
    cursor: Option<usize>,
) -> Vec<PrintfCall> {
    let trace = workspace.trace.queries.then(std::time::Instant::now);
    let mut scope_time = std::time::Duration::ZERO;
    let mut signature_time = std::time::Duration::ZERO;
    let tokens: Vec<_> = Lexer::new(source).collect();
    let mut active: Option<CallScope> = None;
    let mut scopes = 0usize;
    let mut candidates = 0usize;
    let mut out = Vec::new();
    let mut argument_visits = MAX_ARGUMENT_TOKEN_VISITS;
    for (open, token) in tokens.iter().enumerate() {
        if cursor.is_some_and(|offset| token.span.start > offset) {
            break;
        }
        if candidates >= MAX_PRINTF_CALLS || scopes >= MAX_SCOPES || argument_visits == 0 {
            break;
        }
        if token.kind != TokenKind::LParen
            || open == 0
            || !matches!(
                tokens[open - 1].kind,
                TokenKind::Ident | TokenKind::RParen | TokenKind::RBrack | TokenKind::RBrace
            )
        {
            continue;
        }
        let Some((close, arguments)) = call_arguments(&tokens, open, &mut argument_visits) else {
            continue;
        };
        // Highlighting needs only enclosing calls. Do not build scopes or
        // resolve signatures for unrelated format calls elsewhere in the file.
        if cursor.is_some_and(|offset| offset >= tokens[close].span.end) {
            continue;
        }
        // This is only a cost prefilter. The literal's position comes from the
        // resolved signature, never its text or the callable's spelling.
        if !arguments.iter().any(|(start, end)| {
            *end == start + 1
                && tokens[*start].kind == TokenKind::String
                && source.get(tokens[*start].span.clone()).is_some_and(|text| {
                    text.len() <= MAX_FORMAT_BYTES
                        && crate::syntax::decode_string_literal(text)
                            .is_some_and(|bytes| bytes.contains(&b'%'))
                })
        }) {
            continue;
        }
        candidates += 1;
        let end = tokens[close].span.end;
        if active
            .as_ref()
            .and_then(|scope| scope.call(source, token.span.start, end))
            .is_none()
        {
            // Drop the previous arena before parsing another body.
            drop(active.take());
            scopes += 1;
            let phase = workspace.trace.queries.then(std::time::Instant::now);
            active = CallScope::at(workspace, file, source, token.span.start);
            if let Some(phase) = phase {
                scope_time += phase.elapsed();
            }
        }
        let Some(scope) = active.as_ref() else {
            continue;
        };
        let Some(call) = scope.call(source, token.span.start, end) else {
            continue;
        };
        let ExprKind::Call {
            function,
            arguments,
            ..
        } = &call.kind
        else {
            continue;
        };
        let phase = workspace.trace.queries.then(std::time::Instant::now);
        let format_index = scope.format_parameter(workspace, file, source, *function);
        if let Some(phase) = phase {
            signature_time += phase.elapsed();
        }
        let Some(format_index) = format_index else {
            continue;
        };
        let Some(literal) = arguments
            .get(format_index)
            .map(|id| &scope.expressions[id.0])
        else {
            continue;
        };
        if !matches!(literal.kind, ExprKind::Literal) {
            continue;
        }
        let format_span = scope.base + literal.span.start..scope.base + literal.span.end;
        if super::format_literal_directives(source, format_span.clone()).is_empty() {
            continue;
        }
        out.push(PrintfCall {
            span: scope.base + call.span.start..scope.base + call.span.end,
            arguments: arguments
                .iter()
                .map(|id| {
                    let span = &scope.expressions[id.0].span;
                    scope.base + span.start..scope.base + span.end
                })
                .collect(),
            format_span,
            format_index,
        });
    }
    if let Some(trace) = trace {
        eprintln!(
            "format calls: candidates {candidates}; scopes {scopes} {scope_time:?}; signatures {signature_time:?}; argument tokens {}; total {:?}",
            MAX_ARGUMENT_TOKEN_VISITS - argument_visits,
            trace.elapsed(),
        );
    }
    out
}

pub(crate) fn format_directives(
    workspace: &Workspace,
    file: &Path,
    source: &str,
) -> Vec<FormatDirective> {
    let mut rows: Vec<_> = calls(workspace, file, source, None)
        .into_iter()
        .filter_map(|call| {
            // gopls splits multiline literals into line tokens before it tries
            // format splitting. Argument highlights still use these calls.
            (!source.get(call.format_span.clone())?.contains('\n')).then(|| FormatDirective {
                directives: super::format_literal_directives(source, call.format_span.clone()),
                literal: call.format_span,
            })
        })
        .collect();
    rows.sort_unstable_by_key(|row| row.literal.start);
    rows.dedup_by(|a, b| a.literal == b.literal);
    rows
}

/// Top-level argument token ranges of one call, starting at its `(`.
fn call_arguments(
    tokens: &[Token],
    open: usize,
    remaining: &mut usize,
) -> Option<(usize, Vec<(usize, usize)>)> {
    let mut depth = 0usize;
    let mut arguments = Vec::new();
    let mut start = open + 1;
    for (index, token) in tokens
        .iter()
        .enumerate()
        .skip(open + 1)
        .take(super::MAX_STATEMENTS)
    {
        *remaining = remaining.checked_sub(1)?;
        match token.kind {
            TokenKind::LParen | TokenKind::LBrack | TokenKind::LBrace => depth += 1,
            TokenKind::RParen if depth == 0 => {
                if start < index {
                    arguments.push((start, index));
                }
                return Some((index, arguments));
            }
            TokenKind::RParen | TokenKind::RBrack | TokenKind::RBrace => {
                depth = depth.checked_sub(1)?;
            }
            TokenKind::Comma if depth == 0 => {
                if start < index {
                    arguments.push((start, index));
                }
                start = index + 1;
            }
            _ => {}
        }
    }
    None
}
