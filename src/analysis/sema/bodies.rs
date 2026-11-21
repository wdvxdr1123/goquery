//! One bounded request-local syntax body, shared by checking and candidates.
use super::Semantics;
use crate::analysis::engine::budget::MAX_BODY_BYTES;
use crate::syntax::{BlockTree, Expr, ExprKind, Lexer, TokenKind};
use std::ops::Range;
use std::path::Path;
use std::sync::Arc;

pub(crate) struct CursorBody {
    pub(crate) base: usize,
    pub(crate) tree: Arc<BlockTree>,
}

pub(super) struct ParsedBody<'a> {
    file: &'a Path,
    span: Range<usize>,
    tree: Arc<BlockTree>,
}

impl<'a> Semantics<'a> {
    pub(super) fn shared_body(
        &mut self,
        file: &'a Path,
        source: &str,
        span: Range<usize>,
    ) -> Option<Arc<BlockTree>> {
        if span.len() > MAX_BODY_BYTES {
            return None;
        }
        if let Some(body) = &self.parsed_body
            && body.file == file
            && body.span == span
        {
            return Some(Arc::clone(&body.tree));
        }
        self.parsed_body = None;
        let tree = Arc::new(crate::syntax::parse_block(source.get(span.clone())?));
        self.parsed_body = Some(ParsedBody {
            file,
            span,
            tree: Arc::clone(&tree),
        });
        Some(tree)
    }

    pub(crate) fn cursor_body(
        &mut self,
        file: &Path,
        source: &str,
        offset: usize,
        function: Option<(usize, Range<usize>)>,
    ) -> Result<Option<CursorBody>, (i32, String)> {
        let Some(file) = self.intern_file(file) else {
            return Ok(None);
        };
        let mut active_body = None;
        let mut literal = if let Some((start, body)) =
            function.filter(|(_, body)| contains_cursor(source, body, offset))
        {
            let body_offset = body.start.checked_sub(start).ok_or((
                crate::error::INVALID_PARAMS,
                "stale function boundary".into(),
            ))?;
            if body_offset > crate::analysis::engine::budget::MAX_BYTES {
                return Ok(None);
            }
            let _ = source
                .get(start..body.end)
                .ok_or((crate::error::INVALID_PARAMS, "stale function range".into()))?;
            let Some(parsed) = self.shared_body(file, source, body.clone()) else {
                return Ok(None);
            };
            let literal = enclosing_literal(source, &parsed.expressions, body.start, offset);
            active_body = Some(CursorBody {
                base: body.start,
                tree: parsed,
            });
            literal
        } else {
            let mut tokens = Lexer::new(source).peekable();
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
                return Ok(active_body);
            };
            drop(active_body.take());
            let body_offset = body.start - signature.start;
            if body_offset > crate::analysis::engine::budget::MAX_BYTES
                || body.len() > MAX_BODY_BYTES
            {
                return Ok(None);
            }
            let Some(text) = source.get(body.clone()) else {
                return Ok(None);
            };
            let parsed = Arc::new(crate::syntax::parse_block(text));
            literal = enclosing_literal(source, &parsed.expressions, body.start, offset);
            active_body = Some(CursorBody {
                base: body.start,
                tree: parsed,
            });
        }
        if literal.is_none() {
            Ok(active_body)
        } else {
            Err((
                crate::error::REQUEST_FAILED,
                "cursor query exceeds 128 nested function scopes".into(),
            ))
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
    for token in Lexer::new(source) {
        match token.kind {
            TokenKind::LBrace => braces += 1,
            TokenKind::RBrace => braces -= 1,
            _ => {}
        }
    }
    braces > 0
}
