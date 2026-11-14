//! Bounded syntax-based declaration presentation; literal bytes are never tokens
//! to rewrite. Multiline source layout and attached comments remain source-owned.
mod expression;
use crate::analysis::query::budget::{MAX_BYTES, MAX_DEPTH, MAX_NODES};
use crate::syntax::{ExprKind, Lexer, Token, TokenKind, TypeNodeKind, TypeSyntax};
use std::borrow::Cow;
use std::ops::Range;
/// Preserve source layout and attached comments without interpreting literal lines.
fn attached_comments(text: &str) -> String {
    let text = text.replace("\r\n", "\n").replace('\r', "");
    let lines: Vec<&str> = text.split('\n').collect();
    let mut output: Vec<&str> = Vec::with_capacity(lines.len());
    let mut index = 0usize;
    let mut offset = 0usize;
    let offsets: Vec<_> = lines
        .iter()
        .map(|line| {
            let start = offset;
            offset += line.len() + 1;
            start
        })
        .collect();
    let mut literals = Lexer::new(&text)
        .filter(|token| matches!(token.kind, TokenKind::String | TokenKind::Char))
        .peekable();
    while index < lines.len() {
        let offset = offsets[index];
        while literals
            .peek()
            .is_some_and(|token| token.span.end <= offset)
        {
            literals.next();
        }
        if literals
            .peek()
            .is_some_and(|token| token.span.start < offset && offset < token.span.end)
        {
            output.push(lines[index]);
            index += 1;
            continue;
        }
        let trimmed = lines[index].trim_start();
        if trimmed.starts_with("//") {
            let mut end = index;
            while end < lines.len() && lines[end].trim_start().starts_with("//") {
                end += 1;
            }
            if !comment_attached(&lines, end) {
                index = end;
                detached_comment_gap(&mut output, &lines[end..]);
                while index < lines.len() && lines[index].trim().is_empty() {
                    index += 1;
                }
                continue;
            }
            output.extend_from_slice(&lines[index..end]);
            index = end;
            continue;
        }
        if trimmed.starts_with("/*") {
            let mut end = index;
            while end < lines.len() {
                let closes = lines[end].contains("*/");
                end += 1;
                if closes {
                    break;
                }
            }
            if !comment_attached(&lines, end) {
                index = end;
                detached_comment_gap(&mut output, &lines[end..]);
                while index < lines.len() && lines[index].trim().is_empty() {
                    index += 1;
                }
                continue;
            }
            output.extend_from_slice(&lines[index..end]);
            index = end;
            continue;
        }
        output.push(lines[index]);
        index += 1;
    }
    output.join("\n")
}

/// Maintain separation after omitting unattached comment groups.
fn detached_comment_gap(output: &mut Vec<&str>, remaining: &[&str]) {
    // Removing a trailing, unattached comment must not leave a phantom field
    // separator before the aggregate's closing brace (e.g. reflect.Value).
    if remaining
        .iter()
        .find(|line| !line.trim().is_empty())
        .is_some_and(|line| line.trim_start().starts_with('}'))
    {
        while output.last().is_some_and(|line| line.trim().is_empty()) {
            output.pop();
        }
    } else if output
        .last()
        .is_some_and(|line| !line.trim().is_empty() && !line.trim_end().ends_with('{'))
    {
        output.push("");
    }
}

/// A comment group is printed only when it documents the following node.
fn comment_attached(lines: &[&str], next: usize) -> bool {
    let Some(line) = lines.get(next) else {
        return false;
    };
    let trimmed = line.trim();
    !trimmed.is_empty() && !trimmed.starts_with('}')
}
pub(super) fn format_spec(text: &str) -> Cow<'_, str> {
    // Alias targets may be larger than the primary declaration admission cap.
    // Keep their source rather than allocating comment/line indexes for them.
    if text.len() > 256 * 1024 {
        return Cow::Borrowed(text);
    }
    let filtered = attached_comments(text);
    Cow::Owned(format_inline_spec(&filtered).unwrap_or(filtered))
}

fn format_inline_spec(text: &str) -> Option<String> {
    if text.len() > MAX_BYTES || !single_source_line(text) {
        return None;
    }
    let name = Lexer::new(text).next()?;
    if name.kind != TokenKind::Ident {
        return None;
    }
    let mut prefix = text.get(name.span.clone())?.to_owned();
    let mut offset = name.span.end;
    if let Some(end) = crate::syntax::type_parameter_end(&text[offset..]) {
        let header = format!("func _{}()", &text[offset..offset + end]);
        let signature = crate::syntax::parse_signature(&header);
        if !signature.diagnostics.is_empty() {
            return None;
        }
        prefix.push('[');
        for (index, parameter) in signature.type_parameters.iter().enumerate() {
            if index > 0 {
                prefix.push_str(", ");
            }
            prefix.push_str(header.get(parameter.name.as_ref()?.clone())?);
            let grouped = signature
                .type_parameters
                .get(index + 1)
                .is_some_and(|next| next.annotation == parameter.annotation);
            if !grouped {
                prefix.push(' ');
                let annotation = header.get(parameter.annotation.clone())?;
                let rendered = if let Some(mut printer) = Printer::new(annotation) {
                    printer.fragment(0..annotation.len(), 0)?
                } else {
                    // A standalone type term is parsed in constraint position.
                    let wrapped = format!("interface{{{annotation}}}");
                    Printer::new(&wrapped)?.fragment(10..wrapped.len() - 1, 0)?
                };
                if prefix.len().checked_add(rendered.len())? > MAX_BYTES * 4 {
                    return None;
                }
                prefix.push_str(&rendered);
            }
            if prefix.len() > MAX_BYTES * 4 {
                return None;
            }
        }
        prefix.push(']');
        offset += end;
    }
    let next = Lexer::new(&text[offset..]).next()?;
    if next.kind == TokenKind::Assign {
        prefix.push_str(" =");
        offset += next.span.end;
    }
    let next = Lexer::new(&text[offset..]).next()?;
    offset += next.span.start;
    let annotation = text.get(offset..)?;
    let rendered = Printer::new(annotation)?.fragment(0..annotation.len(), 0)?;
    if prefix.len().checked_add(rendered.len() + 1)? > MAX_BYTES * 4 {
        return None;
    }
    prefix.push(' ');
    prefix.push_str(&rendered);
    Some(prefix)
}

/// Newlines inside raw literals are values, rather than layout gaps. Keeping
/// existing multiline declarations avoids discarding field-comment placement.
fn single_source_line(text: &str) -> bool {
    let mut end = 0;
    for token in Lexer::with_comments(text) {
        if token.kind == TokenKind::Comment && text[token.span.clone()].contains('\n') {
            return false;
        }
        if text
            .get(end..token.span.start)
            .is_none_or(|gap| gap.contains('\n'))
        {
            return false;
        }
        end = token.span.end;
    }
    !text[end..].contains('\n')
}

struct Printer<'a> {
    text: &'a str,
    syntax: TypeSyntax,
    tokens: Vec<Token>,
    binary: Vec<(usize, bool)>,
    arrays: Vec<usize>,
    type_parens: Vec<usize>,
    calls: Vec<usize>,
    steps: usize,
}

struct FieldGroup {
    annotation: Range<usize>,
    names: Vec<Range<usize>>,
    tag: Option<Range<usize>>,
}

/// Elastic field columns use rune widths and discard empty soft columns, like
/// go/printer's tabwriter. Values remain atomic strings, not split on tag bytes.
fn align_fields(rows: Vec<Vec<String>>) -> Option<Vec<String>> {
    fn widths(rows: &[Vec<String>], padding: &mut [[usize; 3]], column: usize) {
        if column >= 3 {
            return;
        }
        let mut start = 0;
        while start < rows.len() {
            if rows[start].len() <= column + 1 {
                start += 1;
                continue;
            }
            let end = start
                + rows[start..]
                    .iter()
                    .take_while(|row| row.len() > column + 1)
                    .count();
            let max = rows[start..end]
                .iter()
                .map(|row| row[column].chars().count())
                .max()
                .unwrap_or(0);
            let width = if max == 0 { 0 } else { max + 1 };
            for (row, padding) in rows[start..end].iter().zip(&mut padding[start..end]) {
                padding[column] = width.saturating_sub(row[column].chars().count());
            }
            widths(&rows[start..end], &mut padding[start..end], column + 1);
            start = end;
        }
    }
    let mut padding = vec![[0; 3]; rows.len()];
    widths(&rows, &mut padding, 0);
    let mut bytes = 0usize;
    rows.into_iter()
        .zip(padding)
        .map(|(row, padding)| {
            let width = row.iter().map(String::len).sum::<usize>()
                + padding
                    .iter()
                    .take(row.len().saturating_sub(1))
                    .sum::<usize>();
            bytes = bytes.checked_add(width)?;
            if bytes > MAX_BYTES * 4 {
                return None;
            }
            let mut out = String::new();
            let count = row.len();
            for (index, cell) in row.into_iter().enumerate() {
                out.push_str(&cell);
                if index + 1 < count {
                    out.extend(std::iter::repeat_n(' ', padding[index]));
                }
            }
            Some(out)
        })
        .collect()
}

impl<'a> Printer<'a> {
    fn new(text: &'a str) -> Option<Self> {
        if text.len() > MAX_BYTES {
            return None;
        }
        let syntax = crate::syntax::parse_type(text);
        if !syntax.diagnostics.is_empty() {
            return None;
        }
        let tokens: Vec<_> = Lexer::new(text)
            .filter(|token| !token.span.is_empty())
            .collect();
        if tokens.len() > MAX_NODES {
            return None;
        }
        let binary = expression::spaces(text, syntax.expressions())?;
        let mut calls = Vec::new();
        for expression in syntax.expressions() {
            match &expression.kind {
                ExprKind::Call { function, .. } => {
                    let end = syntax.expressions()[function.0].span.end;
                    let index = tokens.partition_point(|token| token.span.start < end);
                    if let Some(token) = tokens.get(index).filter(|t| t.kind == TokenKind::LParen) {
                        calls.push(token.span.start);
                    }
                }
                ExprKind::FunctionLiteral { .. } => return None,
                _ => {}
            }
        }
        calls.sort_unstable();
        let mut arrays: Vec<_> = syntax
            .type_nodes()
            .iter()
            .filter(|node| matches!(node.kind, TypeNodeKind::Array | TypeNodeKind::Slice))
            .map(|node| node.span.start)
            .collect();
        arrays.sort_unstable();
        let mut type_parens: Vec<_> = syntax
            .type_nodes()
            .iter()
            .filter(|node| node.kind == TypeNodeKind::Parenthesized)
            .map(|node| node.span.start)
            .collect();
        type_parens.sort_unstable();
        Some(Self {
            text,
            syntax,
            tokens,
            binary,
            arrays,
            type_parens,
            calls,
            steps: MAX_NODES,
        })
    }

    fn fragment(&mut self, span: Range<usize>, depth: usize) -> Option<String> {
        if depth >= MAX_DEPTH {
            return None;
        }
        let mut out = String::new();
        let mut previous = None;
        let mut index = self
            .tokens
            .partition_point(|token| token.span.start < span.start);
        while index < self.tokens.len() && self.tokens[index].span.start < span.end {
            self.steps = self.steps.checked_sub(1)?;
            let token = self.tokens[index].clone();
            if token.kind == TokenKind::Semicolon {
                index += 1;
                continue;
            }
            if previous.is_some_and(|prev| self.space(prev, index)) {
                out.push(' ');
            }
            let aggregate = matches!(token.kind, TokenKind::Struct | TokenKind::Interface)
                .then(|| {
                    self.syntax
                        .type_nodes()
                        .iter()
                        .find(|node| {
                            node.span.start == token.span.start
                                && matches!(
                                    node.kind,
                                    TypeNodeKind::Struct | TypeNodeKind::Interface
                                )
                        })
                        .map(|node| node.span.clone())
                })
                .flatten();
            if let Some(aggregate) = aggregate {
                out.push_str(&self.aggregate(aggregate.clone(), token.kind, depth)?);
                index = self
                    .tokens
                    .partition_point(|token| token.span.start < aggregate.end);
                previous = index.checked_sub(1);
            } else {
                out.push_str(self.text.get(token.span.clone())?);
                previous = Some(index);
                index += 1;
            }
            if out.len() > MAX_BYTES * 4 {
                return None;
            }
        }
        Some(out)
    }

    fn space(&self, previous: usize, index: usize) -> bool {
        use TokenKind::*;
        let a = &self.tokens[previous];
        let b = &self.tokens[index];
        if matches!(
            b.kind,
            RParen | RBrack | RBrace | Comma | Period | Semicolon | Colon
        ) {
            return false;
        }
        if matches!(a.kind, LParen | LBrack | LBrace | Period) {
            return false;
        }
        if let Ok(index) = self
            .binary
            .binary_search_by_key(&a.span.start, |entry| entry.0)
        {
            return self.binary[index].1;
        }
        if let Ok(index) = self
            .binary
            .binary_search_by_key(&b.span.start, |entry| entry.0)
        {
            return self.binary[index].1;
        }
        if a.kind == Or || b.kind == Or || a.kind == Assign || b.kind == Assign {
            return true;
        }
        if matches!(a.kind, Mul | Tilde | Ellipsis | Not | Add | Sub | Xor) {
            return false;
        }
        if a.kind == Arrow {
            return previous > 0 && self.tokens[previous - 1].kind == Chan;
        }
        if b.kind == Arrow && a.kind == Chan {
            return false;
        }
        if b.kind == LParen {
            if self.type_parens.binary_search(&b.span.start).is_ok() {
                return a.kind != RBrack;
            }
            return a.kind == RParen && self.calls.binary_search(&b.span.start).is_err();
        }
        if b.kind == LBrack {
            return self.arrays.binary_search(&b.span.start).is_ok()
                && !matches!(a.kind, Map | RBrack);
        }
        if matches!(a.kind, RBrack) || b.kind == LBrace {
            return false;
        }
        true
    }

    fn aggregate(&mut self, span: Range<usize>, kind: TokenKind, depth: usize) -> Option<String> {
        let brace = self
            .tokens
            .iter()
            .find(|token| {
                token.span.start > span.start
                    && token.span.end <= span.end
                    && token.kind == TokenKind::LBrace
            })?
            .span
            .start;
        let mut fields: Vec<FieldGroup> = Vec::new();
        if kind == TokenKind::Struct {
            for field in self
                .syntax
                .fields
                .iter()
                .filter(|field| field.container == brace)
            {
                if !field.embedded
                    && fields.last().is_some_and(|group| {
                        group.annotation == field.annotation && group.tag == field.tag
                    })
                {
                    fields.last_mut()?.names.push(field.name.clone());
                } else {
                    fields.push(FieldGroup {
                        annotation: field.annotation.clone(),
                        names: if field.embedded {
                            Vec::new()
                        } else {
                            vec![field.name.clone()]
                        },
                        tag: field.tag.clone(),
                    });
                }
            }
        } else {
            fields.extend(
                self.syntax
                    .interface_members
                    .iter()
                    .filter(|member| member.container == brace)
                    .map(|member| FieldGroup {
                        annotation: member.annotation.clone(),
                        names: member.name.clone().into_iter().collect(),
                        tag: None,
                    }),
            );
        }
        let align = kind == TokenKind::Struct && fields.len() > 1;
        let mut rows = Vec::new();
        let mut type_size = 0;
        let mut has_tag = false;
        for FieldGroup {
            annotation,
            names,
            tag,
        } in fields
        {
            let mut row = names
                .iter()
                .map(|name| self.text.get(name.clone()))
                .collect::<Option<Vec<_>>>()?
                .join(", ");
            let ty = self.fragment(annotation, depth + 1)?;
            type_size = if ty.contains('\n') {
                usize::MAX
            } else {
                ty.len()
                    + usize::from(!names.is_empty())
                    + if kind == TokenKind::Interface && !names.is_empty() {
                        4
                    } else {
                        0
                    }
            };
            if align {
                // Complex source-position-driven multiline groups retain their
                // source layout. Literal newlines must never become cell gaps.
                if ty.contains('\n')
                    || tag
                        .as_ref()
                        .is_some_and(|tag| self.text[tag.clone()].contains('\n'))
                {
                    return None;
                }
                let named = !row.is_empty();
                let mut cells = if named { vec![row, ty] } else { vec![ty] };
                if let Some(tag) = tag {
                    has_tag = true;
                    if named {
                        cells.push(String::new());
                    }
                    cells.push(self.text.get(tag)?.to_owned());
                }
                rows.push(cells);
                continue;
            }
            if !row.is_empty() && kind == TokenKind::Struct {
                row.push(' ');
            }
            row.push_str(&ty);
            if let Some(tag) = tag {
                has_tag = true;
                row.push(' ');
                row.push_str(self.text.get(tag)?);
            }
            rows.push(vec![row]);
        }
        let rows = align_fields(rows)?;
        let keyword = if kind == TokenKind::Struct {
            "struct"
        } else {
            "interface"
        };
        let inline = !self.text.get(span.clone())?.contains('\n')
            && !has_tag
            && (rows.is_empty() || rows.len() == 1 && type_size <= 30);
        if inline {
            return Some(if rows.is_empty() {
                format!("{keyword}{{}}")
            } else {
                format!("{keyword}{{ {} }}", rows[0])
            });
        }
        let indentation = "\t".repeat(depth + 1);
        let mut out = format!("{keyword} {{");
        for row in rows {
            out.push('\n');
            out.push_str(&indentation);
            out.push_str(&row);
            if out.len() > MAX_BYTES * 4 {
                return None;
            }
        }
        out.push('\n');
        out.push_str(&"\t".repeat(depth));
        out.push('}');
        Some(out)
    }
}
