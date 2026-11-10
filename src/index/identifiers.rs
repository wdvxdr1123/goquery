//! Compact negative filter for identifier candidates, never a reference identity.
use crate::bloom::{BloomFilter, Fingerprint};
use crate::syntax::{Lexer, TokenKind};

/// Three probes into 2,048 bits; source text and token lists are not retained.
#[derive(Clone, Debug, Default)]
pub(crate) struct IdentifierFilter(BloomFilter);

impl IdentifierFilter {
    pub(crate) fn new(source: &str) -> Self {
        let mut filter = Self::default();
        for token in Lexer::new(source) {
            if token.kind == TokenKind::Ident {
                filter.0.insert(source[token.span].as_bytes());
            }
        }
        filter
    }

    pub(crate) fn may_contain_fingerprint(&self, fingerprint: Fingerprint) -> bool {
        self.0.may_contain_fingerprint(fingerprint)
    }
}
