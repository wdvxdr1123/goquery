//! Descriptions of predeclared identifiers, read from the pinned SDK builtin.go.
use super::content::HoverContent;
use super::doc;
use crate::analysis::query::Context;
use crate::analysis::query::builtin::{
    declaration as builtin_declaration, replace_placeholder_types,
};
use crate::syntax::{Lexer, TokenKind};

pub(super) fn hover(
    context: &Context<'_>,
    name: &str,
    range: super::SourceRange,
) -> Option<HoverContent> {
    let builtin = builtin_declaration(context, name)?;
    let file = builtin.file;
    let source = builtin.source;
    let symbol = builtin.symbol;
    let declaration = symbol.range.clone();
    let (code, doc_start) = match symbol.kind {
        crate::syntax::SymbolKind::Function => (
            source
                .get(declaration.clone())?
                .lines()
                .next()
                .map(replace_placeholder_types)?,
            declaration.start,
        ),
        crate::syntax::SymbolKind::Type => (
            format!(
                "type {}",
                source.get(declaration.start..declaration.end)?.trim_end()
            ),
            declaration_keyword(&source, declaration.start).unwrap_or(declaration.start),
        ),
        crate::syntax::SymbolKind::Variable | crate::syntax::SymbolKind::Constant => {
            let keyword = declaration_keyword(&source, declaration.start)?;
            let (text, _end) = value_declaration(&source, keyword, declaration.end, symbol.kind)?;
            (text, keyword)
        }
        _ => return None,
    };
    let mut content = HoverContent::new(range);
    content.code = Some(code);
    content.doc = attach(context, &file, &source, doc_start, declaration.end).map(|documents| {
        content.doc_links = documents.links;
        documents.text
    });
    content.link = Some(super::HoverLink {
        symbol: name.to_owned(),
        path: "builtin".to_owned(),
        anchor: name.to_owned(),
    });
    Some(content)
}

/// Declaration position of a predeclared identifier in the SDK builtin file.
pub(super) fn location(
    context: &Context<'_>,
    name: &str,
) -> Option<(std::path::PathBuf, (u32, u32))> {
    let builtin = builtin_declaration(context, name)?;
    let file = builtin.file;
    let source = builtin.source;
    let symbol = builtin.symbol;
    let position = crate::text::SourceMap::new(&source).position(symbol.selection.start);
    Some((file, (position.line, position.character)))
}

/// Rebuild the enclosing `const`/`var` declaration with go/printer alignment.
fn value_declaration(
    source: &str,
    keyword: usize,
    spec_end: usize,
    kind: crate::syntax::SymbolKind,
) -> Option<(String, usize)> {
    let keyword_text = if kind == crate::syntax::SymbolKind::Constant {
        "const"
    } else {
        "var"
    };
    let text = source.get(keyword..)?;
    let grouped = Lexer::new(text)
        .nth(1)
        .is_some_and(|token| token.kind == TokenKind::LParen);
    let end = text.find('\n').map_or(text.len(), |index| index) + keyword;
    if !grouped {
        let line = source.get(keyword..end)?.trim_end();
        let (body, comment) = split_comment(line);
        let body = body
            .strip_prefix(keyword_text)
            .map(str::trim_start)
            .filter(|body| !body.is_empty())?;
        let mut formatted = format!("{keyword_text} {body}");
        if let Some(comment) = comment {
            formatted.push('\t');
            formatted.push_str(comment);
        }
        return Some((formatted, end));
    }
    // Grouped declaration: format each value spec on its own line.
    let close = closing_paren(source, keyword)?;
    let body = source.get(keyword..=close)?;
    let mut lines = Vec::new();
    let mut inner = body
        .find('(')
        .and_then(|open| body.get(open + 1..body.len().saturating_sub(1)))?;
    if let Some(last) = body.rfind(')') {
        inner = body.get(body.find('(')? + 1..last)?;
    }
    for line in inner.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed == ")" {
            continue;
        }
        let (spec, comment) = split_comment(trimmed);
        let formatted = format_spec(spec)?;
        match comment {
            Some(comment) => lines.push(format!("\t{formatted}\t{comment}")),
            None => lines.push(format!("\t{formatted}")),
        }
    }
    let _ = spec_end;
    Some((format!("{keyword_text} (\n{}\n)", lines.join("\n")), close))
}

fn closing_paren(source: &str, keyword: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut started = false;
    for token in Lexer::new(source.get(keyword..)?) {
        match token.kind {
            TokenKind::LParen => {
                depth += 1;
                started = true;
            }
            TokenKind::RParen => {
                depth = depth.checked_sub(1)?;
                if started && depth == 0 {
                    return Some(keyword + token.span.end);
                }
            }
            _ => {}
        }
    }
    None
}

fn split_comment(line: &str) -> (&str, Option<&str>) {
    match line.find("//") {
        Some(index) => (line[..index].trim_end(), Some(line[index..].trim_end())),
        None => (line.trim_end(), None),
    }
}

/// Align a value spec as `names\t= values` (go/printer's single-line form).
fn format_spec(body: &str) -> Option<String> {
    let assign = Lexer::new(body).find(|token| token.kind == TokenKind::Assign)?;
    let names = body[..assign.span.start]
        .trim()
        .trim_end_matches(',')
        .trim_end();
    let values = body[assign.span.end..].trim();
    if names.is_empty() || values.is_empty() {
        return None;
    }
    Some(format!("{names}\t= {values}"))
}

fn declaration_keyword(source: &str, start: usize) -> Option<usize> {
    let mut keyword = None;
    for token in Lexer::new(source) {
        if token.span.start >= start {
            break;
        }
        if matches!(
            token.kind,
            TokenKind::Const | TokenKind::Var | TokenKind::Type | TokenKind::Func
        ) {
            keyword = Some(token.span.start);
        }
    }
    keyword
}

fn attach(
    context: &Context<'_>,
    file: &std::path::Path,
    source: &str,
    start: usize,
    end: usize,
) -> Option<doc::Documents> {
    doc::attached(
        context,
        file,
        source,
        doc::Target {
            start,
            end,
            declaration: None,
            trailing: false,
        },
    )
}
