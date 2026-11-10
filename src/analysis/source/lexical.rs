//! Lexical occurrences, skipping unrelated indexed bodies before tokenization.
use crate::analysis::source::{Name, Role, SourceAnalysis};
use crate::index::IndexedFile;
use crate::syntax::{Lexer, TokenKind};

impl SourceAnalysis {
    pub(super) fn collect_names(
        source: &str,
        stub: &IndexedFile,
        name: Option<&str>,
        qualifiers: Option<&[&str]>,
    ) -> (Vec<Name>, bool) {
        let mut regions = Vec::new();
        let mut start = 0;
        if let Some(name) = name {
            for body in stub
                .symbols
                .iter()
                .filter_map(|symbol| symbol.body.as_ref())
            {
                // Indexed body ranges are opaque balanced blocks. Header tokens
                // remain visible, including receiver names and type parameters.
                if body.start >= start
                    && source
                        .get(body.clone())
                        .is_some_and(|body| !body.contains(name))
                {
                    regions.push(start..body.start);
                    start = body.end;
                }
            }
        }
        regions.push(start..source.len());
        let mut previous = TokenKind::Illegal;
        let mut identifier = None;
        let mut qualifier = None;
        let mut identifier_span = None;
        let mut qualifier_span = None;
        let mut unshadowed = qualifiers.is_some();
        let mut pending_alias = false;
        let mut names = Vec::new();
        let mut importing = false;
        let mut grouped_import = false;
        for region in regions {
            for token in Lexer::new(&source[region.clone()]) {
                let span = region.start + token.span.start..region.start + token.span.end;
                let spelling = &source[span.clone()];
                if pending_alias && token.kind != TokenKind::Period {
                    unshadowed = false;
                }
                pending_alias = false;
                if !importing
                    && token.kind == TokenKind::Ident
                    && qualifiers.is_some_and(|aliases| aliases.contains(&spelling))
                {
                    // Declarations use a bare alias; selector chains use an alias
                    // after a dot. Neither proves a reference to the import.
                    if previous == TokenKind::Period {
                        unshadowed = false;
                    }
                    pending_alias = true;
                }
                if token.kind == TokenKind::Ident
                    && name.is_none_or(|name| source[span.clone()] == *name)
                    && qualifiers.is_none_or(|aliases| {
                        previous == TokenKind::Period
                            && qualifier.is_some_and(|qualifier| aliases.contains(&qualifier))
                    })
                {
                    names.push(Name {
                        span: span.clone(),
                        role: if importing {
                            Role::Namespace
                        } else {
                            match previous {
                                TokenKind::Period => Role::Member(if qualifiers.is_some() {
                                    qualifier_span.clone()
                                } else {
                                    None
                                }),
                                TokenKind::Package | TokenKind::Import => Role::Namespace,
                                _ => Role::Name,
                            }
                        },
                    });
                }
                qualifier = if token.kind == TokenKind::Period {
                    identifier.take()
                } else {
                    None
                };
                qualifier_span = if token.kind == TokenKind::Period {
                    identifier_span.take()
                } else {
                    None
                };
                identifier = (token.kind == TokenKind::Ident).then_some(spelling);
                identifier_span = (token.kind == TokenKind::Ident).then_some(span);
                match token.kind {
                    TokenKind::Import => importing = true,
                    TokenKind::LParen if importing => grouped_import = true,
                    TokenKind::RParen if importing => {
                        importing = false;
                        grouped_import = false;
                    }
                    TokenKind::Semicolon if !grouped_import => importing = false,
                    _ => {}
                }
                previous = token.kind;
            }
        }
        unshadowed &= !pending_alias;
        if !unshadowed {
            for name in &mut names {
                if matches!(name.role, Role::Member(Some(_))) {
                    name.role = Role::Member(None);
                }
            }
        }
        (names, unshadowed)
    }
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
