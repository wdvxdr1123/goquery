//! Identity resolution shared by references, fallback navigation and highlighting.
mod members;
use super::{Binding, Locals, ObjectKind};
use crate::analysis::check::{CachedBinding, Inference, NativeTypeCache, Selection};
use crate::analysis::member::MemberDeclaration;
use crate::analysis::method_set::ReceiverMember;
use crate::analysis::resolve::NameIndex;
use crate::analysis::source::{Name, Role, SourceAnalysis};
use crate::index::IndexedSymbol as Symbol;
use crate::syntax::ExprKind;
use crate::workspace::Workspace;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub(crate) enum Identity<'a> {
    Local(Range<usize>),
    Global(&'a PathBuf, &'a Symbol),
    Member(MemberDeclaration<'a>),
}
type Symbols<'a> = HashMap<&'a str, (&'a PathBuf, &'a Symbol)>;
type SelectorKindMap = HashMap<(usize, String), Option<ObjectKind>>;
type RangeKey = (usize, usize, usize, usize);

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum LiteralContext {
    Explicit(usize),
    Element(usize),
}

/// Bounded by the packages and receiver types actually used by one file query.
pub(crate) struct Resolver<'a> {
    workspace: &'a Workspace,
    index: Option<&'a NameIndex<'a>>,
    package_global_only: bool,
    members_enabled: bool,
    file: &'a Path,
    package: HashMap<&'a str, Option<(&'a PathBuf, &'a Symbol)>>,
    imports: HashMap<String, Symbols<'a>>,
    methods: HashMap<String, Vec<(&'a PathBuf, &'a Symbol)>>,
    members: HashMap<String, Vec<ReceiverMember<'a>>>,
    /// Query-local memos for the two inference walks a semantic-token request
    /// repeats per occurrence: range/initializer binding types and selector
    /// member resolution. Interior mutability lets the read-only classification
    /// fallbacks share the same results within one request.
    binding_types: RefCell<HashMap<(usize, usize), Option<String>>>,
    /// Native (scoped) lexical binding types, shared by every receiver walk in
    /// one candidate file. A binding whose type needs a large declaration
    /// expansion would otherwise be re-inferred by each independent arena.
    scoped_bindings: RefCell<HashMap<(usize, usize), CachedBinding<'a>>>,
    inferred_members: RefCell<HashMap<(usize, usize), Option<Identity<'a>>>>,
    /// Range element types are inferred from the written range expression, which
    /// is independent of the member occurrence; a single query-local memo keeps
    /// the native slot walk from repeating for each field selector.
    range_types: RefCell<HashMap<RangeKey, Option<String>>>,
    /// Cheap field-chain identities keyed by the root binding and the written
    /// selector text, so repeated `x.y.z` spellings share one walk.
    field_chains: RefCell<HashMap<(usize, String), Option<Identity<'a>>>>,
    /// Full selector resolutions keyed by the root binding and the written
    /// selector text. Repeated `t.Kind`/`t.t.Hash` spellings in one scope share
    /// the parsed/inferred result, including unresolved outcomes.
    selector_identities: RefCell<HashMap<(usize, String), Option<Identity<'a>>>>,
    /// Native selector kinds for unresolved selectors, shared with the
    /// classification fallback under the same key.
    selector_kinds: RefCell<SelectorKindMap>,
    /// Type-directed field/expression facts, shared by sibling elided literals.
    /// Expression-key identities still resolve at each occurrence's own scope.
    literal_keys: RefCell<HashMap<(LiteralContext, String), members::LiteralKey<'a>>>,
    /// Range/call-chain native projection shared by every occurrence in the
    /// request; other bindings keep the existing resolver paths.
    native_types: NativeTypeCache,
    trace_once: Cell<bool>,
}

impl<'a> Resolver<'a> {
    /// Read-only semantic views used by classification fallbacks that consult
    /// the native type graph instead of a rendered receiver spelling.
    pub(crate) fn workspace(&self) -> &'a Workspace {
        self.workspace
    }

    pub(crate) fn file(&self) -> &'a Path {
        self.file
    }

    /// Request-local native binding-type memo, shared with classification
    /// fallbacks so repeated receiver inference over one file is computed once.
    pub(crate) fn scoped_bindings(&self) -> &RefCell<HashMap<(usize, usize), CachedBinding<'a>>> {
        &self.scoped_bindings
    }

    pub(crate) fn new(workspace: &'a Workspace, file: &'a Path) -> Self {
        let mut package = HashMap::new();
        for (path, symbol) in workspace.file_symbol_iter(file) {
            package
                .entry(symbol.name.as_str())
                .and_modify(|entry| *entry = None)
                .or_insert(Some((path, symbol)));
        }
        Self {
            workspace,
            index: None,
            package_global_only: false,
            members_enabled: true,
            file,
            package,
            imports: HashMap::new(),
            methods: HashMap::new(),
            members: HashMap::new(),
            binding_types: RefCell::new(HashMap::new()),
            scoped_bindings: RefCell::new(HashMap::new()),
            inferred_members: RefCell::new(HashMap::new()),
            range_types: RefCell::new(HashMap::new()),
            field_chains: RefCell::new(HashMap::new()),
            selector_identities: RefCell::new(HashMap::new()),
            selector_kinds: RefCell::new(HashMap::new()),
            literal_keys: RefCell::new(HashMap::new()),
            native_types: NativeTypeCache::new(),
            trace_once: Cell::new(false),
        }
    }

    pub(crate) fn for_name(
        workspace: &'a Workspace,
        file: &'a Path,
        index: &'a NameIndex<'a>,
    ) -> Self {
        Self {
            workspace,
            file,
            index: Some(index),
            package_global_only: false,
            members_enabled: true,
            package: HashMap::new(),
            imports: HashMap::new(),
            methods: HashMap::new(),
            members: HashMap::new(),
            binding_types: RefCell::new(HashMap::new()),
            scoped_bindings: RefCell::new(HashMap::new()),
            inferred_members: RefCell::new(HashMap::new()),
            range_types: RefCell::new(HashMap::new()),
            field_chains: RefCell::new(HashMap::new()),
            selector_identities: RefCell::new(HashMap::new()),
            selector_kinds: RefCell::new(HashMap::new()),
            literal_keys: RefCell::new(HashMap::new()),
            native_types: NativeTypeCache::new(),
            trace_once: Cell::new(false),
        }
    }

    pub(crate) fn for_reference(
        workspace: &'a Workspace,
        file: &'a Path,
        index: &'a NameIndex<'a>,
        target: &Symbol,
    ) -> Self {
        let mut resolver = Self::for_name(workspace, file, index);
        resolver.package_global_only = target.receiver.is_none();
        resolver.members_enabled = false;
        resolver
    }

    fn unqualified(&self, name: &str) -> Option<(&'a PathBuf, &'a Symbol)> {
        if let Some(index) = self.index {
            debug_assert_eq!(index.name(), name);
            index.unqualified(self.file)
        } else {
            self.package.get(name).copied().flatten()
        }
    }

    pub(crate) fn has_outer_name(&self, name: &str) -> bool {
        self.unqualified(name).is_some() || self.imported_name(name)
    }

    /// Native element type of a `range` binding, rendered in the declaring file.
    /// This keeps range field selectors off the legacy initializer walk.
    fn native_range_text(
        &self,
        analysis: &SourceAnalysis,
        source: &str,
        binding: &crate::analysis::scope::Binding,
    ) -> Option<String> {
        let (span, result, count) = binding.range_initializer()?;
        let key = (span.start, span.end, result, count);
        if let Some(value) = self.range_types.borrow().get(&key) {
            return value.clone();
        }
        let value = Inference::new(self.workspace, self.file, source, &analysis.locals, &[], 0)
            .with_scoped_bindings(&self.scoped_bindings)
            .native_range_slot(span.clone(), result, count)
            .map(|ty| ty.text);
        if self.range_types.borrow().len() < 4_096 {
            self.range_types.borrow_mut().insert(key, value.clone());
        }
        value
    }

    pub(crate) fn imported_name(&self, name: &str) -> bool {
        super::universe::imported_name(self.workspace, self.file, name)
    }

    /// Query-local memo for initializer/range receiver type inference. The same
    /// binding is visited once per member occurrence; the walk is bounded by
    /// the existing inference budgets, not repeated for every occurrence.
    pub(crate) fn binding_type_cached(
        &self,
        file: &Path,
        source: &str,
        locals: &Locals,
        binding: &Binding,
    ) -> Option<String> {
        let key = (binding.selection.start, binding.selection.end);
        if let Some(value) = self.binding_types.borrow().get(&key) {
            return value.clone();
        }
        let value = self.workspace.binding_type(file, source, locals, binding);
        if self.binding_types.borrow().len() < 4_096 {
            self.binding_types.borrow_mut().insert(key, value.clone());
        }
        value
    }

    /// Selector cache key: the root local binding plus the written selector
    /// text. Package-qualified and opaque receivers keep the uncached paths.
    pub(crate) fn selector_cache_key(
        &self,
        analysis: &SourceAnalysis,
        source: &str,
        name: &Name,
    ) -> Option<(usize, String)> {
        let (start, binding_start) = match &name.role {
            Role::ExpressionMember(operand) => {
                let first = crate::syntax::Lexer::new(source.get(operand.start..)?).next()?;
                if first.kind != crate::syntax::TokenKind::Ident {
                    return None;
                }
                let text = source.get(operand.start..operand.start + first.span.end)?;
                let binding = analysis.locals.resolve_at(operand.start, text)?;
                (operand.start, binding.selection.start)
            }
            Role::Member(Some(qualifier)) => {
                let text = source.get(qualifier.clone())?;
                let binding = analysis.locals.resolve_at(qualifier.start, text)?;
                (qualifier.start, binding.selection.start)
            }
            _ => return None,
        };
        Some((binding_start, source.get(start..name.span.end)?.to_owned()))
    }

    pub(crate) fn cached_selector_identity(
        &self,
        key: &(usize, String),
    ) -> Option<Option<Identity<'a>>> {
        self.selector_identities.borrow().get(key).cloned()
    }

    fn store_selector_identity(&self, key: (usize, String), value: &Option<Identity<'a>>) {
        if self.selector_identities.borrow().len() < 4_096 {
            self.selector_identities
                .borrow_mut()
                .insert(key, value.clone());
        }
    }

    pub(crate) fn cached_selector_kind(&self, key: &(usize, String)) -> Option<Option<ObjectKind>> {
        self.selector_kinds.borrow().get(key).copied()
    }

    pub(crate) fn store_selector_kind(&self, key: (usize, String), value: Option<ObjectKind>) {
        if self.selector_kinds.borrow().len() < 4_096 {
            self.selector_kinds.borrow_mut().insert(key, value);
        }
    }

    /// Cheap selector-chain identity: walk written fields from the root local
    /// binding's queried type. Successes are memoized by root binding and text;
    /// calls, indexes and unresolved links keep the general inference fallback.
    fn field_chain_member(
        &self,
        analysis: &SourceAnalysis,
        source: &str,
        span: Range<usize>,
    ) -> Option<Identity<'a>> {
        let text = source.get(span.clone())?;
        if text.len() > 16 * 1024 {
            return None;
        }
        let parsed = crate::syntax::parse_expression(text);
        if !parsed.diagnostics.is_empty() {
            return None;
        }
        let mut fields: Vec<Range<usize>> = Vec::new();
        let mut node = &parsed.nodes[parsed.root.0];
        loop {
            match &node.kind {
                ExprKind::Selector { operand, field } => {
                    fields.push(field.clone());
                    node = &parsed.nodes[operand.0];
                }
                ExprKind::Parenthesized { expression } => {
                    node = &parsed.nodes[expression.0];
                }
                ExprKind::Name => break,
                _ => return None,
            }
            if fields.len() > 32 {
                return None;
            }
        }
        let root_span = span.start + node.span.start..span.start + node.span.end;
        let root_text = source.get(root_span.clone())?;
        let binding = analysis.locals.resolve_at(root_span.start, root_text)?;
        let key = (binding.selection.start, text.to_owned());
        if let Some(value) = self.field_chains.borrow().get(&key) {
            return value.clone();
        }
        let mut base = if binding.range_initializer().is_some() {
            self.native_range_text(analysis, source, binding)?
        } else {
            self.binding_type_cached(self.file, source, &analysis.locals, binding)?
        };
        fields.reverse();
        let (last, intermediates) = fields.split_last()?;
        for intermediate in intermediates {
            let name =
                source.get(span.start + intermediate.start..span.start + intermediate.end)?;
            let declaration = match self.workspace.receiver_member(self.file, &base, name)? {
                ReceiverMember::Declaration(declaration) => declaration,
                ReceiverMember::Method(..) => return None,
            };
            let annotation = self.workspace.member_data(declaration)?.annotation?;
            base = if declaration.file.as_path() == self.file {
                annotation
            } else {
                self.workspace
                    .requalified_type(self.file, declaration.file, &annotation)?
            };
        }
        let name = source.get(span.start + last.start..span.start + last.end)?;
        let identity = match self.workspace.receiver_member(self.file, &base, name)? {
            ReceiverMember::Method(file, symbol) => Identity::Global(file, symbol),
            ReceiverMember::Declaration(declaration) => Identity::Member(declaration),
        };
        if self.field_chains.borrow().len() < 4_096 {
            self.field_chains
                .borrow_mut()
                .insert(key, Some(identity.clone()));
        }
        Some(identity)
    }

    fn imported(&mut self, alias: &str, name: &str) -> Option<(&'a PathBuf, &'a Symbol)> {
        if let Some(index) = self.index {
            debug_assert_eq!(index.name(), name);
            return index.imported(self.file, alias);
        }
        let symbols = self.imports.entry(alias.to_owned()).or_insert_with(|| {
            self.workspace
                .imported_symbol_iter(self.file, alias)
                .map(|(path, symbol)| (symbol.name.as_str(), (path, symbol)))
                .collect()
        });
        symbols.get(name).copied()
    }

    fn method(&mut self, receiver: &str, name: &str) -> Option<(&'a PathBuf, &'a Symbol)> {
        if self.package_global_only {
            return None;
        }
        self.methods
            .entry(receiver.to_owned())
            .or_insert_with(|| self.workspace.receiver_methods(self.file, receiver))
            .iter()
            .find(|(_, symbol)| symbol.name == name)
            .copied()
    }
}

impl<'a> Resolver<'a> {
    pub(crate) fn resolve(
        &mut self,
        analysis: &SourceAnalysis,
        source: &str,
        name: &Name,
    ) -> Option<Identity<'a>> {
        let phase = self.workspace.trace.queries.then(std::time::Instant::now);
        let key = self.selector_cache_key(analysis, source, name);
        if let Some(phase) = phase {
            let elapsed = phase.elapsed();
            if elapsed > std::time::Duration::from_micros(100) {
                eprintln!("resolver selector_key {elapsed:?}");
            }
        }
        if self.workspace.trace.queries && !self.trace_once.get() {
            self.trace_once.set(true);
            eprintln!(
                "resolver locals bindings {} symbol lookup start",
                analysis.locals.bindings.len()
            );
        }
        if let Some(key) = &key
            && let Some(value) = self.cached_selector_identity(key)
        {
            return value;
        }
        let value = self.resolve_uncached(analysis, source, name);
        if let Some(key) = key {
            self.store_selector_identity(key, &value);
        }
        value
    }

    fn resolve_uncached(
        &mut self,
        analysis: &SourceAnalysis,
        source: &str,
        name: &Name,
    ) -> Option<Identity<'a>> {
        let phase = self.workspace.trace.queries.then(std::time::Instant::now);
        if let Some((path, stub)) = self.workspace.files.get_key_value(self.file)
            && let Some(symbol) = stub
                .symbols
                .iter()
                .find(|symbol| symbol.start == name.span.start && symbol.end == name.span.end)
        {
            return Some(Identity::Global(path, symbol));
        }
        if let Some(phase) = phase {
            let elapsed = phase.elapsed();
            if elapsed > std::time::Duration::from_micros(100) {
                eprintln!(
                    "resolver exact_symbol {elapsed:?} symbols {}",
                    self.workspace.files[self.file].symbols.len()
                );
            }
        }
        let spelling = &source[name.span.clone()];
        if spelling == "_" {
            return None;
        }
        match &name.role {
            Role::ExpressionMember(operand) => {
                if self.package_global_only {
                    return None;
                }
                // Complex operands (calls, assertions, indexes) benefit from
                // native projection; plain `a.b` chains already have a cheap
                // field walk and keep the existing paths.
                let nested = source
                    .get(operand.clone())
                    .is_some_and(|text| text.contains('[') || text.contains('('));
                if nested
                    && let Some(member) = crate::analysis::check::native_selector_member(
                        self.workspace,
                        self.file,
                        source,
                        &analysis.locals,
                        &self.native_types,
                        &(operand.start..name.span.end),
                    )
                {
                    return Some(match member {
                        ReceiverMember::Method(path, symbol) => Identity::Global(path, symbol),
                        ReceiverMember::Declaration(declaration) => Identity::Member(declaration),
                    });
                }
                if let Some(identity) =
                    self.field_chain_member(analysis, source, operand.start..name.span.end)
                {
                    return Some(identity);
                }
                self.inferred_member(analysis, source, operand.start..name.span.end)
            }
            Role::Parameter | Role::Result => Some(Identity::Local(name.span.clone())),
            Role::FieldDeclaration(owner) => self
                .field_declaration(*owner, spelling)
                .map(Identity::Member),
            Role::LiteralKey(_) | Role::LiteralElementKey(_) => {
                let context = match &name.role {
                    Role::LiteralKey(ty) => LiteralContext::Explicit(ty.start),
                    Role::LiteralElementKey(id) => LiteralContext::Element(*id),
                    _ => unreachable!("literal key role"),
                };
                let key = (context, spelling.to_owned());
                let cached = self.literal_keys.borrow().get(&key).copied();
                let literal = match cached {
                    Some(literal) => literal,
                    None => {
                        let literal = match &name.role {
                            Role::LiteralKey(ty) => {
                                self.literal_key(source, &analysis.locals, ty, spelling)
                            }
                            Role::LiteralElementKey(id) => analysis
                                .literal_element_type(self.workspace, self.file, source, *id)
                                .map_or(members::LiteralKey::Unknown, |(ty, offset)| {
                                    self.literal_key_type(
                                        source,
                                        &analysis.locals,
                                        &ty,
                                        offset,
                                        spelling,
                                    )
                                }),
                            _ => unreachable!("literal key role"),
                        };
                        if self.literal_keys.borrow().len() < 4_096 {
                            self.literal_keys.borrow_mut().insert(key, literal);
                        }
                        literal
                    }
                };
                match literal {
                    members::LiteralKey::Field(member) => Some(Identity::Member(member)),
                    members::LiteralKey::Expression => {
                        if let Some(binding) = analysis.locals.definition_at(&name.span, spelling) {
                            Some(Identity::Local(binding.selection.clone()))
                        } else {
                            self.unqualified(spelling)
                                .map(|(path, symbol)| Identity::Global(path, symbol))
                        }
                    }
                    members::LiteralKey::Unknown => None,
                }
            }
            Role::Namespace
            | Role::Field
            | Role::Ignored
            | Role::BlankDeclaration { .. }
            | Role::Member(None) => None,
            Role::Type(Some(qualifier))
            | Role::EmbeddedType(Some(qualifier))
            | Role::Member(Some(qualifier)) => {
                let text = &source[qualifier.clone()];
                if let Some(binding) = analysis.locals.resolve_at(qualifier.start, text) {
                    if matches!(name.role, Role::Type(_) | Role::EmbeddedType(_)) {
                        return None;
                    }
                    if self.package_global_only {
                        return None;
                    }
                    // Range variables and call-chain intermediate bindings use
                    // the native projection; a missing member falls through to
                    // the existing paths so results stay unchanged.
                    // Only written range operands and call/selector/assertion
                    // initializer chains need native projection; plain values
                    // keep the existing (cheaper) resolver paths.
                    let native_candidate = binding.range_initializer().is_some()
                        || binding.initializer().is_some_and(|(span, _, _)| {
                            source
                                .get(span.clone())
                                .is_some_and(|text| text.contains('(') || text.contains('.'))
                        });
                    if native_candidate {
                        let phase = self.workspace.trace.queries.then(std::time::Instant::now);
                        let projected = crate::analysis::check::native_binding_type(
                            self.workspace,
                            self.file,
                            source,
                            &analysis.locals,
                            &self.native_types,
                            binding,
                        );
                        let projected_time = phase.map(|phase| phase.elapsed());
                        if self.workspace.trace.queries {
                            eprintln!(
                                "resolver native target {spelling} binding {} -> {projected:?}",
                                binding.name
                            );
                        }
                        if let Some(ty) = projected {
                            let phase = self.workspace.trace.queries.then(std::time::Instant::now);
                            let in_scope = self.workspace.receiver_in_scope(
                                self.file,
                                &ty,
                                &analysis.locals,
                                qualifier.start,
                            );
                            let scope_time = phase.map(|phase| phase.elapsed());
                            if in_scope {
                                let phase =
                                    self.workspace.trace.queries.then(std::time::Instant::now);
                                let identity = if self.members_enabled {
                                    self.member(&ty, spelling)
                                } else {
                                    self.method(&ty, spelling)
                                        .map(|(path, symbol)| Identity::Global(path, symbol))
                                };
                                let member_time = phase.map(|phase| phase.elapsed());
                                if self.workspace.trace.queries
                                    && [projected_time, scope_time, member_time]
                                        .into_iter()
                                        .flatten()
                                        .any(|time| time > std::time::Duration::from_millis(1))
                                {
                                    eprintln!(
                                        "resolver native timing {spelling} type {ty} projected {projected_time:?} scope {scope_time:?} member {member_time:?}"
                                    );
                                }
                                if identity.is_some() {
                                    return identity;
                                }
                                // A written, in-scope native receiver type that
                                // has no such member is a negative fact; do not
                                // repeat the legacy binding walk for it.
                                return None;
                            }
                        }
                    }
                    let phase = self.workspace.trace.queries.then(std::time::Instant::now);
                    let range_text = self.native_range_text(analysis, source, binding);
                    if let Some(phase) = phase {
                        let elapsed = phase.elapsed();
                        if elapsed > std::time::Duration::from_micros(300) {
                            eprintln!("resolver native_range_text {elapsed:?} target {spelling}");
                        }
                    }
                    if self.workspace.trace.queries
                        && matches!(spelling, "Mtyp" | "Ifn" | "Tfn" | "Name" | "Methods")
                    {
                        eprintln!(
                            "resolver range spelling {spelling} target {spelling} ty {range_text:?}"
                        );
                    }
                    if let Some(ty) = range_text {
                        let phase = self.workspace.trace.queries.then(std::time::Instant::now);
                        let in_scope = self.workspace.receiver_in_scope(
                            self.file,
                            &ty,
                            &analysis.locals,
                            qualifier.start,
                        );
                        if let Some(phase) = phase {
                            let elapsed = phase.elapsed();
                            if elapsed > std::time::Duration::from_micros(300) {
                                eprintln!(
                                    "resolver receiver_in_scope(range) {elapsed:?} target {spelling}"
                                );
                            }
                        }
                        if !in_scope {
                            return self.inferred_member(
                                analysis,
                                source,
                                qualifier.start..name.span.end,
                            );
                        }
                        let phase = self.workspace.trace.queries.then(std::time::Instant::now);
                        let identity = if self.members_enabled {
                            self.member(&ty, spelling)
                        } else {
                            self.method(&ty, spelling)
                                .map(|(path, symbol)| Identity::Global(path, symbol))
                        };
                        if let Some(phase) = phase {
                            let elapsed = phase.elapsed();
                            if elapsed > std::time::Duration::from_micros(300) {
                                eprintln!(
                                    "resolver member(range) {elapsed:?} type {ty} target {spelling}"
                                );
                            }
                        }
                        return identity;
                    }
                    let phase = self.workspace.trace.queries.then(std::time::Instant::now);
                    let binding_type =
                        self.binding_type_cached(self.file, source, &analysis.locals, binding);
                    if let Some(phase) = phase {
                        let elapsed = phase.elapsed();
                        if elapsed > std::time::Duration::from_micros(300) {
                            eprintln!(
                                "resolver binding_type {elapsed:?} qualifier {} target {}",
                                text, spelling
                            );
                        }
                    }
                    if let Some(ty) = binding_type {
                        let phase = self.workspace.trace.queries.then(std::time::Instant::now);
                        let in_scope = self.workspace.receiver_in_scope(
                            self.file,
                            &ty,
                            &analysis.locals,
                            qualifier.start,
                        );
                        if let Some(phase) = phase {
                            let elapsed = phase.elapsed();
                            if elapsed > std::time::Duration::from_micros(300) {
                                eprintln!(
                                    "resolver receiver_in_scope(binding) {elapsed:?} target {spelling}"
                                );
                            }
                        }
                        if !in_scope {
                            return self.inferred_member(
                                analysis,
                                source,
                                qualifier.start..name.span.end,
                            );
                        }
                        let phase = self.workspace.trace.queries.then(std::time::Instant::now);
                        let identity = if self.members_enabled {
                            self.member(&ty, spelling)
                        } else {
                            self.method(&ty, spelling)
                                .map(|(path, symbol)| Identity::Global(path, symbol))
                        };
                        if let Some(phase) = phase {
                            let elapsed = phase.elapsed();
                            if elapsed > std::time::Duration::from_micros(300) {
                                eprintln!(
                                    "resolver member(binding) {elapsed:?} type {ty} target {spelling}"
                                );
                            }
                        }
                        return identity;
                    }
                    self.inferred_member(analysis, source, qualifier.start..name.span.end)
                } else {
                    let phase = self.workspace.trace.queries.then(std::time::Instant::now);
                    let imported = self.imported(text, spelling);
                    if let Some(phase) = phase {
                        let elapsed = phase.elapsed();
                        if elapsed > std::time::Duration::from_micros(300) {
                            eprintln!(
                                "resolver imported {elapsed:?} alias {text} target {spelling}"
                            );
                        }
                    }
                    if let Some((path, symbol)) = imported {
                        Some(Identity::Global(path, symbol))
                    } else if matches!(name.role, Role::Member(_))
                        && !self.package_global_only
                        && self
                            .workspace
                            .unqualified_symbol(self.file, text)
                            .is_some_and(|(_, symbol)| symbol.is_value())
                    {
                        self.inferred_member(analysis, source, qualifier.start..name.span.end)
                    } else {
                        None
                    }
                }
            }
            Role::Name | Role::Type(None) | Role::EmbeddedType(None) => {
                if let Some(binding) = analysis.locals.definition_at(&name.span, spelling) {
                    return Some(Identity::Local(binding.selection.clone()));
                }
                self.unqualified(spelling)
                    .map(|(p, s)| Identity::Global(p, s))
            }
        }
    }

    fn inferred_member(
        &self,
        analysis: &SourceAnalysis,
        source: &str,
        span: Range<usize>,
    ) -> Option<Identity<'a>> {
        let key = (span.start, span.end);
        if let Some(value) = self.inferred_members.borrow().get(&key) {
            return value.clone();
        }
        let value = self.inferred_member_walk(analysis, source, span);
        if self.inferred_members.borrow().len() < 4_096 {
            self.inferred_members
                .borrow_mut()
                .insert(key, value.clone());
        }
        value
    }

    fn inferred_member_walk(
        &self,
        analysis: &SourceAnalysis,
        source: &str,
        span: Range<usize>,
    ) -> Option<Identity<'a>> {
        let phase = self.workspace.trace.queries.then(std::time::Instant::now);
        let parsed = crate::syntax::parse_expression(source.get(span.clone())?);
        if !parsed.diagnostics.is_empty() {
            return None;
        }
        let inference = Inference::new(
            self.workspace,
            self.file,
            source,
            &analysis.locals,
            &parsed.nodes,
            span.start,
        )
        .with_name_index(self.index)
        .with_scoped_bindings(&self.scoped_bindings);
        let identity = if self.members_enabled {
            inference
                .selection(parsed.root)
                .map(|selection| match selection {
                    Selection::Global(path, symbol) => Identity::Global(path, symbol),
                    Selection::Member(member) => Identity::Member(member),
                })
        } else {
            inference
                .callee(parsed.root)
                .map(|(path, symbol)| Identity::Global(path, symbol))
        };
        if let Some(phase) = phase {
            let elapsed = phase.elapsed();
            if elapsed > std::time::Duration::from_millis(20) {
                eprintln!(
                    "resolver inferred_member {elapsed:?} span {:?} text {:?} -> {}",
                    span,
                    source
                        .get(span.clone())
                        .map(|text| &text[..text.len().min(80)]),
                    identity.is_some()
                );
            }
        }
        identity
    }
}
