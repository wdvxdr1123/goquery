//! Bounded lexical and semantic completion candidates; no protocol items.
use super::{Completion, CompletionMember, Kind};
use crate::analysis::query::Context;
use crate::analysis::scope::Binding;
use crate::fuzzy::CompletionMatcher;
use crate::index::IndexedSymbol as Symbol;
use crate::text::SourceMap;
use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Instant;

pub(super) fn collect(
    context: &Context<'_>,
    offset: usize,
    call_context: bool,
    render_documentation: Option<&mut dyn FnMut(&str) -> String>,
) -> Result<Completion, (i32, String)> {
    let file = context.file();
    let source = context.source();
    let started = Instant::now();
    let trace = context.trace_queries();
    let map = SourceMap::new(source);
    let mut scope = context.cursor_scope_with_hint(offset, Some(offset))?;
    if !scope.available {
        return Ok(Completion {
            span: offset..offset,
            calls: false,
            items: Vec::new(),
        });
    }
    let expected = if context.type_position(offset, &scope) {
        ExpectedKind::Type
    } else {
        ExpectedKind::Any
    };
    let mut calls = call_context;
    let scope_time = started.elapsed();
    let receiver_started = Instant::now();
    let sema_selector = context.completion_selector_members(&scope, offset);
    let literal = context.completion_literal_members(&scope, offset, &map);
    let key_only = literal.as_ref().is_some_and(|(_, definite)| *definite);
    let member_site = sema_selector
        .as_ref()
        .or_else(|| literal.as_ref().map(|(site, _)| site));
    drop(scope.body.take());
    let receiver_time = receiver_started.elapsed();
    let collection_started = Instant::now();
    let locals = &scope.locals;
    context.trace_candidates("completion", 1);
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
        &before_dot[start..]
    });
    let mut candidates = Candidates::new(&prefix, expected);
    let visible = locals
        .visible_bindings(offset)
        .filter(|_| qualifier.is_none() && !key_only);
    for binding in visible {
        candidates.insert(&binding.name, Candidate::Local(binding));
    }
    if let Some(alias) = qualifier {
        if let Some(selector) = &sema_selector {
            for member in &selector.members {
                candidates.insert(&member.name, Candidate::Member(member, None));
            }
        } else if locals.resolve_at(offset, alias).is_none() {
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
                && (!matches!(expected, ExpectedKind::Type)
                    || locals.resolve_at(offset, &symbol.name).is_none())
            {
                candidates.insert(&symbol.name, Candidate::Symbol(path, symbol));
            }
        }
    }
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
            && let Some(ranking) = context.with_sema(|sema| {
                sema.completion_ranking(
                    file,
                    source,
                    cursor,
                    member_site.map(|site| site.request()),
                    &probes,
                    call_context,
                )
            })
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
        if let Some(views) = context.with_sema(|sema| {
            sema.completion_member_views(file, source, selector.request(), &objects)
        }) {
            let mut views = views.into_iter();
            for candidate in candidates.items.values_mut() {
                if let Candidate::Member(_, view) = candidate {
                    *view = views.next().flatten();
                }
            }
        }
    }
    let start = offset - prefix.len();
    let end = source[offset..]
        .char_indices()
        .find(|(_, ch)| !crate::syntax::is_identifier_continue(*ch))
        .map_or(source.len(), |(byte, _)| offset + byte);
    let edit = super::items::PresentationContext {
        query: context,
        file,
        source,
        locals,
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
    Local(&'a Binding),
    Symbol(&'a Path, &'a Symbol),
    Member(
        &'a CompletionMember<'a>,
        Option<crate::analysis::sema::MemberTypeView>,
    ),
    Conflict,
}

impl<'a> Candidate<'a> {
    fn probe(&self) -> Option<crate::analysis::sema::CompletionProbe<'a>> {
        use crate::analysis::sema::CompletionProbe;
        Some(match self {
            Self::Local(binding) => CompletionProbe::Binding {
                selection: [
                    u32::try_from(binding.selection.start).ok()?,
                    u32::try_from(binding.selection.end).ok()?,
                ],
                scope_start: u32::try_from(binding.scope_start()).ok()?,
            },
            Self::Symbol(file, symbol) => {
                CompletionProbe::Declaration(crate::analysis::obj::DeclId { file, symbol })
            }
            Self::Member(member, _) if member.field_key.is_none() => {
                CompletionProbe::Member(member.object)
            }
            _ => return None,
        })
    }

    fn is_type(&self) -> bool {
        matches!(self, Self::Local(binding) if binding.object_kind.is_type())
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
            Self::Local(binding) => {
                if binding.object_kind.is_type() {
                    Kind::TypeName
                } else {
                    Kind::Variable
                }
            }
            Self::Member(member, _) => {
                if matches!(member.object, crate::analysis::obj::ObjectRef::Method(_)) {
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
                (Candidate::Local(binding), Candidate::Local(old))
                    if binding.is_inner_than(old) =>
                {
                    *previous = candidate
                }
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
