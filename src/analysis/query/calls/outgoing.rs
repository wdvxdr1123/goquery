//! Expand nested bodies on demand, attributing their calls to the named function.
use super::{CallGroup, Groups, finish, record_call};
use crate::analysis::check::Inference;
use crate::analysis::query::{Context, SourceRange};
use crate::analysis::scope::Locals;
use crate::index::IndexedSymbol as Symbol;
use crate::syntax::ExprKind;
use crate::text::SourceMap;
use std::path::Path;

impl<'a, View> Context<'a, View> {
    pub(crate) fn outgoing_calls(
        &self,
        file: &'a Path,
        target: &'a Symbol,
    ) -> Result<Vec<CallGroup<'a>>, (i32, String)> {
        let mut groups = Groups::new();
        self.workspace.trace_candidates("outgoingCalls", 1);
        let Some(source) = self.workspace.source(file) else {
            return Ok(finish(groups));
        };
        let map = SourceMap::new(&source);
        let Some(start) = map.offset(target.range.start) else {
            return Ok(finish(groups));
        };
        let initializer = target.is_value();
        let body = if initializer {
            let Some(end) = map.offset(target.range.end) else {
                return Ok(finish(groups));
            };
            start..end
        } else {
            let Some(body) = target.body.clone() else {
                return Ok(finish(groups));
            };
            body
        };
        let mut work = vec![(start..body.start, body, 0)];
        let mut locals = Locals::empty();
        let mut count = 0;
        let mut bodies = 0;
        while let Some((signature, body, depth)) = work.pop() {
            bodies += 1;
            if depth >= 128 || bodies > 4096 {
                return Err((
                    crate::error::REQUEST_FAILED,
                    "call hierarchy exceeds nested-body query limits".into(),
                ));
            }
            let (parsed, base) = if initializer && depth == 0 {
                let Some(base) = body.start.checked_sub(5) else {
                    return Ok(finish(groups));
                };
                (
                    crate::syntax::parse_block(&format!("{{var {}\n}}", &source[body.clone()])),
                    base,
                )
            } else {
                let header = &source[signature.clone()];
                let parsed_header = if depth == 0 {
                    crate::syntax::parse_signature(header)
                } else {
                    crate::syntax::parse_literal_signature(header)
                };
                let mut parameters = Locals::from_parsed_signature(
                    &source[signature.start..body.end],
                    body.start - signature.start,
                    &parsed_header,
                );
                parameters.rebase(signature.start);
                locals.extend(parameters);
                let parsed = crate::syntax::parse_block(&source[body.clone()]);
                locals.append_body(&source, body.start, &parsed);
                (parsed, body.start)
            };
            for expression in &parsed.expressions {
                if let ExprKind::FunctionLiteral {
                    signature,
                    body: nested,
                } = &expression.kind
                {
                    if work.len() == 4096 {
                        return Err((
                            crate::error::REQUEST_FAILED,
                            "call hierarchy exceeds nested-body query limits".into(),
                        ));
                    }
                    work.push((
                        base + signature.start..base + signature.end,
                        base + nested.start..base + nested.end,
                        depth + 1,
                    ));
                }
                let ExprKind::Call { function, .. } = expression.kind else {
                    continue;
                };
                let mut selected = &parsed.expressions[function.0];
                loop {
                    let operand = match selected.kind {
                        ExprKind::Parenthesized { expression } => expression,
                        ExprKind::Index { operand, .. } => operand,
                        _ => break,
                    };
                    selected = &parsed.expressions[operand.0];
                }
                let range = match &selected.kind {
                    ExprKind::Name => &selected.span,
                    ExprKind::Selector { field, .. } => field,
                    _ => continue,
                };
                let Some((callee_file, callee)) = Inference::new(
                    self.workspace,
                    file,
                    &source,
                    &locals,
                    &parsed.expressions,
                    base,
                )
                .callee(function)
                .filter(|(_, symbol)| symbol.is_callable()) else {
                    continue;
                };
                record_call(
                    &mut groups,
                    callee_file,
                    callee,
                    SourceRange::new(&map, base + range.start..base + range.end),
                    &mut count,
                )?;
            }
        }
        Ok(finish(groups))
    }
}
