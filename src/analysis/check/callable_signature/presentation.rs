//! Bounded, literal-safe expansion of syntax-declared signature names.
use crate::analysis::query::budget::{MAX_BYTES, MAX_DEPTH, MAX_NODES, RELATION_STEPS};
use crate::syntax::{Lexer, TokenKind};
use std::ops::Range;

struct Insertion {
    offset: usize,
    end: usize,
    annotation: Option<Range<usize>>,
    prefix: &'static str,
}

pub(crate) fn function_types(text: &str) -> Option<String> {
    if text.len() > MAX_BYTES {
        return None;
    }
    if !Lexer::new(text).any(|token| matches!(token.kind, TokenKind::Func | TokenKind::Interface)) {
        return Some(text.to_owned());
    }
    let source = strip_comments(text);
    let mut insertions = Vec::new();
    let mut covered = 0;
    let mut nodes = 0usize;
    for token in Lexer::new(&source) {
        if token.span.start < covered
            || !matches!(token.kind, TokenKind::Func | TokenKind::Interface)
        {
            continue;
        }
        let base = token.span.start;
        let syntax = crate::syntax::parse_type_prefix(&source[base..]);
        if !syntax.diagnostics().is_empty() {
            continue;
        }
        nodes = nodes.checked_add(syntax.type_nodes().len() + syntax.signature_names.len())?;
        if nodes > MAX_NODES {
            return None;
        }
        covered = base + syntax.span.end;
        let mut names = syntax.signature_names;
        names.sort_unstable_by_key(|name| {
            (name.annotation.start, name.annotation.end, name.name.start)
        });
        for (index, name) in names.iter().enumerate() {
            if names
                .get(index + 1)
                .is_some_and(|next| next.annotation == name.annotation)
            {
                insertions.push(Insertion {
                    offset: base + name.name.end,
                    end: base + name.name.end,
                    annotation: Some(base + name.annotation.start..base + name.annotation.end),
                    prefix: " ",
                });
            } else {
                let gap = base + name.name.end..base + name.annotation.start;
                let prefix = if name.variadic { " ..." } else { " " };
                if source.get(gap.clone())? != prefix {
                    insertions.push(Insertion {
                        offset: gap.start,
                        end: gap.end,
                        annotation: None,
                        prefix,
                    });
                }
            }
        }
    }
    insertions.sort_unstable_by_key(|edit| edit.offset);
    let mut renderer = Renderer {
        source: &source,
        insertions: &insertions,
        output: String::new(),
        steps: RELATION_STEPS,
    };
    renderer.fragment(0..source.len(), 0)?;
    canonical_separators(&canonical_literals(&renderer.output)?)
}

struct Renderer<'a> {
    source: &'a str,
    insertions: &'a [Insertion],
    output: String,
    steps: usize,
}

impl Renderer<'_> {
    fn push(&mut self, text: &str) -> Option<()> {
        if self.output.len().checked_add(text.len())? > MAX_BYTES {
            return None;
        }
        self.output.push_str(text);
        Some(())
    }

    fn fragment(&mut self, range: Range<usize>, depth: usize) -> Option<()> {
        if depth >= MAX_DEPTH {
            return None;
        }
        let mut start = range.start;
        let first = self.insertions.partition_point(|edit| edit.offset < start);
        let last = self
            .insertions
            .partition_point(|edit| edit.offset < range.end);
        for index in first..last {
            self.steps = self.steps.checked_sub(1)?;
            let insertion = &self.insertions[index];
            let offset = insertion.offset;
            let end = insertion.end;
            let prefix = insertion.prefix;
            let annotation = insertion.annotation.clone();
            self.push(self.source.get(start..offset)?)?;
            self.push(prefix)?;
            if let Some(annotation) = annotation {
                self.fragment(annotation, depth + 1)?;
            }
            start = end;
        }
        self.push(self.source.get(start..range.end)?)
    }
}

fn canonical_separators(text: &str) -> Option<String> {
    let mut output = String::new();
    let mut start = 0;
    let mut previous = None;
    for token in Lexer::new(text) {
        let result_list = token.kind == TokenKind::LParen && previous == Some(TokenKind::RParen);
        previous = Some(token.kind);
        if token.kind != TokenKind::Comma && !result_list {
            continue;
        }
        let prefix = text.get(start..token.span.start)?.trim_end();
        if output.len().checked_add(prefix.len() + 2)? > MAX_BYTES {
            return None;
        }
        output.push_str(prefix);
        if result_list {
            output.push(' ');
            start = token.span.start;
            continue;
        }
        output.push_str(", ");
        let rest = &text[token.span.end..];
        start = token.span.end + rest.len() - rest.trim_start().len();
    }
    if output.len().checked_add(text.len() - start)? > MAX_BYTES {
        return None;
    }
    output.push_str(&text[start..]);
    Some(output)
}

fn canonical_literals(text: &str) -> Option<String> {
    let mut output = String::new();
    let mut start = 0;
    for token in Lexer::new(text).filter(|token| token.kind == TokenKind::String) {
        let bytes = crate::syntax::decode_string_literal(text.get(token.span.clone())?)?;
        let quoted = crate::syntax::quote_bytes(&bytes);
        let prefix = text.get(start..token.span.start)?;
        if output.len().checked_add(prefix.len() + quoted.len())? > MAX_BYTES {
            return None;
        }
        output.push_str(prefix);
        output.push_str(&quoted);
        start = token.span.end;
    }
    if output.len().checked_add(text.len() - start)? > MAX_BYTES {
        return None;
    }
    output.push_str(&text[start..]);
    Some(output)
}

/// Remove comments without interpreting literal contents.
pub(crate) fn strip_comments(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut start = 0;
    for token in Lexer::with_comments(text).filter(|token| token.kind == TokenKind::Comment) {
        output.push_str(&text[start..token.span.start]);
        output.push(if text[token.span.clone()].contains('\n') {
            '\n'
        } else {
            ' '
        });
        start = token.span.end;
    }
    output.push_str(&text[start..]);
    output
}
