//! Request-local implementation search. Closed source/type trees are not cached.
use super::candidates::Candidates;
use super::{Context, SymbolTarget};
use crate::analysis::resolve::MemberDescription;
use crate::analysis::resolve::{MemberId, MemberIds};
use crate::analysis::sema::{DeclId, MethodOrigin, NamedObject, ObjectRef, Semantics};
use crate::index::IndexedSymbol as Symbol;
use crate::syntax::TypeConstructor;
use crate::syntax::{Lexer, MemberKind, TokenKind};
use crate::workspace::Workspace;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Query-local memo for named-type spellings: (file, package, name) → target.
type ResolveCache<'a> = HashMap<(&'a Path, &'a str, &'a str), Option<(&'a PathBuf, &'a Symbol)>>;

struct MethodSet<'a> {
    file: &'a Path,
    owner: &'a Symbol,
    abstract_: bool,
    members: BTreeMap<MemberId<'a>, NamedObject<'a>>,
}

type Key<'a> = (&'a Path, usize, ObjectRef<'a>);
type ShapeKey<'a> = ObjectRef<'a>;

struct Search<'a> {
    workspace: &'a Workspace,
    matches: HashMap<(Key<'a>, Key<'a>), Option<bool>>,
    shapes: HashMap<ShapeKey<'a>, Option<MethodShape>>,
    member_ids: MemberIds<'a>,
}

impl<'a> MethodSet<'a> {
    fn new(
        sema: &mut Semantics<'a>,
        workspace: &'a Workspace,
        file: &'a Path,
        owner: &'a Symbol,
        required: Option<&[&str]>,
    ) -> Self {
        let (abstract_, members) = sema
            .relations()
            .implementation_members_matching(
                DeclId {
                    file,
                    symbol: owner,
                },
                required,
            )
            .unwrap_or_default();
        let members = members
            .into_iter()
            .filter(|member| required.is_none_or(|required| required.contains(&member.name)))
            .filter_map(|member| {
                let origin = member.file()?;
                Some((
                    crate::analysis::resolve::member_id(workspace, origin, member.name),
                    member,
                ))
            })
            .collect();
        Self {
            file,
            owner,
            abstract_,
            members,
        }
    }

    fn covers(&self, interface: &Self) -> bool {
        interface.abstract_
            && !interface.members.is_empty()
            && interface
                .members
                .keys()
                .all(|id| self.members.contains_key(id))
    }
}

impl<'a> Search<'a> {
    fn signatures_match(
        &mut self,
        sema: &mut Semantics<'_>,
        left: DeclId<'a>,
        left_member: NamedObject<'a>,
        right: DeclId<'a>,
        right_member: NamedObject<'a>,
    ) -> Option<bool> {
        let key = (
            (left.file, left.symbol.start, left_member.object),
            (right.file, right.symbol.start, right_member.object),
        );
        if let Some(result) = self.matches.get(&key) {
            return *result;
        }
        let result = sema.relations().implementation_signatures_match(
            left,
            left_member,
            right,
            right_member,
        );
        if self.matches.len() < 512 {
            self.matches.insert(key, result);
        }
        result
    }

    fn implements(
        &mut self,
        sema: &mut crate::analysis::sema::Semantics<'_>,
        concrete: &MethodSet<'a>,
        interface: &MethodSet<'a>,
    ) -> bool {
        // Exact IDs reject unrelated types without reading their sources. No
        // type-name or direct-import filter is valid for structural matching.
        if !concrete.covers(interface) {
            return false;
        }
        let (concrete_file, concrete_owner) = (concrete.file, concrete.owner);
        let (interface_file, interface_owner) = (interface.file, interface.owner);
        interface.members.iter().all(|(id, required)| {
            let concrete_member = concrete.members[id];
            // A cheap shape mismatch rejects before either side is rendered.
            if let (Some(left), Some(right)) = (self.shape(concrete_member), self.shape(*required))
                && (left.parameters != right.parameters
                    || left.results != right.results
                    || left.variadic != right.variadic)
            {
                return false;
            }
            self.signatures_match(
                sema,
                DeclId {
                    file: concrete_file,
                    symbol: concrete_owner,
                },
                concrete_member,
                DeclId {
                    file: interface_file,
                    symbol: interface_owner,
                },
                *required,
            ) == Some(true)
        })
    }

    /// Query-local method shape: only the parameter/result arity is needed to
    /// reject most candidates before signature identity is rendered.
    fn shape(&mut self, member: NamedObject<'a>) -> Option<MethodShape> {
        let key = member.object;
        if let Some(shape) = self.shapes.get(&key) {
            return *shape;
        }
        let shape = receiver_shape(self.workspace, member);
        if self.shapes.len() < 4096 {
            self.shapes.insert(key, shape);
        }
        shape
    }

    fn interface_member(
        &mut self,
        file: &'a PathBuf,
        owner: &'a Symbol,
        name: &'a str,
    ) -> Option<NamedObject<'a>> {
        let identity = self.member_ids.get(MemberDescription {
            file,
            owner,
            name,
            kind: MemberKind::Method,
            embedded: false,
        })?;
        Some(NamedObject {
            name,
            object: ObjectRef::Method(MethodOrigin::Interface(identity)),
        })
    }
}

/// Checks a plain interface (no embeddings, complete direct members) against the
/// target's method set without materializing the candidate method set.
#[allow(clippy::too_many_arguments)]
fn plain_interface_matches<'a>(
    sema: &mut Semantics<'_>,
    workspace: &'a Workspace,
    search: &mut Search<'a>,
    target: &MethodSet<'a>,
    target_file: &'a Path,
    candidate_file: &'a Path,
    candidate: &'a Symbol,
    members: &'a [String],
) -> bool {
    let Some((file_key, _)) = workspace.files.get_key_value(candidate_file) else {
        return false;
    };
    for name in members {
        let id = crate::analysis::resolve::member_id(workspace, candidate_file, name);
        let Some(expected) = target.members.get(&id) else {
            return false;
        };
        let Some(member) = search.interface_member(file_key, candidate, name.as_str()) else {
            return false;
        };
        if search.signatures_match(
            sema,
            DeclId {
                file: candidate_file,
                symbol: candidate,
            },
            member,
            DeclId {
                file: target_file,
                symbol: target.owner,
            },
            *expected,
        ) != Some(true)
        {
            return false;
        }
    }
    true
}

/// Subtype direction for a plain interface candidate: every target method must
/// be declared directly with a matching signature. `None` means unknown, so the
/// caller falls back to the full method-set merge.
fn plain_interface_implements<'a>(
    sema: &mut Semantics<'_>,
    workspace: &'a Workspace,
    search: &mut Search<'a>,
    target: &MethodSet<'a>,
    candidate_file: &'a Path,
    candidate: &'a Symbol,
    members: &'a [String],
) -> Option<bool> {
    let (file_key, _) = workspace.files.get_key_value(candidate_file)?;
    let mut unknown = false;
    for (target_id, target_member) in target.members.iter() {
        if !members.iter().any(|name| name == target_id.name) {
            return Some(false);
        }
        if crate::analysis::resolve::member_id(workspace, candidate_file, target_id.name)
            != *target_id
        {
            return Some(false);
        }
        let member = search.interface_member(file_key, candidate, target_id.name)?;
        match search.signatures_match(
            sema,
            DeclId {
                file: candidate_file,
                symbol: candidate,
            },
            member,
            DeclId {
                file: target.file,
                symbol: target.owner,
            },
            *target_member,
        ) {
            Some(false) => return Some(false),
            Some(true) => {}
            None => unknown = true,
        }
    }
    if unknown { None } else { Some(true) }
}

impl<'a, View> Context<'a, View> {
    pub(crate) fn type_relations(
        &mut self,
        file: &'a Path,
        target: &'a Symbol,
        supers: bool,
    ) -> Result<Vec<SymbolTarget<'a>>, (i32, String)> {
        self.relations(file, target, supers, None)
    }

    pub(in crate::analysis::query) fn method_relations(
        &mut self,
        file: &'a Path,
        target: &'a Symbol,
        supers: bool,
        name: &'a str,
    ) -> Result<Vec<SymbolTarget<'a>>, (i32, String)> {
        self.relations(file, target, supers, Some(name))
    }

    fn relations(
        &mut self,
        file: &'a Path,
        target: &'a Symbol,
        supers: bool,
        method: Option<&'a str>,
    ) -> Result<Vec<SymbolTarget<'a>>, (i32, String)> {
        let started = Instant::now();
        let target =
            self.with_sema(|sema| MethodSet::new(sema, self.workspace, file, target, None));
        if target.members.is_empty() || !supers && !target.abstract_ {
            return Ok(Vec::new());
        }
        let view = self.resolver();
        let method_packages = method
            .filter(|name| !crate::analysis::resolve::exported(name))
            .and_then(|name| {
                let id = crate::analysis::resolve::member_id(self.workspace, file, name);
                target.members.get(&id)?.file()
            })
            .and_then(|origin| view.dependent_packages(origin));
        let mut search = Search {
            workspace: self.workspace,
            matches: HashMap::new(),
            shapes: HashMap::new(),
            member_ids: MemberIds::new(self.workspace),
        };
        let mut results = BTreeMap::new();
        let package = self.type_package(file);
        // Reject concrete declarations with neither own methods nor embeddings
        // from stubs alone. This is keyed by receiver identity, not by a type's
        // spelling in the query, and cannot reject structural implementations.
        let required: Vec<_> = target.members.keys().map(|id| id.name).collect();
        // A candidate can only provide names declared somewhere in its package
        // (or promoted from a declared embedding). One bounded mask per receiver
        // owner gives a sound negative filter for plain declarations.
        let mask_enabled = !required.is_empty() && required.len() <= 64;
        let full_mask = if mask_enabled {
            u64::MAX >> (64 - required.len())
        } else {
            0
        };
        let collect_phase = Instant::now();
        let mut indexed = true;
        // Packages that shadow a predeclared type name; only those need the
        // expensive universe-name resolution below.
        let mut shadowing: HashSet<(Option<&Path>, &str)> = HashSet::new();
        let mut shadowing_complete = true;
        let mut candidates = Candidates::new(self.resolver());
        let detached_target = self.workspace.is_detached(file);
        // Receiver-name inventories only drive the subtype direction's coverage
        // and method-set narrowing; supertype candidates still need the shadow
        // inventory but not these indexes.
        let inventory = !supers;
        for (path, stub) in &self.workspace.files {
            if self.workspace.is_detached(path) != detached_target
                || !self.workspace.hierarchy_scope(file, path)
            {
                continue;
            }
            for symbol in &stub.symbols {
                if symbol.is_type() && universe_shadow(&symbol.name) {
                    if shadowing.len() < 4_096 {
                        shadowing.insert((path.parent(), stub.package.as_str()));
                    } else {
                        shadowing_complete = false;
                    }
                }
                if !inventory {
                    continue;
                }
                if let Some(receiver) = &symbol.receiver {
                    let owner = (path.parent(), stub.package.as_str(), receiver.as_ref());
                    if mask_enabled
                        && let Some(bit) = required
                            .iter()
                            .position(|name| *name == symbol.name.as_str())
                    {
                        candidates.insert_restricted(
                            path,
                            stub.package.as_str(),
                            symbol,
                            1u64 << bit,
                        );
                        if let Some(mask) = candidates.owner_masks.get_mut(&owner) {
                            *mask |= 1u64 << bit;
                        } else if candidates.owner_masks.len() < 16_384 {
                            candidates.owner_masks.insert(owner, 1u64 << bit);
                        } else {
                            candidates.masks_complete = false;
                        }
                    }
                    if indexed {
                        candidates.receivers.insert(owner);
                        if candidates.receivers.len() > 16_384 {
                            // The complete receiver-owner inventory exceeded its
                            // bound: discard the entire index and continue with
                            // ordinary method-set lookup for the remaining
                            // candidates. Positive name hints stay bounded.
                            indexed = false;
                            candidates.receivers.clear();
                        } else if required.contains(&symbol.name.as_str()) {
                            candidates.matching.insert(owner);
                        }
                        continue;
                    }
                    if required.contains(&symbol.name.as_str())
                        && candidates.matching.len() < 16_384
                    {
                        candidates.matching.insert(owner);
                    }
                }
            }
        }
        let collect_time = collect_phase.elapsed();
        // The complete owner inventory is only a negative candidate filter;
        // native method traversal replaces the former receiver/member index.
        indexed &= inventory;
        let index_time = Duration::ZERO;
        let mut visited = 0;
        let mut sets = 0;
        let mut set_time = Duration::ZERO;
        let mut match_time = Duration::ZERO;
        let mut coverage_time = Duration::ZERO;
        let mut coverage_unknown = 0usize;
        let mut fallback_time = Duration::ZERO;
        let mut fallback_calls = 0usize;
        let mut no_info = 0usize;
        let mut interfaces = 0usize;
        let mut embedded_interfaces = 0usize;
        let mut plain_candidates = 0usize;
        let mut filter_time = Duration::ZERO;
        let mut files_seen = 0usize;
        let mut symbols_seen = 0usize;
        let mut types_seen = 0usize;
        let mut file_time = Duration::ZERO;
        let mut constructor_kinds = BTreeMap::new();
        let mut possible_calls = 0usize;
        let mut possible_time = Duration::ZERO;
        let mut target_shapes: HashMap<&'a str, Option<MethodShape>> = HashMap::new();
        let mut fast_plain = 0usize;
        let mut fast_no_inventory = 0usize;
        let mut fast_rejected = 0usize;
        let mut fast_matched = 0usize;
        let mut sub_iface = 0usize;
        let mut sub_embedded = 0usize;
        let mut sub_target = 0usize;
        let mut sub_incomplete = 0usize;
        let mut sub_none = 0usize;
        let mut fast_find_missing = 0usize;
        let mut fast_unknown = 0usize;
        let mut coverage_full = 0usize;
        let mut empty_sets = 0usize;
        let mut mask_full = 0usize;
        let mut coverage_possible = 0usize;
        let mut coverage_impossible = 0usize;
        let mut shape_none = 0usize;
        let mut fast_mask_full = 0usize;
        // Query-local named-type resolution cache: candidate metadata walks
        // revisit the same unqualified target spellings within a package.
        let mut resolve_cache: ResolveCache<'_> = HashMap::new();
        if self.workspace.trace.queries {
            eprintln!(
                "type hierarchy: {} {} begins",
                target.owner.name,
                if supers { "supertypes" } else { "subtypes" }
            );
        }
        let loop_phase = Instant::now();
        for (path, stub) in &self.workspace.files {
            let file_phase = self.workspace.trace.queries.then(Instant::now);
            if self.workspace.is_detached(path) != detached_target
                || !self.workspace.hierarchy_scope(file, path)
                || method_packages.as_ref().is_some_and(|packages| {
                    view.file_visible(path) && !packages.contains(path, stub)
                })
            {
                continue;
            }
            if self.workspace.trace.queries {
                files_seen += 1;
                symbols_seen += stub.symbols.len();
                if let Some(phase) = file_phase {
                    file_time += phase.elapsed();
                }
            }
            for candidate in stub.symbols.iter().filter(|symbol| symbol.is_type()) {
                if self.workspace.trace.queries {
                    types_seen += 1;
                }
                let filter_phase = self.workspace.trace.queries.then(Instant::now);
                // The indexed path preserves the recorded candidate budget over
                // every declared type; the overflow fallback charges the work
                // budget only for types that survive metadata narrowing.
                if indexed {
                    visited += 1;
                    if visited > 16_384 {
                        return Err((
                            crate::error::REQUEST_FAILED,
                            "type hierarchy exceeds 16384 candidate types".into(),
                        ));
                    }
                }
                if path == file && candidate.start == target.owner.start {
                    continue;
                }
                if self.workspace.trace.queries && supers {
                    match &candidate.type_info {
                        None => {
                            no_info += 1;
                            if no_info <= 5 {
                                eprintln!(
                                    "type hierarchy no_info: {} {}",
                                    path.display(),
                                    candidate.name
                                );
                            }
                        }
                        Some(info) => {
                            if info.is_interface() {
                                interfaces += 1;
                                if !info.embedded.is_empty() {
                                    embedded_interfaces += 1;
                                }
                            }
                        }
                    }
                }
                let leaf = candidate.leaf
                    || candidate.type_info.as_ref().is_some_and(|info| {
                        info.embeddings_complete
                            && !info.is_interface()
                            && info.target.is_none()
                            && info.embedded.is_empty()
                    });
                if !supers
                    && indexed
                    && leaf
                    && !candidates.receivers.contains(&(
                        path.parent(),
                        stub.package.as_str(),
                        candidate.name.as_str(),
                    ))
                {
                    continue;
                }
                if candidate.alias {
                    continue;
                }
                if candidate
                    .type_info
                    .as_ref()
                    .and_then(|info| info.target.as_ref())
                    .is_some_and(|target| target.alias)
                {
                    continue;
                }
                if supers {
                    // Declaration flags already proved the written underlying
                    // constructor: only interfaces can be supertypes of a concrete
                    // query. Named/unknown headers stay candidates and continue
                    // through the exact metadata walk.
                    if self.workspace.trace.queries {
                        *constructor_kinds.entry(candidate.constructor).or_insert(0) += 1;
                    }
                    match candidate.constructor {
                        TypeConstructor::Interface => {}
                        TypeConstructor::Unknown | TypeConstructor::Named => {
                            let phase = self.workspace.trace.queries.then(Instant::now);
                            let possible = possible_interface(
                                self.workspace,
                                path,
                                candidate,
                                &mut resolve_cache,
                                &shadowing,
                                shadowing_complete,
                            );
                            if let Some(phase) = phase {
                                possible_calls += 1;
                                possible_time += phase.elapsed();
                            }
                            if !possible {
                                continue;
                            }
                        }
                        _ => continue,
                    }
                }
                // Every proved direct interface method is necessary. Unknown
                // inventories remain candidates and still undergo full merging.
                if supers
                    && candidate.type_info.as_ref().is_some_and(|info| {
                        info.is_interface()
                            && info.embeddings_complete
                            && info.members.iter().any(|name| {
                                !target
                                    .members
                                    .contains_key(&crate::analysis::resolve::member_id(
                                        self.workspace,
                                        path,
                                        name,
                                    ))
                            })
                    })
                {
                    continue;
                }
                if !supers {
                    let restricted = if mask_enabled {
                        candidates.restricted_mask(
                            path,
                            stub.package.as_str(),
                            candidate.name.as_str(),
                        )
                    } else {
                        None
                    };
                    let needs_coverage = match restricted {
                        Some(mask) if mask & full_mask == full_mask => {
                            mask_full += 1;
                            false
                        }
                        Some(_)
                            if plain_declaration(candidate) || isolated_constructor(candidate) =>
                        {
                            continue;
                        }
                        _ => true,
                    };
                    if needs_coverage {
                        let coverage_phase = self.workspace.trace.queries.then(Instant::now);
                        let required_coverage = if mask_enabled {
                            candidates
                                .coverage(
                                    self.workspace,
                                    path,
                                    candidate,
                                    &required,
                                    0,
                                    &shadowing,
                                    shadowing_complete,
                                )
                                .map(|coverage| coverage & full_mask == full_mask)
                        } else {
                            None
                        };
                        if let Some(phase) = coverage_phase {
                            coverage_time += phase.elapsed();
                            if required_coverage.is_none() {
                                coverage_unknown += 1;
                            }
                        }
                        let possible = match required_coverage {
                            Some(possible) => possible,
                            None => {
                                coverage_impossible += 1;
                                let fallback_phase =
                                    self.workspace.trace.queries.then(Instant::now);
                                let possible = if indexed {
                                    candidates.may_match(self.workspace, path, candidate, &required)
                                } else {
                                    candidates.may_match_ordinary(
                                        self.workspace,
                                        path,
                                        candidate,
                                        &required,
                                    )
                                };
                                if let Some(phase) = fallback_phase {
                                    fallback_time += phase.elapsed();
                                    fallback_calls += 1;
                                }
                                possible
                            }
                        };
                        if !possible {
                            continue;
                        }
                        coverage_possible += 1;
                        coverage_full += 1;
                    }
                }
                if let Some(phase) = filter_phase {
                    filter_time += phase.elapsed();
                }
                // A plain declaration exposes only its own receiver methods (the
                // pointer method set is the union of both receiver forms). Check
                // the small required-name set directly instead of materializing a
                // method set for every same-named dependency type.
                let mut fast_match = None;
                if self.workspace.trace.queries && !supers {
                    match candidate.type_info.as_ref() {
                        None => sub_none += 1,
                        Some(info) => {
                            if info.is_interface() {
                                sub_iface += 1;
                            }
                            if !info.embedded.is_empty() {
                                sub_embedded += 1;
                            }
                            if info.target.is_some() {
                                sub_target += 1;
                            }
                            if !info.embeddings_complete {
                                sub_incomplete += 1;
                            }
                        }
                    }
                }
                // A plain interface's method set is its complete direct member
                // list; compare required methods directly for the subtype
                // direction instead of merging a method set per candidate.
                if !supers
                    && !indexed
                    && let Some(info) = candidate.type_info.as_ref()
                    && info.is_interface()
                    && info.embeddings_complete
                    && info.embedded.is_empty()
                    && !info.members.is_empty()
                {
                    match self.with_sema(|sema| {
                        plain_interface_implements(
                            sema,
                            self.workspace,
                            &mut search,
                            &target,
                            path,
                            candidate,
                            info.members.as_slice(),
                        )
                    }) {
                        Some(true) => {
                            fast_matched += 1;
                            fast_match = Some(true);
                        }
                        Some(false) => {
                            fast_rejected += 1;
                            continue;
                        }
                        None => {}
                    }
                }
                if fast_match.is_none() && !supers && !indexed {
                    fast_plain += 1;
                    if mask_enabled
                        && candidates
                            .restricted_mask(path, stub.package.as_str(), candidate.name.as_str())
                            .is_some_and(|mask| mask & full_mask == full_mask)
                    {
                        fast_mask_full += 1;
                    }
                    let package = self
                        .workspace
                        .files
                        .get(path)
                        .map(|stub| stub.package.as_str());
                    let restricted = package.and_then(|package| {
                        candidates.restricted_methods(path, package, candidate.name.as_str())
                    });
                    let owned;
                    let inventory: Option<&[(&PathBuf, &Symbol)]> = match restricted {
                        Some(methods) => Some(methods),
                        None => {
                            owned = candidates.package_methods(
                                self.workspace,
                                path,
                                candidate.name.as_str(),
                                &required,
                            );
                            owned.as_deref()
                        }
                    };
                    match inventory {
                        Some(methods) => {
                            let mut rejected = false;
                            let mut unknown = false;
                            let mut exact = true;
                            for ((target_id, target_member), &name) in
                                target.members.iter().zip(required.iter())
                            {
                                let Some((method_file, method)) = methods
                                    .iter()
                                    .find(|(_, symbol)| symbol.name == name)
                                    .copied()
                                else {
                                    if plain_declaration(candidate)
                                        || isolated_constructor(candidate)
                                    {
                                        // No promotion path exists: a plain
                                        // declaration or a slice/map/func/chan/
                                        // pointer/array underlying type can only
                                        // expose its own receiver methods.
                                        rejected = true;
                                    } else {
                                        // A promoted or embedded method could
                                        // still provide this name; keep the full
                                        // merge.
                                        fast_find_missing += 1;
                                        unknown = true;
                                    }
                                    break;
                                };
                                if crate::analysis::resolve::member_id(
                                    self.workspace,
                                    method_file,
                                    name,
                                ) != *target_id
                                {
                                    rejected = true;
                                    break;
                                }
                                let target_shape =
                                    *target_shapes.entry(name).or_insert_with(|| {
                                        receiver_shape(self.workspace, *target_member)
                                    });
                                match (
                                    method_shape(self.workspace, method_file, method),
                                    target_shape,
                                ) {
                                    (Some(candidate_shape), Some(target_shape)) => {
                                        if candidate_shape.parameters != target_shape.parameters
                                            || candidate_shape.results != target_shape.results
                                            || candidate_shape.variadic != target_shape.variadic
                                        {
                                            rejected = true;
                                            break;
                                        }
                                    }
                                    _ => {
                                        shape_none += 1;
                                        fast_unknown += 1;
                                        unknown = true;
                                        break;
                                    }
                                }
                                let matched = self.with_sema(|sema| {
                                    search.signatures_match(
                                        sema,
                                        DeclId {
                                            file: path,
                                            symbol: candidate,
                                        },
                                        NamedObject {
                                            name: &method.name,
                                            object: ObjectRef::Method(MethodOrigin::Concrete(
                                                DeclId {
                                                    file: method_file,
                                                    symbol: method,
                                                },
                                            )),
                                        },
                                        DeclId {
                                            file: target.file,
                                            symbol: target.owner,
                                        },
                                        *target_member,
                                    )
                                });
                                if matched != Some(true) {
                                    exact = false;
                                    break;
                                }
                            }
                            if rejected && !unknown {
                                fast_rejected += 1;
                                continue;
                            }
                            if exact && !unknown && !rejected {
                                fast_matched += 1;
                                fast_match = Some(true);
                            }
                        }
                        None => fast_no_inventory += 1,
                    }
                }
                if !indexed {
                    visited += 1;
                    if visited > 16_384 {
                        return Err((
                            crate::error::REQUEST_FAILED,
                            "type hierarchy exceeds 16384 candidate types".into(),
                        ));
                    }
                }
                let plain_interface = supers
                    && candidate.type_info.as_ref().is_some_and(|info| {
                        info.is_interface()
                            && info.embeddings_complete
                            && info.embedded.is_empty()
                            && !info.members.is_empty()
                    });
                if plain_interface && self.workspace.trace.queries {
                    plain_candidates += 1;
                }
                let phase = self.workspace.trace.queries.then(Instant::now);
                let candidate_set = (!plain_interface && fast_match.is_none()).then(|| {
                    self.with_sema(|sema| {
                        MethodSet::new(
                            sema,
                            self.workspace,
                            path,
                            candidate,
                            (!supers).then_some(required.as_slice()),
                        )
                    })
                });
                if let Some(phase) = phase {
                    sets += 1;
                    set_time += phase.elapsed();
                }
                if candidate_set
                    .as_ref()
                    .is_some_and(|set| set.members.is_empty())
                {
                    empty_sets += 1;
                    continue;
                }
                let phase = self.workspace.trace.queries.then(Instant::now);
                let matches = match fast_match {
                    Some(matched) => matched,
                    None => match (&candidate_set, supers) {
                        // For a private method query the package filter already
                        // narrowed the search. Check IDs here and let the native
                        // proof below compare each signature once.
                        (Some(set), true) if method_packages.is_some() => target.covers(set),
                        (Some(set), false) if method_packages.is_some() => set.covers(&target),
                        (Some(set), true) => {
                            self.with_sema(|sema| search.implements(sema, &target, set))
                        }
                        (Some(set), false) => {
                            self.with_sema(|sema| search.implements(sema, set, &target))
                        }
                        // A plain interface's method set is exactly its complete
                        // direct member list; compare the target's methods directly
                        // instead of materializing another method set.
                        (None, _) => {
                            let members = candidate
                                .type_info
                                .as_ref()
                                .map(|info| info.members.as_slice())
                                .unwrap_or_default();
                            self.with_sema(|sema| {
                                plain_interface_matches(
                                    sema,
                                    self.workspace,
                                    &mut search,
                                    &target,
                                    file,
                                    path,
                                    candidate,
                                    members,
                                )
                            })
                        }
                    },
                };
                let matches = matches
                    && self.with_sema(|sema| {
                        let target = DeclId {
                            file: target.file,
                            symbol: target.owner,
                        };
                        let candidate = DeclId {
                            file: path,
                            symbol: candidate,
                        };
                        let (concrete, interface) = if supers {
                            (target, candidate)
                        } else {
                            (candidate, target)
                        };
                        sema.relations().implementation_matches(concrete, interface) == Some(true)
                    });
                if let Some(phase) = phase {
                    match_time += phase.elapsed();
                }
                if matches {
                    let detail = self.type_package(path);
                    results.insert(
                        (
                            detail != package,
                            detail,
                            &candidate.name,
                            path.as_path(),
                            candidate.start,
                        ),
                        SymbolTarget {
                            file: path,
                            symbol: candidate,
                        },
                    );
                    if results.len() > 1024 {
                        return Err((
                            crate::error::REQUEST_FAILED,
                            "type hierarchy exceeds 1024 results".into(),
                        ));
                    }
                }
            }
        }
        let loop_time = loop_phase.elapsed();
        if self.workspace.trace.queries {
            eprintln!(
                "type hierarchy: {visited} candidate types in {:?}; {sets} method sets in {set_time:?}; matching in {match_time:?}; index {index_time:?}; collect {collect_time:?}; loop {loop_time:?}; coverage {coverage_time:?} ({coverage_unknown} unknown); fallback {fallback_time:?} ({fallback_calls}); no_info {no_info}; interfaces {interfaces}; embedded_interfaces {embedded_interfaces}; plain {plain_candidates}; filter {filter_time:?}; files {files_seen}/{symbols_seen} symbols, {types_seen} types, filewalk {file_time:?}; constructors {constructor_kinds:?} possible {possible_calls} in {possible_time:?}; fast {fast_plain}/{fast_no_inventory}/{fast_rejected}/{fast_matched}; sub {sub_iface} iface, {sub_embedded} embedded, {sub_target} target, {sub_incomplete} incomplete, {sub_none} no_info; miss {fast_find_missing}/{fast_unknown}; coverage calls {} unknowns {} resolutions {} ({} underlying, {} declaration; qualified {} fast/{} slow; unqualified {} fast/{} slow) hits {} full {coverage_full} inventories {} empty_sets {empty_sets} mask_full {mask_full} cov_possible {coverage_possible} cov_impossible {coverage_impossible} shape_none {shape_none} fast_mask_full {fast_mask_full}; {} cached method relations",
                started.elapsed(),
                candidates.coverage_calls,
                candidates.coverage_unknowns,
                candidates.resolution_calls,
                candidates.underlying_calls,
                candidates.declaration_calls,
                candidates.qualified_fast,
                candidates.qualified_slow,
                candidates.unqualified_fast,
                candidates.unqualified_slow,
                candidates.resolution_hits,
                candidates.inventories,
                search.matches.len()
            );
        }
        Ok(results.into_values().collect())
    }
}

/// Follow only known named indirections. Unresolved/opaque shapes remain
/// candidates; an incomplete shape cannot prove a negative structural result.
fn possible_interface<'a>(
    workspace: &'a Workspace,
    mut file: &'a Path,
    mut symbol: &'a Symbol,
    cache: &mut ResolveCache<'a>,
    shadows: &HashSet<(Option<&'a Path>, &'a str)>,
    shadows_complete: bool,
) -> bool {
    for _ in 0..64 {
        if symbol.constructor == TypeConstructor::Interface {
            return true;
        }
        if !matches!(
            symbol.constructor,
            TypeConstructor::Unknown | TypeConstructor::Named
        ) {
            // Written struct/map/chan/func/pointer/array/slice underlying types
            // cannot be interfaces.
            return false;
        }
        let Some(info) = &symbol.type_info else {
            return true;
        };
        if info.is_interface() {
            return true;
        }
        let Some(target) = &info.target else {
            return false;
        };
        // Constructor spellings cannot name an interface type. This is a
        // metadata-only proof from the written underlying type, so it cannot
        // hide a structural interface implemented by the candidate.
        let spelling = target.name.trim_start();
        match Lexer::new(spelling).next().map(|token| token.kind) {
            Some(TokenKind::LBrack | TokenKind::Map | TokenKind::Chan) => return false,
            Some(TokenKind::Func | TokenKind::Struct | TokenKind::Arrow | TokenKind::Mul) => {
                return false;
            }
            Some(TokenKind::Interface) => return true,
            _ => {}
        }
        // Unqualified predeclared names resolve to the universe unless some
        // declaration in this package shadows that spelling. Dot imports cannot
        // contribute unexported (lowercase) names, so the collected package set
        // is a complete negative proof.
        let name = target.name.trim();
        if shadows_complete
            && !name.contains('.')
            && let Some(stub) = workspace.files.get(file)
            && !shadows.contains(&(file.parent(), stub.package.as_str()))
        {
            if matches!(name, "any" | "error" | "comparable") {
                return true;
            }
            if builtin(name) {
                return false;
            }
        }
        let Some((origin, next)) = resolve_named(workspace, file, &target.name, cache) else {
            return !builtin(&target.name);
        };
        file = origin;
        symbol = next;
    }
    true
}

/// Resolves one unqualified target spelling with a query-local memo. Import
/// aliases stay per-file; unqualified names resolve inside the current package.
fn resolve_named<'a>(
    workspace: &'a Workspace,
    file: &'a Path,
    name: &'a str,
    cache: &mut ResolveCache<'a>,
) -> Option<(&'a PathBuf, &'a Symbol)> {
    if name.contains('.') {
        return crate::analysis::resolve::resolve_type_declaration(workspace, file, name);
    }
    let package = workspace.files.get(file)?.package.as_str();
    let directory = file.parent()?;
    let key = (directory, package, name);
    if let Some(value) = cache.get(&key) {
        return *value;
    }
    let value = crate::analysis::resolve::resolve_type_declaration(workspace, file, name);
    cache.insert(key, value);
    value
}

fn universe_shadow(name: &str) -> bool {
    builtin(name) || matches!(name, "any" | "error" | "comparable")
}

#[derive(Clone, Copy)]
struct MethodShape {
    parameters: usize,
    results: usize,
    variadic: bool,
}

/// A declaration with no underlying named type and no embedded members exposes
/// exactly the receiver methods declared on it.
fn plain_declaration(symbol: &Symbol) -> bool {
    symbol.type_info.as_ref().is_some_and(|info| {
        info.target.is_none() && info.embedded.is_empty() && !info.is_interface()
    })
}

/// Non-aggregate underlying constructors cannot embed fields, so they have no
/// promotion path; only their own receiver methods exist.
fn isolated_constructor(symbol: &Symbol) -> bool {
    symbol.type_info.is_none()
        && matches!(
            symbol.constructor,
            TypeConstructor::Slice
                | TypeConstructor::Array
                | TypeConstructor::Map
                | TypeConstructor::Chan
                | TypeConstructor::Func
                | TypeConstructor::Pointer
        )
}

fn method_shape(workspace: &Workspace, file: &Path, method: &Symbol) -> Option<MethodShape> {
    let declaration = method.declaration()?;
    let end = method.body().map_or(declaration.end, |body| body.start);
    let header = workspace.source_slice(file, declaration.start..end)?;
    shape(&crate::syntax::parse_signature(&header))
}

fn receiver_shape(workspace: &Workspace, member: NamedObject<'_>) -> Option<MethodShape> {
    if let Some((file, method)) = member.method_source(workspace) {
        method_shape(workspace, file.as_path(), method)
    } else {
        let declaration = member.member(workspace)?;
        let data = declaration.data(workspace)?;
        let annotation = data.annotation.as_deref()?;
        // Interface method annotations are written without the `func` keyword.
        let header = if declaration.kind == MemberKind::Method {
            format!("func {annotation}")
        } else {
            annotation.to_owned()
        };
        shape(&crate::syntax::parse_literal_signature(&header))
    }
}

fn shape(parsed: &crate::syntax::Signature) -> Option<MethodShape> {
    if !parsed.diagnostics.is_empty() {
        return None;
    }
    Some(MethodShape {
        parameters: parsed.parameters.len(),
        results: parsed.results.len(),
        variadic: parsed
            .parameters
            .last()
            .is_some_and(|parameter| parameter.variadic),
    })
}

fn builtin(name: &str) -> bool {
    matches!(
        name,
        "bool"
            | "byte"
            | "rune"
            | "string"
            | "int"
            | "int8"
            | "int16"
            | "int32"
            | "int64"
            | "uint"
            | "uint8"
            | "uint16"
            | "uint32"
            | "uint64"
            | "uintptr"
            | "float32"
            | "float64"
            | "complex64"
            | "complex128"
    )
}
