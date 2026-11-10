//! Render selected members by file/owner, without retaining closed sources.
use crate::analysis::check::callable_signature::SignatureSource;
use crate::features::completion::collect::Candidate;
use crate::features::completion::render::EditContext;
use crate::features::documentation;
use crate::features::documentation::Target;
use crate::syntax::TokenKind;
use crate::text::SourceMap;
use serde_json::{Value, json};

pub(super) fn render_completion_candidates<'a>(
    candidates: impl Iterator<Item = (&'a str, Candidate<'a>)>,
    context: &EditContext<'_, '_>,
) -> Vec<Value> {
    let workspace = context.query.workspace();
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
            .any(|(_, (_, item))| matches!(item, Candidate::Member(_)));
        let needs_signatures = group.iter().any(
            |(_, (_, item))| matches!(item, Candidate::Symbol(_, symbol) if symbol.is_callable()),
        );
        let has_types = group.iter().any(
                |(_, (_, item))| matches!(item, Candidate::Symbol(_, symbol) if symbol.is_type() && symbol.type_info.as_ref().is_none_or(|info| info.target.is_some())),
            );
        let needs_source = has_members
            || needs_signatures
            || has_types
            || workspace.completion_config.documentation;
        let storage = needs_source
            .then(|| {
                origin
                    .filter(|origin| *origin != context.file)
                    .and_then(|origin| workspace.source(origin))
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
        let mut types = None;
        let mut methods = None;
        let mut targets = Vec::new();
        let mut documented = Vec::new();
        for (rank, (label, candidate)) in group {
            let presentation = if let Candidate::Member(member) = candidate {
                if owner != Some(member.owner.start) {
                    entries.clear();
                    declaration = source
                        .zip(map.as_ref())
                        .and_then(|(source, map)| {
                            source.get(member.owner.end..map.offset(member.owner.range.end)?)
                        })
                        .unwrap_or("");
                    entries = context.query.aggregate_members(declaration);
                    types = context.receiver.and_then(|(scope, receiver)| {
                        context
                            .query
                            .completion_member_types(scope, receiver, *member, declaration)
                    });
                    owner = Some(member.owner.start);
                }
                context
                    .query
                    .member_presentation(types.as_ref(), member, declaration, &entries)
            } else if let Candidate::Symbol(origin, symbol) = candidate {
                let location = source.zip(map).map(|(source, map)| SignatureSource {
                    file: context.file,
                    origin,
                    source,
                    map,
                });
                context.query.symbol_completion(
                    symbol,
                    location.as_ref(),
                    context.receiver,
                    &mut methods,
                )
            } else if let Candidate::Local(binding) = candidate {
                context.query.local_completion_type(context.locals, binding)
            } else {
                None
            };
            if let Some(item) =
                super::render::completion_item(label, candidate, context, presentation)
            {
                if workspace.completion_config.documentation
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
                        Candidate::Member(member) => {
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
                        }
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
        {
            for (index, comment) in documented.into_iter().zip(documentation::comments(
                source,
                &targets,
                (256 * 1024 - documentation_bytes) / 2,
            )) {
                if let Some(comment) = comment {
                    let value = if workspace.completion_config.markdown {
                        documentation::markdown(&comment, &[])
                    } else {
                        comment
                    };
                    if documentation_bytes + value.len() > 256 * 1024 {
                        continue;
                    }
                    documentation_bytes += value.len();
                    results[index].1["documentation"] = if workspace.completion_config.markdown {
                        json!({"kind":"markdown", "value":value})
                    } else {
                        value.into()
                    };
                }
            }
        }
    }
    results.sort_by_key(|(rank, _)| *rank);
    results
        .into_iter()
        .enumerate()
        .map(|(rank, (_, mut item))| {
            item["sortText"] = format!("{rank:05}").into();
            if rank == 0 {
                item["preselect"] = true.into();
            }
            item
        })
        .collect()
}

fn method_receiver<'a>(candidate: &Candidate<'a>) -> Option<&'a str> {
    match candidate {
        Candidate::Symbol(_, symbol) => symbol.receiver.as_deref(),
        _ => None,
    }
}
