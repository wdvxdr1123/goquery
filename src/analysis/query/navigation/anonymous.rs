//! Syntax-only anonymous-aggregate conflict sites for rename validation.
use crate::syntax::{Lexer, MemberKind, TokenKind};
use std::ops::Range;
use std::path::PathBuf;

struct InlineStruct {
    members: Vec<(String, Range<usize>)>,
}

/// Syntax-only declaration index used to check rename conflicts.
pub(crate) struct InlineIndex {
    structs: Vec<InlineStruct>,
}

impl InlineIndex {
    pub(crate) fn new(source: &str) -> Self {
        Self {
            structs: inline_structs(source),
        }
    }
}

pub(crate) struct InlineField {
    pub(crate) file: PathBuf,
    pub(crate) selection: Range<usize>,
    pub(crate) name: String,
}

/// A direct field conflict in the same anonymous aggregate.
pub(crate) fn field_conflict(
    index: &InlineIndex,
    selection: &Range<usize>,
    new_name: &str,
) -> Option<Range<usize>> {
    let owner = index
        .structs
        .iter()
        .find(|site| site.members.iter().any(|(_, span)| span == selection))?;
    owner
        .members
        .iter()
        .find(|(name, span)| name == new_name && span != selection)
        .map(|(_, span)| span.clone())
}

fn inline_structs(source: &str) -> Vec<InlineStruct> {
    let mut result = Vec::new();
    let mut tokens = Lexer::new(source);
    while let Some(token) = tokens.next() {
        if token.kind != TokenKind::Struct {
            continue;
        }
        let start = token.span.start;
        let mut depth = 0usize;
        let mut end = None;
        for token in tokens.clone() {
            match token.kind {
                TokenKind::LBrace => depth += 1,
                TokenKind::RBrace => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        end = Some(token.span.end);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(end) = end else { continue };
        let text = &source[start..end];
        let Some(root) = crate::syntax::aggregate_offset(text) else {
            continue;
        };
        let members: Vec<(String, Range<usize>)> = crate::syntax::aggregate_members(text)
            .into_iter()
            .filter(|member| member.container == root && member.symbol.kind == MemberKind::Field)
            .map(|member| {
                (
                    member.symbol.name,
                    start + member.selection.start..start + member.selection.end,
                )
            })
            .collect();
        result.push(InlineStruct { members });
    }
    result
}
