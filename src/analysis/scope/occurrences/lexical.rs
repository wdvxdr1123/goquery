//! Fast lexical prefilters, skipping unrelated indexed bodies before tokenization.
use crate::index::IndexedFile;
use crate::syntax::{Lexer, TokenKind};

/// Fast lexical scan returning all identifier spans matching `name`, skipping
/// indexed function bodies whose text does not contain `name`.
pub(crate) fn scan_name(
    source: &str,
    stub: &IndexedFile,
    name: &str,
) -> Vec<std::ops::Range<usize>> {
    scan_with(source, stub, name, None)
}

/// Qualified candidate scan returning spans where `name` is preceded by `qual.`
/// for one of `qualifiers`.
pub(crate) fn scan_qualified_name(
    source: &str,
    stub: &IndexedFile,
    name: &str,
    qualifiers: &[&str],
) -> Vec<std::ops::Range<usize>> {
    scan_with(source, stub, name, Some(qualifiers))
}

fn scan_with(
    source: &str,
    stub: &IndexedFile,
    name: &str,
    qualifiers: Option<&[&str]>,
) -> Vec<std::ops::Range<usize>> {
    let mut regions = Vec::new();
    let mut start = 0;
    for body in stub.symbols.iter().filter_map(|symbol| symbol.body()) {
        if body.start >= start
            && source
                .get(body.clone())
                .is_some_and(|body| !body.contains(name))
        {
            regions.push(start..body.start);
            start = body.end;
        }
    }
    regions.push(start..source.len());
    let mut spans = Vec::new();
    let mut previous = TokenKind::Illegal;
    let mut identifier = None;
    let mut qualifier = None;
    for region in regions {
        for token in Lexer::new(&source[region.clone()]) {
            let span = region.start + token.span.start..region.start + token.span.end;
            let spelling = &source[span.clone()];
            if token.kind == TokenKind::Ident
                && spelling == name
                && qualifiers.is_none_or(|aliases| {
                    previous == TokenKind::Period && qualifier.is_some_and(|q| aliases.contains(&q))
                })
            {
                spans.push(span);
            }
            qualifier = if token.kind == TokenKind::Period {
                identifier.take()
            } else {
                None
            };
            identifier = (token.kind == TokenKind::Ident).then_some(spelling);
            previous = token.kind;
        }
    }
    spans
}

/// The identifier whose token contains the cursor; the right-hand identifier
/// wins at a token boundary, matching point selection in the reference server.
pub(crate) fn identifier_at(source: &str, offset: usize) -> Option<std::ops::Range<usize>> {
    let mut found = None;
    for token in Lexer::new(source) {
        if token.span.start > offset {
            break;
        }
        if token.kind == TokenKind::Ident && offset <= token.span.end {
            found = Some(token.span);
        }
    }
    found
}
