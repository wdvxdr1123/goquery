//! Completion parses only the enclosing function and the cursor's closure path.
use crate::analysis::check::ScopedType;
use crate::analysis::member::MemberDeclaration;
use crate::analysis::method_set::ReceiverMember;
use crate::analysis::query::Context;
use crate::analysis::scope::Binding;
use crate::features::completion::render::EditContext;
use crate::fuzzy::CompletionMatcher;
use crate::index::IndexedSymbol as Symbol;
use crate::text::SourceMap;
use serde_json::{Value, json};
use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Instant;

pub(in crate::features) fn execute(
    context: &Context<'_>,
    offset: usize,
) -> Result<Value, (i32, String)> {
    let workspace = context.workspace();
    let file = context.file();
    let source = context.source();
    let started = Instant::now();
    let trace = workspace.trace.queries;
    let map = SourceMap::new(source);
    let mut scope = context.cursor_scope(offset)?;
    let expected = if context.type_position(offset, &scope) {
        ExpectedKind::Type
    } else {
        ExpectedKind::Any
    };
    let calls = workspace.completion_config.snippets
        && workspace.completion_config.calls
        && !context.function_value_context(offset, &scope);
    let scope_time = started.elapsed();
    let receiver_started = Instant::now();
    let selected_receiver = context.completion_receiver(&scope, offset);
    drop(scope.body.take());
    let receiver_time = receiver_started.elapsed();
    let collection_started = Instant::now();
    let locals = &scope.locals;
    workspace.trace_candidates("completion", 1);
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
    let mut receiver = selected_receiver;
    let visible = locals
        .visible_bindings(offset)
        .filter(|_| qualifier.is_none());
    for binding in visible {
        candidates.insert(&binding.name, Candidate::Local(binding));
    }
    if let Some(alias) = qualifier {
        let binding = locals.resolve_at(offset, alias);
        if receiver.is_some() || binding.is_some() {
            if receiver.is_none() {
                receiver = binding
                    .and_then(|binding| context.binding_type(locals, binding))
                    .map(|text| ScopedType { scope: file, text });
            }
            for member in receiver
                .as_ref()
                .map_or_else(Vec::new, |ty| context.completion_members(ty))
            {
                match member {
                    ReceiverMember::Method(path, symbol) => {
                        candidates.insert(&symbol.name, Candidate::Symbol(path, symbol))
                    }
                    ReceiverMember::Declaration(member) => {
                        candidates.insert(member.name, Candidate::Member(member))
                    }
                }
            }
        } else {
            for (path, symbol) in workspace.imported_symbol_iter(file, alias) {
                candidates.insert(&symbol.name, Candidate::Symbol(path, symbol));
            }
        }
    } else {
        for (path, symbol) in workspace.file_symbol_iter(file) {
            if symbol.receiver.is_none()
                && (!matches!(expected, ExpectedKind::Type)
                    || locals.resolve_at(offset, &symbol.name).is_none())
            {
                candidates.insert(&symbol.name, Candidate::Symbol(path, symbol));
            }
        }
    }
    let start = offset - prefix.len();
    let end = source[offset..]
        .char_indices()
        .find(|(_, ch)| !crate::syntax::is_identifier_continue(*ch))
        .map_or(source.len(), |(byte, _)| offset + byte);
    let edit = EditContext {
        query: context,
        file,
        source,
        locals,
        map: &map,
        span: start..end,
        cursor: offset,
        calls,
        receiver: receiver.as_ref().map(|ty| (ty.scope, ty.text.as_str())),
    };
    let collection_time = collection_started.elapsed();
    let render_started = Instant::now();
    let items = super::members::render_completion_candidates(
        candidates
            .items
            .into_iter()
            .map(|((_, label), candidate)| (label, candidate)),
        &edit,
    );
    if trace {
        eprintln!(
            "completion profile: scope={scope_time:?} receiver={receiver_time:?} collection={collection_time:?} render={:?} items={} total={:?}",
            render_started.elapsed(),
            items.len(),
            started.elapsed()
        );
    }
    // A longer prefix changes fuzzy ranking and can bring back candidates
    // beyond the current bounded result set. Ask the client to requery.
    Ok(json!({"isIncomplete":true,"items":items}))
}
pub(super) enum Candidate<'a> {
    Local(&'a Binding),
    Symbol(&'a Path, &'a Symbol),
    Member(MemberDeclaration<'a>),
    Conflict,
}

impl Candidate<'_> {
    fn is_type(&self) -> bool {
        matches!(self, Self::Local(binding) if binding.object_kind.is_type())
            || matches!(self, Self::Symbol(_, symbol) if symbol.is_type())
    }
    pub(super) fn origin(&self) -> Option<&Path> {
        match self {
            Self::Symbol(path, _) => Some(path),
            Self::Member(member) => Some(member.file),
            _ => None,
        }
    }

    pub(super) fn owner_start(&self) -> usize {
        match self {
            Self::Member(member) => member.owner.start,
            _ => 0,
        }
    }
    pub(super) fn kind(&self) -> Option<tower_lsp::lsp_types::CompletionItemKind> {
        use tower_lsp::lsp_types::CompletionItemKind as Kind;
        Some(match self {
            Self::Conflict => return None,
            Self::Local(binding) => {
                if binding.object_kind.is_type() {
                    Kind::CLASS
                } else {
                    Kind::VARIABLE
                }
            }
            Self::Member(member) => {
                if member.kind == crate::syntax::MemberKind::Method {
                    Kind::METHOD
                } else {
                    Kind::FIELD
                }
            }
            Self::Symbol(_, symbol) => {
                if symbol.is_function() {
                    Kind::FUNCTION
                } else if symbol.is_method() {
                    Kind::METHOD
                } else if symbol.is_type() {
                    Kind::CLASS
                } else if symbol.is_const() {
                    Kind::CONSTANT
                } else {
                    Kind::VARIABLE
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
        let score = self.matcher.score(name);
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
                (Candidate::Member(member), Candidate::Member(old))
                    if member.owner.start != old.owner.start || member.file != old.file =>
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
