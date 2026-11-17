//! Native field identities plus syntax-only anonymous-aggregate conflict sites.
use super::super::Context;
use crate::analysis::obj::ObjectRef;
use crate::analysis::scope::occurrences::{Name, Occurrences, Role};
use crate::analysis::sema::LiteralKey;
use crate::syntax::{Lexer, MemberKind, TokenKind};
use std::ops::Range;
use std::path::{Path, PathBuf};

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

/// Only a checked field object can establish a selector/declaration identity.
pub(crate) fn field_at<View>(
    context: &Context<'_, View>,
    file: &Path,
    source: &str,
    selection: &Range<usize>,
) -> Option<InlineField> {
    let span = [
        u32::try_from(selection.start).ok()?,
        u32::try_from(selection.end).ok()?,
    ];
    context.with_sema(|sema| {
        let (found, ObjectRef::Field(member)) = sema.object_at(file, source, span[0])? else {
            return None;
        };
        (found == span && !member.builtin && member.selection[0] < member.selection[1]).then(|| {
            InlineField {
                file: member.file.to_path_buf(),
                selection: member.selection[0] as usize..member.selection[1] as usize,
                name: source.get(selection.clone()).unwrap_or_default().to_owned(),
            }
        })
    })
}

/// Candidate roles supply literal container coordinates, never type spellings.
pub(crate) fn occurrence_field<View>(
    context: &Context<'_, View>,
    file: &Path,
    source: &str,
    analysis: &Occurrences,
    occurrence: &Name,
) -> Option<InlineField> {
    if let Some(field) = field_at(context, file, source, &occurrence.span) {
        return Some(field);
    }
    let (annotation, projections) = match &occurrence.role {
        Role::LiteralKey(annotation) => (annotation.clone(), Vec::new()),
        Role::LiteralElementKey(key) => analysis.literal_element_projection(*key)?,
        _ => return None,
    };
    let name = source.get(occurrence.span.clone())?;
    context.with_sema(|sema| {
        let LiteralKey::Field(member) =
            sema.literal_key_at(file, source, annotation, &projections, name)
        else {
            return None;
        };
        Some(InlineField {
            file: member.file.to_path_buf(),
            selection: member.selection[0] as usize..member.selection[1] as usize,
            name: name.to_owned(),
        })
    })
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
