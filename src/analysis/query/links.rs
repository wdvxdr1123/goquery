//! Source-scanned hyperlinks: import paths plus URLs in comments and strings.
//!
//! Mirrors gopls v0.24.0 `internal/server/link.go`: one bounded token pass
//! collects import path literals, every other string literal and every comment;
//! imports become `pkg.go.dev` links (dependency imports carry `@version`),
//! while strings and comments are scanned for xurls URLs and `org/repo#N`
//! issue links. No source or token data survives the request.

mod escape;
mod schemes;
mod text;
mod tlds;
mod unicode;
mod url;
mod xurls;

use super::Context;
use crate::syntax::{Lexer, TokenKind};
use crate::workspace::Workspace;
use std::borrow::Cow;
use std::ops::Range;
use xurls::Budget;

/// gopls returns every link it finds; this bound keeps pathological buffers
/// from growing an unbounded response.
const MAX_LINKS: usize = 8192;
/// Character-work budget for one document request.
const SCAN_BUDGET: u32 = 8_000_000;

pub(crate) struct DocumentLink {
    pub(crate) span: Range<usize>,
    pub(crate) target: String,
}

impl Context<'_> {
    pub(crate) fn document_links(&self) -> Result<Vec<DocumentLink>, (i32, String)> {
        let workspace = self.workspace();
        let source = self.source();
        let mut budget = Budget::new(SCAN_BUDGET);
        let mut imports = Vec::new();
        let mut strings = Vec::new();
        let mut comments = Vec::new();
        collect_pieces(source, &mut imports, &mut strings, &mut comments);
        let mut links = Vec::new();
        for span in imports {
            if links.len() == MAX_LINKS {
                return Ok(links);
            }
            let Some(path) = crate::syntax::decode_import_path(&source[span.clone()]) else {
                continue;
            };
            if path.is_empty() {
                continue;
            }
            links.push(DocumentLink {
                // gopls accounts for the quotation marks in the positions.
                span: span.start + 1..span.end - 1,
                target: import_target(workspace, &path),
            });
        }
        for span in strings.iter().chain(comments.iter()) {
            let piece = scanner_text(source, span);
            for (start, end, target) in text::links(&piece, &mut budget) {
                if links.len() == MAX_LINKS {
                    return Ok(links);
                }
                // gopls scans the scanner-normalized text and adds its offsets to
                // the literal start, so expanded CRLF files inherit that mapping.
                let span = span.start + start..span.start + end;
                links.push(DocumentLink { span, target });
            }
            if budget.exhausted() {
                break;
            }
        }
        Ok(links)
    }
}

/// Text exactly as Go's scanner hands it to the parser: carriage returns are
/// stripped from comments and raw string literals (with the `*\r/` exception
/// for block comments), while interpreted literals keep their source bytes.
fn scanner_text<'source>(source: &'source str, span: &Range<usize>) -> Cow<'source, str> {
    let bytes = &source.as_bytes()[span.clone()];
    let text = &source[span.clone()];
    if bytes.first() == Some(&b'`') || bytes.starts_with(b"//") {
        return strip_carriage_returns(text);
    }
    if bytes.starts_with(b"/*") {
        if !bytes.contains(&b'\r') {
            return Cow::Borrowed(text);
        }
        let mut stripped = Vec::with_capacity(bytes.len());
        for (index, byte) in bytes.iter().enumerate() {
            let keep = *byte != b'\r'
                || (stripped.len() > 2
                    && stripped[stripped.len() - 1] == b'*'
                    && index + 1 < bytes.len()
                    && bytes[index + 1] == b'/');
            if keep {
                stripped.push(*byte);
            }
        }
        return Cow::Owned(String::from_utf8(stripped).expect("CR-free comment text is UTF-8"));
    }
    Cow::Borrowed(text)
}

fn strip_carriage_returns(text: &str) -> Cow<'_, str> {
    if text.as_bytes().contains(&b'\r') {
        Cow::Owned(text.replace('\r', ""))
    } else {
        Cow::Borrowed(text)
    }
}

/// Splits the token stream into import path literals and text pieces exactly
/// like gopls' AST walk: import spec paths are excluded from URL scanning, and
/// import aliases/parentheses are not text.
fn collect_pieces(
    source: &str,
    imports: &mut Vec<Range<usize>>,
    strings: &mut Vec<Range<usize>>,
    comments: &mut Vec<Range<usize>>,
) {
    let mut expect_path = false;
    let mut in_block = false;
    for token in Lexer::with_comments(source) {
        match token.kind {
            TokenKind::Comment => comments.push(token.span),
            TokenKind::Import => {
                expect_path = true;
                in_block = false;
            }
            TokenKind::String => {
                if expect_path || in_block {
                    imports.push(token.span);
                    expect_path = false;
                } else {
                    strings.push(token.span);
                }
            }
            TokenKind::LParen if expect_path => {
                expect_path = false;
                in_block = true;
            }
            TokenKind::RParen if in_block => in_block = false,
            TokenKind::Semicolon => expect_path = false,
            // An import alias (`_`, `.` or a name) precedes its path literal.
            TokenKind::Ident | TokenKind::Period if expect_path => {}
            _ => expect_path = false,
        }
    }
}

fn import_target(workspace: &Workspace, import: &str) -> String {
    let url_path = match workspace.import_module(import) {
        Some(identity) if identity.version.is_some() => {
            match import.strip_prefix(identity.prefix) {
                Some(rest) if rest.is_empty() || rest.starts_with('/') => {
                    let version = identity.version.as_deref().expect("checked version");
                    format!("{}@{version}{rest}", identity.module)
                }
                _ => import.to_string(),
            }
        }
        _ => import.to_string(),
    };
    format!("https://pkg.go.dev/{url_path}")
}

/// Differential golden test against recorded gopls v0.24.0 `findLinksInString`
/// results (`tests/fixtures/document-link/corpus.json`). `GOQUERY_DOCLINK_CORPUS`
/// points at a larger local corpus with the same schema.
#[cfg(test)]
mod tests {
    use super::Budget;
    use super::text;
    use serde_json::Value;
    use std::path::PathBuf;

    fn corpus_path() -> PathBuf {
        match std::env::var_os("GOQUERY_DOCLINK_CORPUS") {
            Some(path) => PathBuf::from(path),
            None => PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/document-link/corpus.json"),
        }
    }

    fn expected_links(value: &Value) -> Vec<(usize, usize, String)> {
        value
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|link| {
                (
                    link["start"].as_u64().expect("start") as usize,
                    link["end"].as_u64().expect("end") as usize,
                    link["target"].as_str().expect("target").to_string(),
                )
            })
            .collect()
    }

    #[test]
    fn text_links_match_recorded_gopls_results() {
        let text = std::fs::read_to_string(corpus_path()).expect("document link corpus");
        let corpus: Value = serde_json::from_str(&text).expect("corpus json");
        let pieces = corpus["pieces"].as_array().expect("pieces");
        let mut failures = 0usize;
        for piece in pieces {
            let text = piece["text"].as_str().expect("text");
            let mut budget = Budget::new(u32::MAX);
            let actual = text::links(text, &mut budget);
            let expected = expected_links(&piece["links"]);
            if actual != expected {
                failures += 1;
                if failures <= 10 {
                    eprintln!(
                        "piece mismatch: {text:?}\n  actual:   {actual:?}\n  expected: {expected:?}"
                    );
                }
            }
        }
        assert_eq!(failures, 0, "{failures}/{} pieces differ", pieces.len());
    }
}
