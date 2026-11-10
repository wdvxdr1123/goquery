//! Query-local scopes along the cursor's enclosing function-literal path.
use crate::analysis::scope::Locals;
use crate::syntax::{BlockTree, Expr, ExprKind, TokenKind};
use std::collections::HashMap;
use std::ops::Range;

pub(crate) struct CursorBody {
    pub base: usize,
    pub tree: BlockTree,
}

pub(crate) struct CursorScope {
    pub locals: Locals,
    pub body: Option<CursorBody>,
}

impl Locals {
    pub(crate) fn scope_at_cursor(
        source: &str,
        offset: usize,
        function: Option<(usize, Range<usize>)>,
    ) -> Result<CursorScope, (i32, String)> {
        let mut active_body = None;
        let mut locals = Self::empty();
        let mut literal = if let Some((start, body)) =
            function.filter(|(_, body)| contains_cursor(source, body, offset))
        {
            let body_offset = body.start.checked_sub(start).ok_or((
                crate::error::INVALID_PARAMS,
                "stale function boundary".into(),
            ))?;
            let text = source
                .get(start..body.end)
                .ok_or((crate::error::INVALID_PARAMS, "stale function range".into()))?;
            let parsed = crate::syntax::parse_block(&source[body.clone()]);
            locals = Self::from_body(text, body_offset, &parsed);
            locals.rebase(start);
            let literal = enclosing_literal(source, &parsed.expressions, body.start, offset);
            active_body = Some(CursorBody {
                base: body.start,
                tree: parsed,
            });
            literal
        } else {
            // Package-level initializers can contain closures too. Scan tokens
            // without building a whole-file tree to find such a literal.
            let mut tokens = crate::syntax::Lexer::new(source).peekable();
            let mut found = None;
            let mut skip_until = 0;
            while let Some(token) = tokens.next() {
                if token.span.start >= offset {
                    break;
                }
                if token.span.start < skip_until {
                    continue;
                }
                if token.kind == TokenKind::Func
                    && tokens
                        .peek()
                        .is_some_and(|next| next.kind == TokenKind::LParen)
                {
                    let parsed = crate::syntax::parse_expression(&source[token.span.start..]);
                    found = enclosing_literal(source, &parsed.nodes, token.span.start, offset);
                    if found.is_some() {
                        break;
                    }
                    for expression in &parsed.nodes {
                        if let ExprKind::FunctionLiteral { body, .. } = &expression.kind {
                            skip_until = skip_until.max(token.span.start + body.end);
                        }
                    }
                }
            }
            found
        };
        for _ in 0..128 {
            let Some((signature, body)) = literal else {
                if offset == source.len() {
                    locals.extend_open_scopes(source);
                }
                return Ok(CursorScope {
                    locals,
                    body: active_body,
                });
            };
            drop(active_body.take());
            let text = &source[signature.start..body.end];
            let body_offset = body.start - signature.start;
            let parsed_signature = crate::syntax::parse_literal_signature(&text[..body_offset]);
            let mut parameters = Self::from_parsed_signature(text, body_offset, &parsed_signature);
            parameters.rebase(signature.start);
            for parameter in &mut parameters.bindings {
                if parameter.declared_type.as_ref().is_some_and(|ty| {
                    locals
                        .resolve_at(signature.start, ty.split('.').next().expect("named type"))
                        .is_some()
                }) {
                    parameter.declared_type = None;
                }
            }
            locals.bindings.extend(parameters.bindings);
            let parsed = crate::syntax::parse_block(&source[body.clone()]);
            locals.append_body(source, body.start, &parsed);
            literal = enclosing_literal(source, &parsed.expressions, body.start, offset);
            active_body = Some(CursorBody {
                base: body.start,
                tree: parsed,
            });
        }
        if literal.is_none() {
            if offset == source.len() {
                locals.extend_open_scopes(source);
            }
            Ok(CursorScope {
                locals,
                body: active_body,
            })
        } else {
            Err((
                crate::error::REQUEST_FAILED,
                "cursor query exceeds 128 nested function scopes".into(),
            ))
        }
    }

    pub(crate) fn rebase(&mut self, offset: usize) {
        for case in &mut self.switch_cases {
            case.selection += offset;
            case.scope_start += offset;
            case.operand.start += offset;
            case.operand.end += offset;
            for ty in &mut case.types {
                ty.start += offset;
                ty.end += offset;
            }
        }
        for (selection, bound) in self
            .type_constraints
            .iter_mut()
            .chain(&mut self.receiver_constraints)
        {
            selection.start += offset;
            selection.end += offset;
            bound.start += offset;
            bound.end += offset;
        }
        for binding in &mut self.bindings {
            binding.selection.start += offset;
            binding.selection.end += offset;
            binding.scope.start += offset;
            binding.scope.end += offset;
            binding.visible_from += offset;
            if let Some(annotation) = &mut binding.annotation {
                annotation.start += offset;
                annotation.end += offset;
            }
            if let Some(initializer) = &mut binding.initializer {
                initializer.span.start += offset;
                initializer.span.end += offset;
            }
        }
    }

    // Incomplete editor input still has an active scope at EOF. Closed inner
    // blocks ending at the same byte must not leak their variables outside.
    fn extend_open_scopes(&mut self, source: &str) {
        let mut open = HashMap::new();
        for binding in &mut self.bindings {
            if binding.scope.end == source.len() {
                let active = open.entry(binding.scope.start).or_insert_with(|| {
                    let text = &source[binding.scope.clone()];
                    let case = crate::syntax::Lexer::new(text).next().is_some_and(|token| {
                        matches!(token.kind, TokenKind::Case | TokenKind::Default)
                    });
                    case || unclosed(text)
                });
                if *active {
                    binding.scope.end += 1;
                }
            }
        }
    }
}

fn enclosing_literal(
    source: &str,
    expressions: &[Expr],
    base: usize,
    offset: usize,
) -> Option<(Range<usize>, Range<usize>)> {
    expressions.iter().find_map(|expression| {
        let ExprKind::FunctionLiteral { signature, body } = &expression.kind else {
            return None;
        };
        let body = base + body.start..base + body.end;
        contains_cursor(source, &body, offset)
            .then(|| (base + signature.start..base + signature.end, body))
    })
}

fn contains_cursor(source: &str, body: &Range<usize>, offset: usize) -> bool {
    body.contains(&offset)
        || offset == source.len() && body.end == offset && unclosed(&source[body.clone()])
}

fn unclosed(source: &str) -> bool {
    let mut braces = 0;
    for token in crate::syntax::Lexer::new(source) {
        match token.kind {
            TokenKind::LBrace => braces += 1,
            TokenKind::RBrace => braces -= 1,
            _ => {}
        }
    }
    braces > 0
}
