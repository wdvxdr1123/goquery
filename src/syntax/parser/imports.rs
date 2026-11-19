//! Shared import-spec grammar for the stub and full-file parsers.
use crate::syntax::{Lexer, Token, TokenKind};
use std::iter::Peekable;
use std::ops::Range;

pub(crate) struct ImportSpec {
    pub(crate) alias: Option<Range<usize>>,
    pub(crate) path: Range<usize>,
}

pub(super) fn import_spec(tokens: &mut Peekable<Lexer<'_>>) -> Option<ImportSpec> {
    let alias = if matches!(tokens.peek()?.kind, TokenKind::Ident | TokenKind::Period) {
        tokens.next().map(|token| token.span)
    } else {
        None
    };
    if tokens.peek()?.kind != TokenKind::String {
        return None;
    }
    let path = tokens.next()?.span;
    Some(ImportSpec { alias, path })
}

/// Visit syntactic import spans without walking declarations after the last
/// possible `import` token. The textual bound cannot exclude a lexer match,
/// including a misplaced declaration in a malformed file.
pub(crate) fn visit_import_spans(source: &str, mut visit: impl FnMut(ImportSpec)) {
    let Some(last) = source.rfind("import") else {
        return;
    };
    let mut tokens = Lexer::new(source).peekable();
    while let Some(token) = tokens.next() {
        if token.span.start > last {
            break;
        }
        if token.kind != TokenKind::Import {
            continue;
        }
        if tokens
            .peek()
            .is_some_and(|next| next.kind == TokenKind::LParen)
        {
            tokens.next();
            while let Some(next) = tokens.peek() {
                if next.kind == TokenKind::RParen {
                    tokens.next();
                    break;
                }
                if let Some(spec) = import_spec(&mut tokens) {
                    visit(spec);
                } else if tokens
                    .peek()
                    .is_none_or(|next| next.kind != TokenKind::RParen)
                {
                    tokens.next();
                }
            }
        } else if let Some(spec) = import_spec(&mut tokens) {
            visit(spec);
        }
        if token.span.start == last {
            break;
        }
    }
}

/// Identifiers immediately followed by a period. This is a necessary spelling
/// for a qualified import use, and may also refer to a shadowing local.
pub(crate) fn qualified_prefixes(source: &str) -> impl Iterator<Item = &str> {
    let mut previous: Option<Token> = None;
    Lexer::new(source).filter_map(move |token| {
        let period = token.kind == TokenKind::Period;
        let previous = previous.replace(token)?;
        (period && previous.kind == TokenKind::Ident).then(|| &source[previous.span])
    })
}
