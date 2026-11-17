//! Cursor syntax locates sites; only checked native facts create members.
use super::{CompletionMember, CompletionMembers, FieldKey};
use crate::analysis::obj::{MethodOrigin, ObjectRef};
use crate::analysis::query::Context;
use crate::analysis::scope::CursorScope;
use crate::analysis::sema::{CompletionSite, MemberRequest};
use crate::syntax::{ExprKind, Lexer, TokenKind};
use crate::text::SourceMap;

impl<'a> Context<'a> {
    pub(in crate::analysis::query) fn completion_selector_members(
        &self,
        scope: &CursorScope,
        offset: usize,
    ) -> Option<CompletionMembers<'a>> {
        let body = scope.body.as_ref()?;
        let operand = body.tree.expressions.iter().find_map(|expression| {
            let ExprKind::Selector { operand, field } = &expression.kind else {
                return None;
            };
            let end = body.base + body.tree.expressions.get(operand.0)?.span.end;
            let gap = self.source().get(end..body.base + field.start)?;
            let period = Lexer::new(gap)
                .next()
                .filter(|token| token.kind == TokenKind::Period)?;
            // A missing field can consume the next line's identifier. The
            // completion cursor still belongs to the preceding selector dot.
            (end + period.span.end..=body.base + field.end)
                .contains(&offset)
                .then_some(*operand)
        })?;
        let expression = body.tree.expressions.get(operand.0)?;
        let operand = [
            u32::try_from(body.base + expression.span.start).ok()?,
            u32::try_from(body.base + expression.span.end).ok()?,
        ];
        let offset = u32::try_from(offset).ok()?;
        let site = crate::analysis::sema::CompletionSite::Selector(operand);
        let members = self
            .with_sema(|sema| {
                sema.completion_members(
                    self.file(),
                    self.source(),
                    crate::analysis::sema::MemberRequest {
                        site,
                        offset,
                        used: &[],
                    },
                )
            })?
            .into_iter()
            .map(|(name, object)| {
                let method = match object {
                    ObjectRef::Method(MethodOrigin::Concrete(decl)) => {
                        Some((decl.file, decl.symbol))
                    }
                    _ => None,
                };
                let declaration = match object {
                    ObjectRef::Field(member)
                    | ObjectRef::Method(MethodOrigin::Interface(member)) => {
                        member.describe_name(self.workspace, &name)
                    }
                    _ => None,
                };
                CompletionMember {
                    name,
                    object,
                    method,
                    declaration,
                    field_key: None,
                    field_order: None,
                }
            })
            .collect();
        Some(CompletionMembers {
            site,
            used: Vec::new(),
            offset,
            members,
        })
    }

    pub(in crate::analysis::query) fn completion_literal_members(
        &self,
        scope: &CursorScope,
        offset: usize,
        map: &SourceMap<'_>,
    ) -> Option<(CompletionMembers<'a>, bool)> {
        let body = scope.body.as_ref()?;
        let source = self.source();
        let (expression, elements, opening) = body
            .tree
            .expressions
            .iter()
            .filter_map(|expression| {
                let ExprKind::Composite {
                    literal_type,
                    elements,
                } = &expression.kind
                else {
                    return None;
                };
                let start = literal_type
                    .and_then(|id| body.tree.expressions.get(id.0))
                    .map_or(expression.span.start, |ty| ty.span.end)
                    + body.base;
                let end = body.base + expression.span.end;
                let opening = Lexer::new(source.get(start..end)?)
                    .find(|token| token.kind == TokenKind::LBrace)?;
                let opening = start + opening.span.start;
                let closed = source.get(end.saturating_sub(1)..end) == Some("}");
                (opening < offset && offset <= end.saturating_sub(usize::from(closed)))
                    .then_some((expression, elements, opening))
            })
            .min_by_key(|(expression, _, _)| expression.span.len())?;
        let mut has_keys = false;
        let mut existing_key = false;
        let mut used = Vec::new();
        for id in elements {
            let element = body.tree.expressions.get(id.0)?;
            let ExprKind::KeyValue { key, value } = &element.kind else {
                continue;
            };
            has_keys = true;
            let key = body.tree.expressions.get(key.0)?;
            let current =
                (body.base + element.span.start..=body.base + element.span.end).contains(&offset);
            if current {
                let value = body.tree.expressions.get(value.0)?;
                let start = body.base + key.span.end;
                let colon = Lexer::new(source.get(start..body.base + value.span.start)?)
                    .find(|token| token.kind == TokenKind::Colon)?;
                if offset > start + colon.span.start {
                    return None;
                }
                existing_key = true;
            } else {
                if used.len() >= crate::analysis::budget::MAX_NODES {
                    return None;
                }
                used.push([
                    u32::try_from(body.base + key.span.start).ok()?,
                    u32::try_from(body.base + key.span.end).ok()?,
                ]);
            }
        }
        if !has_keys && !elements.is_empty() {
            if elements.len() != 1 {
                return None;
            }
            let element = body.tree.expressions.get(elements[0].0)?;
            if !matches!(element.kind, ExprKind::Name)
                || !(body.base + element.span.start..=body.base + element.span.end)
                    .contains(&offset)
            {
                return None;
            }
        }
        let offset = u32::try_from(offset).ok()?;
        let site = CompletionSite::Literal([
            u32::try_from(body.base + expression.span.start).ok()?,
            u32::try_from(body.base + expression.span.end).ok()?,
        ]);
        let raw = self.with_sema(|sema| {
            sema.completion_members(
                self.file(),
                source,
                MemberRequest {
                    site,
                    offset,
                    used: &used,
                },
            )
        })?;
        let key = FieldKey {
            colon: !existing_key,
            comma: map.position(opening).line != map.position(offset as usize).line,
        };
        let members = raw
            .into_iter()
            .enumerate()
            .map(|(index, (name, object))| {
                let declaration = match object {
                    ObjectRef::Field(member) => member.describe_name(self.workspace, &name),
                    _ => None,
                };
                CompletionMember {
                    name,
                    object,
                    method: None,
                    declaration,
                    field_key: Some(key),
                    field_order: u16::try_from(index).ok(),
                }
            })
            .collect();
        Some((
            CompletionMembers {
                site,
                used,
                offset,
                members,
            },
            has_keys,
        ))
    }
}
