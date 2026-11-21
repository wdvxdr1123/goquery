//! Bounded lexical and semantic completion candidates; no protocol items.
use super::{Completion, CompletionMember, Kind};
use crate::analysis::query::Context;
use crate::analysis::sema::{CompletionProbe, DeclId, RegionMode, SemanticObject};
use crate::fuzzy::CompletionMatcher;
use crate::index::IndexedSymbol as Symbol;
use crate::text::SourceMap;
use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Instant;

pub(super) struct LocalCandidate {
    pub(super) name: String,
    pub(super) selection: [u32; 2],
    pub(super) scope_start: u32,
    pub(super) is_type: bool,
}

pub(super) fn collect(
    context: &mut Context<'_>,
    offset: usize,
    call_context: bool,
    render_documentation: Option<&mut dyn FnMut(&str) -> String>,
) -> Result<Completion, (i32, String)> {
    let started = Instant::now();
    let trace = context.trace_queries();
    let body = context.cursor_body(offset)?;
    let oversize = body.is_none()
        && crate::analysis::query::cursor_function(
            context.workspace(),
            context.file(),
            context.source(),
            offset,
        )
        .is_some_and(|(_, b)| b.contains(&offset));
    if oversize {
        return Ok(Completion {
            span: offset..offset,
            calls: false,
            items: Vec::new(),
        });
    }
    let expected = if super::type_position(
        context.workspace(),
        context.file(),
        context.source(),
        offset,
        body.as_ref(),
    ) {
        ExpectedKind::Type
    } else {
        ExpectedKind::Any
    };
    let mut calls = call_context;
    let scope_time = started.elapsed();
    let receiver_started = Instant::now();
    let sema_selector = context.completion_selector_members(body.as_ref(), offset);
    let literal = context.completion_literal_members(body.as_ref(), offset);
    let key_only = literal.as_ref().is_some_and(|(_, definite)| *definite);
    let member_site = sema_selector
        .as_ref()
        .or_else(|| literal.as_ref().map(|(site, _)| site));
    drop(body);
    let receiver_time = receiver_started.elapsed();
    let collection_started = Instant::now();
    let locals: Vec<LocalCandidate> = {
        let (doc, sema) = context.split();
        u32::try_from(offset)
            .ok()
            .and_then(|off| {
                sema.region_at(doc.file(), doc.source(), off, RegionMode::Prefix(offset))
            })
            .map(|view| {
                view.visible_locals_at(offset as u32)
                    .into_iter()
                    .map(|local| LocalCandidate {
                        name: local.name.to_owned(),
                        selection: local.selection,
                        scope_start: local.scope_start,
                        is_type: local.is_type,
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    let has_local = |ident: &str| locals.iter().any(|local| local.name == ident);
    context.trace_candidates("completion", 1);
    let (prefix, qualifier, start, end) = {
        let source = context.source();
        let prefix = source[..offset]
            .chars()
            .rev()
            .take_while(|c| crate::syntax::is_identifier_continue(*c))
            .collect::<String>()
            .chars()
            .rev()
            .collect::<String>();
        let before_prefix = &source[..offset - prefix.len()];
        let qualifier = before_prefix.strip_suffix('.').map(|before_dot| {
            let start = before_dot
                .char_indices()
                .rev()
                .find(|(_, c)| !crate::syntax::is_identifier_continue(*c))
                .map_or(0, |(i, c)| i + c.len_utf8());
            before_dot[start..].to_owned()
        });
        let start = offset - prefix.len();
        let end = source[offset..]
            .char_indices()
            .find(|(_, ch)| !crate::syntax::is_identifier_continue(*ch))
            .map_or(source.len(), |(byte, _)| offset + byte);
        (prefix, qualifier, start, end)
    };
    let mut candidates = Candidates::new(&prefix, expected);
    if qualifier.is_none() && !key_only {
        for local in &locals {
            candidates.insert(&local.name, Candidate::Local(local));
        }
    }
    if let Some(alias) = qualifier.as_deref() {
        if let Some(selector) = &sema_selector {
            for member in &selector.members {
                candidates.insert(&member.name, Candidate::Member(member, None));
            }
        } else if !has_local(alias) {
            for (path, symbol) in context.imported_symbols(alias) {
                candidates.insert(&symbol.name, Candidate::Symbol(path, symbol));
            }
        }
    } else {
        if let Some((literal, _)) = &literal {
            for member in &literal.members {
                candidates.insert(&member.name, Candidate::Member(member, None));
            }
        }
        for (path, symbol) in context.file_symbols_in_scope().filter(|_| !key_only) {
            if symbol.receiver.is_none()
                && (!matches!(expected, ExpectedKind::Type) || !has_local(&symbol.name))
            {
                candidates.insert(&symbol.name, Candidate::Symbol(path, symbol));
            }
        }
    }
    let (doc, sema) = context.split();
    let file = doc.file();
    let source = doc.source();
    let map = SourceMap::new(source);
    if matches!(expected, ExpectedKind::Any) {
        let probes: Vec<_> = candidates
            .items
            .values()
            .map(|candidate| {
                (
                    candidate.probe(),
                    matches!(candidate.kind(), Some(Kind::Function | Kind::Method)),
                )
            })
            .collect();
        if let Ok(cursor) = u32::try_from(offset)
            && let Some(ranking) = sema.completion_ranking(
                file,
                source,
                cursor,
                member_site.map(|site| site.request()),
                &probes,
                call_context,
            )
        {
            calls &= !ranking.function_value;
            candidates.items = std::mem::take(&mut candidates.items)
                .into_iter()
                .zip(ranking.compatible)
                .map(|(((Reverse(score), label), candidate), fit)| {
                    let score = f32::from_bits(score) * if fit == Some(true) { 10.0 } else { 1.0 };
                    ((Reverse(score.to_bits()), label), candidate)
                })
                .collect();
        }
    }
    // Formatting follows the 200-item rank cutoff. Re-query the operand in the
    // same native region rather than retaining TypeIds across facade calls.
    if let Some(selector) = member_site {
        let objects: Vec<_> = candidates
            .items
            .values()
            .filter_map(|candidate| match candidate {
                Candidate::Member(member, _) => Some(member.object),
                _ => None,
            })
            .collect();
        if let Some(views) =
            sema.completion_member_views(file, source, selector.request(), &objects)
        {
            let mut views = views.into_iter();
            for candidate in candidates.items.values_mut() {
                if let Candidate::Member(_, view) = candidate {
                    *view = views.next().flatten();
                }
            }
        }
    }
    let edit = super::items::PresentationContext {
        doc,
        map: &map,
        calls,
    };
    let collection_time = collection_started.elapsed();
    let render_started = Instant::now();
    let items = super::items::project(
        candidates
            .items
            .into_iter()
            .map(|((_, label), candidate)| (label, candidate)),
        &edit,
        sema,
        render_documentation,
    );
    if trace {
        eprintln!(
            "completion query profile: scope={scope_time:?} receiver={receiver_time:?} collection={collection_time:?} render={:?} items={} total={:?}",
            render_started.elapsed(),
            items.len(),
            started.elapsed()
        );
    }
    // A longer prefix changes fuzzy ranking and can bring back candidates
    // beyond the current bounded result set. Ask the client to requery.
    Ok(Completion {
        span: start..end,
        calls,
        items,
    })
}
pub(super) enum Candidate<'a> {
    Local(&'a LocalCandidate),
    Symbol(&'a Path, &'a Symbol),
    Member(
        &'a CompletionMember<'a>,
        Option<crate::analysis::sema::MemberTypeView>,
    ),
    Conflict,
}

impl<'a> Candidate<'a> {
    fn probe(&self) -> Option<CompletionProbe<'a>> {
        Some(match self {
            Self::Local(local) => CompletionProbe::Binding {
                selection: local.selection,
                scope_start: local.scope_start,
            },
            Self::Symbol(file, symbol) => CompletionProbe::Declaration(DeclId { file, symbol }),
            Self::Member(member, _) if member.field_key.is_none() => {
                CompletionProbe::Member(member.object)
            }
            _ => return None,
        })
    }

    fn is_type(&self) -> bool {
        matches!(self, Self::Local(local) if local.is_type)
            || matches!(self, Self::Symbol(_, symbol) if symbol.is_type())
    }
    pub(super) fn origin(&self) -> Option<&Path> {
        match self {
            Self::Symbol(path, _) => Some(path),
            Self::Member(member, _) => Some(member.origin()),
            _ => None,
        }
    }

    pub(super) fn owner_start(&self) -> usize {
        match self {
            Self::Member(member, _) => member
                .declaration
                .map_or(0, |description| description.owner.start),
            _ => 0,
        }
    }
    pub(super) fn kind(&self) -> Option<Kind> {
        Some(match self {
            Self::Conflict => return None,
            Self::Local(local) => {
                if local.is_type {
                    Kind::TypeName
                } else {
                    Kind::Variable
                }
            }
            Self::Member(member, _) => {
                if matches!(member.object, SemanticObject::Method(_)) {
                    Kind::Method
                } else {
                    Kind::Field
                }
            }
            Self::Symbol(_, symbol) => {
                if symbol.is_function() {
                    Kind::Function
                } else if symbol.is_method() {
                    Kind::Method
                } else if symbol.is_type() {
                    Kind::TypeName
                } else if symbol.is_const() {
                    Kind::Constant
                } else {
                    Kind::Variable
                }
            }
        })
    }
}

#[derive(Clone, Copy)]
enum ExpectedKind {
    Any,
    Type,
}

struct Candidates<'a> {
    items: BTreeMap<(Reverse<u32>, &'a str), Candidate<'a>>,
    matcher: CompletionMatcher,
    expected: ExpectedKind,
}

impl<'a> Candidates<'a> {
    fn new(prefix: &str, expected: ExpectedKind) -> Self {
        Self {
            items: BTreeMap::new(),
            matcher: CompletionMatcher::new(prefix),
            expected,
        }
    }

    fn insert(&mut self, name: &'a str, candidate: Candidate<'a>) {
        if matches!(self.expected, ExpectedKind::Type) && !candidate.is_type() {
            return;
        }
        let mut score = self.matcher.score(name);
        if let Candidate::Member(member, _) = &candidate
            && let Some(order) = member.field_order
        {
            score *= 10.0 - f32::from(order.min(4095)) * 0.0001;
        }
        if score == 0.0 {
            return;
        }
        let key = (Reverse(score.to_bits()), name);
        if let Some(previous) = self.items.get_mut(&key) {
            match (&candidate, &mut *previous) {
                (Candidate::Local(_), Candidate::Symbol(..) | Candidate::Conflict) => {
                    *previous = candidate
                }
                (Candidate::Symbol(_, symbol), Candidate::Symbol(_, old))
                    if !std::ptr::eq(*symbol, *old) =>
                {
                    *previous = Candidate::Conflict
                }
                (Candidate::Member(member, _), Candidate::Member(old, _))
                    if member.object != old.object =>
                {
                    *previous = Candidate::Conflict
                }
                _ => {}
            }
            return;
        }
        if self.items.len() == 200 {
            if self
                .items
                .last_key_value()
                .is_some_and(|(last, _)| key > *last)
            {
                return;
            }
            self.items.pop_last();
        }
        // Name-only scores make the rank cutoff monotone, even across shadow
        // resolution. Evicted names cannot return; deduplication stays bounded.
        self.items.insert(key, candidate);
    }
}
