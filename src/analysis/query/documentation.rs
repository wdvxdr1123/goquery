//! Bounded comment attachment and Go CommentGroup.Text shared by document features.
use super::Context;
use crate::syntax::{Lexer, TokenKind};
use std::ops::Range;

/// gopls only attaches assignment docs to single-name short declarations.
pub(crate) fn multi_name_short_declaration(source: &str, offset: usize) -> bool {
    let begin = source
        .get(..offset)
        .and_then(|prefix| prefix.rfind('\n'))
        .map_or(0, |index| index + 1);
    let end = source
        .get(offset..)
        .and_then(|rest| rest.find('\n'))
        .map_or(source.len(), |index| offset + index);
    let Some(line) = source.get(begin..end) else {
        return false;
    };
    let Some((before, _)) = line.split_once(":=") else {
        return false;
    };
    let lhs = before
        .trim()
        .strip_prefix("if ")
        .or_else(|| before.trim().strip_prefix("for "))
        .or_else(|| before.trim().strip_prefix("switch "))
        .unwrap_or(before.trim());
    lhs.matches(',').count() > 0
}

/// Whether `offset` is inside a function/method declaration header.
pub(crate) fn in_function_header(context: &Context<'_>, offset: usize) -> bool {
    context
        .workspace()
        .files
        .get(context.file())
        .is_some_and(|stub| {
            stub.symbols.iter().any(|symbol| {
                symbol
                    .body()
                    .zip(symbol.declaration())
                    .is_some_and(|(body, declaration)| {
                        declaration.start <= offset && offset < body.start
                    })
            })
        })
}

/// Whether the token sits in a control-statement header (`for x := ...`).
pub(crate) fn control_header_statement(source: &str, offset: usize) -> bool {
    let start = source[..offset].rfind('\n').map_or(0, |index| index + 1);
    let line = source[start..offset].trim_start();
    line.starts_with("for ")
}

pub(crate) fn declaration_keyword(source: &str, spec_start: usize) -> Option<usize> {
    let mut braces = 0usize;
    let mut keyword = None;
    for token in crate::syntax::Lexer::new(source) {
        if token.span.start >= spec_start {
            break;
        }
        match token.kind {
            TokenKind::LBrace => braces += 1,
            TokenKind::RBrace => braces = braces.saturating_sub(1),
            TokenKind::Func | TokenKind::Type | TokenKind::Var | TokenKind::Const
                if braces == 0 =>
            {
                keyword = Some(token.span.start);
            }
            _ => {}
        }
    }
    keyword
}

const MAX_COMMENT_BYTES: usize = 64 * 1024;

/// Start of the struct field declaration containing relative `selection`.
pub(crate) fn field_start(declaration: &str, selection: usize) -> Option<usize> {
    let prefix = declaration.get(..selection)?;
    let boundary = prefix.rfind(['{', '\n']).map_or(0, |index| index + 1);
    prefix
        .get(boundary..)?
        .char_indices()
        .find_map(|(index, ch)| (ch != ' ' && ch != '\t').then_some(boundary + index))
}

/// First non-blank token offset of the line containing `offset`.
pub(crate) fn line_start_token(source: &str, offset: usize) -> usize {
    let begin = source
        .get(..offset)
        .and_then(|prefix| prefix.rfind('\n'))
        .map_or(0, |index| index + 1);
    let Some(rest) = source.get(begin..offset) else {
        return offset;
    };
    match rest
        .char_indices()
        .find(|(_, ch)| *ch != ' ' && *ch != '\t')
    {
        Some((index, _)) => begin + index,
        None => offset,
    }
}

/// Where gopls looks for the documentation comment of a declaration.
pub(crate) struct Target {
    pub(crate) start: usize,
    pub(crate) end: usize,
    /// Start of the enclosing `var`/`const`/`type` declaration keyword.
    pub(crate) declaration: Option<usize>,
    pub(crate) trailing: bool,
}

pub(crate) fn attached(source: &str, target: Target) -> Option<String> {
    let spans = spans(source, target)?;
    text(source, &spans)
}

pub(crate) fn spans(source: &str, target: Target) -> Option<Vec<Range<usize>>> {
    group_before(source, target.start)
        .or_else(|| {
            target
                .declaration
                .and_then(|start| group_before(source, start))
        })
        .or_else(|| {
            target
                .trailing
                .then(|| group_after(source, target.end))
                .flatten()
        })
}

/// Committed comments immediately above `start`, requiring exactly one newline
/// between the group and the declaration (gopls' `ast.CommentGroup` attachment).
fn group_before(source: &str, start: usize) -> Option<Vec<Range<usize>>> {
    let mut group: Option<(Vec<Range<usize>>, bool)> = None;
    for token in Lexer::with_comments(source) {
        if token.inserted {
            continue;
        }
        if token.span.start >= start || token.span.end > start {
            let (spans, line_start) = group.take()?;
            if !line_start {
                return None;
            }
            let gap = &source[spans.last()?.end..start];
            return (gap.bytes().filter(|byte| *byte == b'\n').count() == 1).then_some(spans);
        }
        if token.kind == TokenKind::Comment {
            let line_begin = source[..token.span.start]
                .rfind('\n')
                .map_or(0, |index| index + 1);
            let line_start = source[line_begin..token.span.start]
                .bytes()
                .all(|byte| byte == b' ' || byte == b'\t');
            group = match group.take() {
                Some((mut spans, first_line_start)) => {
                    let previous = spans.last().map_or(0, |span| span.end);
                    if source[previous..token.span.start]
                        .bytes()
                        .filter(|byte| *byte == b'\n')
                        .count()
                        <= 1
                    {
                        spans.push(token.span);
                        Some((spans, first_line_start))
                    } else {
                        Some((vec![token.span], line_start))
                    }
                }
                None => Some((vec![token.span], line_start)),
            };
        } else {
            group = None;
        }
    }
    None
}

/// Trailing comment group on the same line after `end`, continuing over
/// adjacent comment lines.
fn group_after(source: &str, end: usize) -> Option<Vec<Range<usize>>> {
    let mut specs = Lexer::with_comments(source).skip_while(|token| token.span.start < end);
    let first = specs
        .next()
        .filter(|token| token.kind == TokenKind::Comment)?;
    if !source[end..first.span.start]
        .bytes()
        .all(|byte| byte == b' ' || byte == b'\t')
    {
        return None;
    }
    let mut spans = vec![first.span.clone()];
    for token in specs {
        if token.kind != TokenKind::Comment {
            break;
        }
        let previous = spans.last().expect("nonempty group").end;
        if source[previous..token.span.start]
            .bytes()
            .filter(|byte| *byte == b'\n')
            .count()
            > 1
        {
            break;
        }
        spans.push(token.span);
    }
    Some(spans)
}

/// Port of `go/ast.(*CommentGroup).Text` for already identified comments.
pub(crate) fn text(source: &str, spans: &[Range<usize>]) -> Option<String> {
    let mut lines: Vec<String> = Vec::new();
    for span in spans {
        let comment = source.get(span.clone())?;
        let mut content = if let Some(text) = comment.strip_prefix("//") {
            if let Some(text) = text.strip_prefix(' ') {
                text
            } else if directive(text) {
                continue;
            } else {
                text
            }
        } else {
            comment.strip_prefix("/*")?.strip_suffix("*/")?
        };
        if content.is_empty() && comment.len() == 2 {
            content = "";
        }
        for line in content.split('\n') {
            lines.push(line.trim_end().to_owned());
        }
    }
    let mut kept: Vec<String> = Vec::new();
    for line in lines {
        if !line.is_empty() || kept.last().is_some_and(|last| !last.is_empty()) {
            kept.push(line);
        }
    }
    if kept.last().is_some_and(|last| !last.is_empty()) {
        kept.push(String::new());
    }
    let text = kept.join("\n");
    (!text.is_empty() && text.len() <= MAX_COMMENT_BYTES).then_some(text)
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
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        && suffix
            .as_bytes()
            .first()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}
