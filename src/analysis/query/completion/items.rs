//! Project selected candidates by file/owner; retain no closed source.
use super::candidates::Candidate;
use super::comments::{self, Target};
use super::{Context, Item, Presentation};
use crate::analysis::query::completion::SignatureSource;
use crate::analysis::scope::Locals;
use crate::syntax::TokenKind;
use crate::text::SourceMap;
use std::path::Path;

pub(super) struct PresentationContext<'a, 'workspace> {
    pub(super) query: &'a Context<'workspace>,
    pub(super) file: &'a Path,
    pub(super) source: &'a str,
    pub(super) locals: &'a Locals,
    pub(super) map: &'a SourceMap<'a>,
    pub(super) calls: bool,
}

pub(super) fn project<'a>(
    candidates: impl Iterator<Item = (&'a str, Candidate<'a>)>,
    context: &PresentationContext<'_, '_>,
    mut render_documentation: Option<&mut dyn FnMut(&str) -> String>,
) -> Vec<Item> {
    let mut ranked: Vec<_> = candidates.enumerate().collect();
    ranked.sort_by(|(_, (_, a)), (_, (_, b))| {
        a.origin()
            .cmp(&b.origin())
            .then(a.owner_start().cmp(&b.owner_start()))
            .then_with(|| method_receiver(a).cmp(&method_receiver(b)))
    });
    let mut results = Vec::with_capacity(ranked.len());
    let mut documentation_bytes = 0;
    for group in ranked.chunk_by(|(_, (_, a)), (_, (_, b))| a.origin() == b.origin()) {
        let origin = group[0].1.1.origin();
        let has_members = group
            .iter()
            .any(|(_, (_, item))| matches!(item, Candidate::Member(..)));
        let needs_signatures = group.iter().any(
            |(_, (_, item))| matches!(item, Candidate::Symbol(_, symbol) if symbol.is_callable()),
        );
        let needs_source = has_members || needs_signatures || render_documentation.is_some();
        let storage = needs_source
            .then(|| {
                origin
                    .filter(|origin| *origin != context.file)
                    .and_then(|origin| context.query.read_source(origin))
            })
            .flatten();
        let source = if origin == Some(context.file) {
            Some(context.source)
        } else {
            storage.as_deref()
        };
        let owned_map = source
            .filter(|_| needs_source && origin != Some(context.file))
            .map(SourceMap::new);
        let map = if origin == Some(context.file) {
            Some(context.map)
        } else {
            owned_map.as_ref()
        };
        let mut owner = None;
        let mut declaration = "";
        let mut entries = Vec::new();
        let mut targets = Vec::new();
        let mut documented = Vec::new();
        for (rank, (label, candidate)) in group {
            let presentation = if let Candidate::Member(member, view) = candidate {
                if let Some(description) = member.declaration
                    && owner != Some(description.owner.start)
                {
                    declaration = source
                        .zip(map.as_ref())
                        .and_then(|(source, map)| {
                            source.get(
                                description.owner.end..map.offset(description.owner.range.end)?,
                            )
                        })
                        .filter(|declaration| declaration.len() <= 16 * 1024)
                        .unwrap_or("");
                    entries = context.query.aggregate_members(declaration);
                    owner = Some(description.owner.start);
                }
                context.query.member_presentation(
                    member,
                    view.as_ref(),
                    source,
                    map,
                    declaration,
                    &entries,
                )
            } else if let Candidate::Symbol(origin, symbol) = candidate {
                let location = source.zip(map).map(|(source, map)| SignatureSource {
                    file: context.file,
                    origin,
                    source,
                    map,
                });
                context
                    .query
                    .symbol_completion(origin, symbol, location.as_ref())
            } else if let Candidate::Local(binding) = candidate {
                context.query.local_completion_type(context.locals, binding)
            } else {
                None
            };
            if let Some(item) = candidate_item(label, candidate, context, presentation) {
                if render_documentation.is_some()
                    && let Some(map) = map
                {
                    let target = match candidate {
                        Candidate::Symbol(_, symbol) => map
                            .offset(symbol.range.start)
                            .zip(map.offset(symbol.range.end))
                            .map(|(start, end)| Target {
                                start,
                                end,
                                trailing: !symbol.is_callable(),
                            }),
                        Candidate::Member(member, _) if member.method.is_some() => {
                            member.method.and_then(|(_, symbol)| {
                                map.offset(symbol.range.start)
                                    .zip(map.offset(symbol.range.end))
                                    .map(|(start, end)| Target {
                                        start,
                                        end,
                                        trailing: false,
                                    })
                            })
                        }
                        Candidate::Member(member, _) => member.declaration.and_then(|member| {
                            member.entry(declaration, &entries).map(|entry| {
                                let start = entries
                                    .iter()
                                    .filter(|other| {
                                        other.container == entry.container
                                            && other.annotation == entry.annotation
                                    })
                                    .map(|other| other.selection.start)
                                    .min()
                                    .unwrap_or(entry.selection.start);
                                let mut end = member.owner.end + entry.annotation.end;
                                if let Some(source) = source
                                    && let Some(tag) =
                                        crate::syntax::Lexer::new(&source[end..]).next()
                                    && tag.kind == TokenKind::String
                                {
                                    end += tag.span.end;
                                }
                                Target {
                                    start: member.owner.end + start,
                                    end,
                                    trailing: true,
                                }
                            })
                        }),
                        _ => None,
                    };
                    if let Some(target) = target {
                        documented.push(results.len());
                        targets.push(target);
                    }
                }
                results.push((*rank, item));
            }
        }
        if !targets.is_empty()
            && let Some(source) = source
            && let Some(render_documentation) = render_documentation.as_deref_mut()
        {
            for (index, comment) in documented.into_iter().zip(comments::comments(
                source,
                &targets,
                (256 * 1024 - documentation_bytes) / 2,
            )) {
                if let Some(comment) = comment {
                    let value = render_documentation(&comment);
                    if documentation_bytes + value.len() > 256 * 1024 {
                        continue;
                    }
                    documentation_bytes += value.len();
                    results[index].1.documentation = Some(value);
                }
            }
        }
    }
    results.sort_by_key(|(rank, _)| *rank);
    results.into_iter().map(|(_, item)| item).collect()
}

fn method_receiver<'a>(candidate: &Candidate<'a>) -> Option<&'a str> {
    match candidate {
        Candidate::Symbol(_, symbol) => symbol.receiver.as_deref(),
        Candidate::Member(member, _) => member
            .method
            .and_then(|(_, symbol)| symbol.receiver.as_deref()),
        _ => None,
    }
}

fn candidate_item(
    label: &str,
    candidate: &Candidate<'_>,
    context: &PresentationContext<'_, '_>,
    presentation: Option<Presentation>,
) -> Option<Item> {
    let kind = candidate.kind()?;
    let (detail, signature, type_parameters, kind) =
        presentation.map_or((None, None, None, kind), |presentation| {
            (
                presentation.detail,
                presentation.signature,
                presentation.type_parameters,
                presentation.kind.unwrap_or(kind),
            )
        });
    let signature = match candidate {
        Candidate::Symbol(_, symbol) if symbol.is_callable() => signature,
        Candidate::Local(binding) => context.query.binding_signature(context.locals, binding),
        Candidate::Member(..) => signature,
        _ => None,
    }
    .map(|(signature, _)| signature);
    let detail = signature
        .as_ref()
        .and_then(|signature| {
            signature
                .label
                .find('(')
                .map(|opening| format!("func{}", &signature.label[opening..]))
        })
        .or(detail)
        .or_else(|| match candidate {
            Candidate::Local(binding) => context.query.binding_detail(context.locals, binding),
            _ => None,
        });
    Some(Item {
        label: label.to_owned(),
        kind,
        detail,
        signature: signature.filter(|_| {
            context.calls && matches!(kind, super::Kind::Function | super::Kind::Method)
        }),
        type_parameters,
        documentation: None,
        field_key: match candidate {
            Candidate::Member(member, _) => member.field_key,
            _ => None,
        },
    })
}
