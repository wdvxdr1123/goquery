//! printf format directive token splitting for string literals.
//!
//! gopls colors the literal text between directives as `string` and each
//! directive (for example `%s`, `%[2]d`) as `string` plus the non-standard
//! `format` modifier. The directive ranges are raw source ranges, so escapes in
//! the literal keep the original byte extents.
use super::{SemanticTokenKind, modifiers::FORMAT};
use std::ops::Range;

pub(super) fn split(
    span: Range<usize>,
    directives: &[Range<usize>],
) -> Vec<(Range<usize>, SemanticTokenKind, u64)> {
    let mut tokens = Vec::new();
    let mut position = span.start;
    for directive in directives {
        if directive.start > position {
            tokens.push((position..directive.start, SemanticTokenKind::String, 0));
        }
        if directive.end > directive.start {
            tokens.push((directive.clone(), SemanticTokenKind::String, FORMAT));
        }
        position = position.max(directive.end);
    }
    if position < span.end {
        tokens.push((position..span.end, SemanticTokenKind::String, 0));
    }
    tokens
}
