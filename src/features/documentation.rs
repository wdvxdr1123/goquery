//! Query-local comment attachments and shared Go documentation rendering.
mod markdown;
pub(crate) use markdown::markdown;

use crate::syntax::{Lexer, TokenKind};
use std::ops::Range;

const MAX_COMMENT_BYTES: usize = 16 * 1024;

pub(crate) struct Target {
    pub start: usize,
    pub end: usize,
    pub trailing: bool,
}

/// Scan once per source, retaining only comment spans for selected declarations.
pub(crate) fn comments(source: &str, targets: &[Target], mut budget: usize) -> Vec<Option<String>> {
    let mut attached: Vec<Option<Range<usize>>> = vec![None; targets.len()];
    let mut starts: Vec<_> = targets
        .iter()
        .enumerate()
        .map(|(i, t)| (t.start, i))
        .collect();
    starts.sort_unstable();
    let mut ends: Vec<_> = targets
        .iter()
        .enumerate()
        .filter(|(_, t)| t.trailing)
        .map(|(i, t)| (t.end, i))
        .collect();
    ends.sort_unstable();
    let mut pending: Option<Range<usize>> = None;
    let mut previous_end = 0;
    let mut keyword = None;
    let mut groups = Vec::new();
    let mut depth = (0usize, 0usize, 0usize);
    for token in Lexer::with_comments(source) {
        if token.inserted {
            continue;
        }
        let span = token.span;
        if token.kind == TokenKind::Comment {
            let trailing = previous_end > 0 && !source[previous_end..span.start].contains('\n');
            if trailing {
                let line = source[..span.start].rfind('\n').map_or(0, |i| i + 1);
                let begin = ends.partition_point(|(end, _)| *end < line);
                for &(end, index) in &ends[begin..] {
                    if end > span.start {
                        break;
                    }
                    if let Some(old) = &mut attached[index] {
                        if old.start >= end && source[old.end..span.start].trim().is_empty() {
                            old.end = span.end;
                        }
                    } else if source[end..span.start].trim().is_empty() {
                        attached[index] = Some(span.clone());
                    }
                }
            } else if let Some(old) = &mut pending {
                if source[old.end..span.start]
                    .bytes()
                    .filter(|b| *b == b'\n')
                    .count()
                    <= 1
                {
                    old.end = span.end;
                } else {
                    *old = span;
                }
            } else {
                pending = Some(span);
            }
            continue;
        }
        let leading = pending.take().filter(|comment| {
            source[comment.end..span.start]
                .bytes()
                .filter(|b| *b == b'\n')
                .count()
                == 1
        });
        let inherited = keyword.take();
        let fallback = groups
            .last()
            .filter(|(at, _)| *at == depth)
            .and_then(|(_, comment): &(_, Option<Range<usize>>)| comment.clone());
        let doc = leading.or_else(|| inherited.clone().flatten()).or(fallback);
        let begin = starts.partition_point(|(start, _)| *start < span.start);
        for &(start, index) in &starts[begin..] {
            if start != span.start {
                break;
            }
            attached[index] = doc.clone();
        }
        match token.kind {
            TokenKind::Var | TokenKind::Const | TokenKind::Type => keyword = Some(doc),
            TokenKind::LParen => {
                depth.0 += 1;
                if let Some(comment) = inherited {
                    // Bound malformed input without retaining any token stream.
                    if groups.len() < 128 {
                        groups.push((depth, comment));
                    }
                }
            }
            TokenKind::RParen => {
                if groups.last().is_some_and(|(at, _)| *at == depth) {
                    groups.pop();
                }
                depth.0 = depth.0.saturating_sub(1);
            }
            TokenKind::LBrack => depth.1 += 1,
            TokenKind::RBrack => depth.1 = depth.1.saturating_sub(1),
            TokenKind::LBrace => depth.2 += 1,
            TokenKind::RBrace => depth.2 = depth.2.saturating_sub(1),
            _ => {}
        }
        previous_end = span.end;
    }
    attached
        .into_iter()
        .map(|span| {
            let span = span.filter(|span| span.len() <= MAX_COMMENT_BYTES)?;
            if span.len() > budget {
                return None;
            }
            budget -= span.len();
            let mut text = String::new();
            let source = &source[span];
            for token in Lexer::with_comments(source) {
                if token.kind != TokenKind::Comment {
                    continue;
                }
                let comment = &source[token.span];
                let content = if let Some(line) = comment.strip_prefix("//") {
                    if let Some(line) = line.strip_prefix(' ') {
                        line
                    } else if directive(line) {
                        continue;
                    } else {
                        line
                    }
                } else {
                    comment.strip_prefix("/*")?.strip_suffix("*/")?
                };
                for line in content.split('\n') {
                    let line = line.trim_end();
                    if line.is_empty() && (text.is_empty() || text.ends_with("\n\n")) {
                        continue;
                    }
                    text.push_str(line);
                    text.push('\n');
                }
            }
            if text.ends_with("\n\n") {
                text.pop();
            }
            (!text.is_empty()).then_some(text)
        })
        .collect()
}

fn directive(line: &str) -> bool {
    if ["line ", "extern ", "export "]
        .iter()
        .any(|prefix| line.starts_with(prefix))
    {
        return true;
    }
    let Some((prefix, suffix)) = line.split_once(':') else {
        return false;
    };
    !prefix.is_empty()
        && prefix
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && suffix
            .as_bytes()
            .first()
            .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}
