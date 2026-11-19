//! Bounded field/method lookup shares visibility, promotion and ambiguity checks.
use super::constraints::Constraint;
use super::{MAX_DEPTH, Method, PromotionIndices, Selection, SelectionKind, Ty, TypeArena, TypeId};
use crate::analysis::obj::{DeclId, MemberDeclId, MethodOrigin, ObjectRef};
use crate::analysis::scope as locals;
use crate::workspace::Workspace;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct MemberName<'a> {
    name: String,
    package: Option<(&'a Path, &'a str)>,
}

const MAX_PKG_METHOD_INDICES: usize = 64;
const MAX_PKG_METHODS: usize = 4096;

pub(super) struct PkgMethodIndex<'a> {
    by_recv_and_name: HashMap<(&'a str, &'a str), Vec<DeclId<'a>>>,
    alias_methods_by_name: HashMap<&'a str, Vec<DeclId<'a>>>,
}

#[derive(Clone, Copy)]
pub(super) struct ConcreteMethodMemo {
    pub(super) pointer_receiver: bool,
    pub(super) signature: Option<TypeId>,
    pub(super) arena_len: usize,
}

impl<'a> PkgMethodIndex<'a> {
    fn build(
        workspace: &'a Workspace,
        decl_file: &'a Path,
        view: crate::workspace::BuildView<'a>,
    ) -> Option<Self> {
        let mut alias_names: HashSet<&'a str> = HashSet::new();
        let mut files: Vec<(&'a PathBuf, &'a crate::index::IndexedFile)> = Vec::new();
        for (file, stub) in workspace.package_files_in_view(decl_file, view) {
            for sym in &stub.symbols {
                if sym.is_type() && sym.alias {
                    alias_names.insert(sym.name.as_str());
                }
            }
            files.push((file, stub));
        }

        let mut by_recv_and_name: HashMap<(&'a str, &'a str), Vec<DeclId<'a>>> = HashMap::new();
        let mut alias_methods_by_name: HashMap<&'a str, Vec<DeclId<'a>>> = HashMap::new();
        let mut count = 0usize;
        for (file, stub) in files {
            let has_dot = stub.imports.iter().any(|import| import.is_dot());
            for method in &stub.symbols {
                if !method.is_method() {
                    continue;
                }
                count += 1;
                if count > MAX_PKG_METHODS {
                    return None;
                }
                let decl = DeclId {
                    file: file.as_path(),
                    symbol: method,
                };
                match method.receiver.as_deref() {
                    Some(recv) => {
                        by_recv_and_name
                            .entry((recv, method.name.as_str()))
                            .or_default()
                            .push(decl);
                        if has_dot || alias_names.contains(recv) {
                            alias_methods_by_name
                                .entry(method.name.as_str())
                                .or_default()
                                .push(decl);
                        }
                    }
                    None => {
                        alias_methods_by_name
                            .entry(method.name.as_str())
                            .or_default()
                            .push(decl);
                    }
                }
            }
        }
        Some(Self {
            by_recv_and_name,
            alias_methods_by_name,
        })
    }
}

#[derive(Clone)]
struct MemberCandidate<'a> {
    selection: Selection<'a>,
}

struct Step<'a> {
    ty: TypeId,
    indirect: bool,
    parent: Option<usize>,
    field_index: u16,
    owner: Option<DeclId<'a>>,
}

struct InterfaceLookup<'b, 'a> {
    recv: TypeId,
    indirect: bool,
    steps: &'b [Step<'a>],
    index: usize,
    origin: &'b Path,
    name: &'b str,
}

fn collect_indices(
    steps: &[Step<'_>],
    mut index: usize,
    leaf: Option<u16>,
) -> Option<PromotionIndices> {
    let mut reversed = Vec::new();
    if let Some(leaf) = leaf {
        reversed.push(leaf);
    }
    while let Some(parent) = steps[index].parent {
        reversed.push(steps[index].field_index);
        index = parent;
    }
    reversed.reverse();
    PromotionIndices::from_slice(&reversed)
}

impl<'a> TypeArena<'a> {
    /// The declaring receiver reached by an already-proven method selection.
    pub(crate) fn selected_method_receiver(&mut self, selection: &Selection<'a>) -> Option<TypeId> {
        let path = self.embedded_field_path(selection.recv, selection.indices.as_slice())?;
        let reached = path.last().map_or(selection.recv, |(_, ty)| *ty);
        let root = self.receiver_base(reached)?;
        match selection.obj {
            ObjectRef::Method(MethodOrigin::Concrete(method)) => {
                let (owner, arguments, _) = self.named_declaration(root)?;
                let (_, pointer) =
                    self.instantiate_named_method(method.file, method.symbol, owner, &arguments)?;
                if pointer {
                    self.pointer(root)
                } else {
                    Some(root)
                }
            }
            ObjectRef::Method(MethodOrigin::Interface(member)) if member.builtin => {
                self.builtin_type(member.file, "error")
            }
            ObjectRef::Method(MethodOrigin::Interface(member)) => {
                let interface = self.underlying(root)?;
                let Ty::Interface { methods, .. } = self.nodes[usize::from(interface.0)].clone()
                else {
                    return None;
                };
                self.methods[methods]
                    .iter()
                    .find(|method| {
                        method.origin == member.file && method.selection == member.selection
                    })?
                    .declared_on
            }
            _ => None,
        }
    }

    /// Unified `go/types` field/method lookup returning a full `Selection<'a>`,
    /// including `ObjectRef<'a>`, promotion index path, and `indirect` flag.
    pub(crate) fn lookup_field_or_method(
        &mut self,
        recv: TypeId,
        addressable: bool,
        origin: &Path,
        name: &str,
    ) -> Option<Option<Selection<'a>>> {
        if name == "_" {
            return Some(None);
        }
        let mut candidate = self.lookup_member(recv, recv, origin, name, false)?;
        if candidate.is_none() && addressable {
            let pointer = self.pointer(recv)?;
            candidate = self.lookup_member(pointer, recv, origin, name, false)?;
        }
        if let Some(candidate) = candidate {
            return Some(Some(candidate.selection));
        }
        // Defined pointers permit fields, but must not acquire methods
        // from their underlying pointer or from selector auto-addressing.
        let root = self.unalias(recv)?;
        if matches!(self.nodes[usize::from(root.0)], Ty::Named { .. })
            && let Some(pointer) = self.underlying(root)
            && matches!(self.nodes[usize::from(pointer.0)], Ty::Pointer(_))
            && let Some(candidate) = self.lookup_member(pointer, recv, origin, name, false)?
            && candidate.selection.kind == SelectionKind::FieldVal
        {
            return Some(Some(candidate.selection));
        }
        Some(None)
    }

    /// A documentation receiver denotes its declaration's formal instance;
    /// keep lookup in the same arena and return only the canonical object.
    pub(crate) fn declaration_member(
        &mut self,
        declaration: crate::analysis::obj::DeclId<'a>,
        consumer: &Path,
        name: &str,
    ) -> Option<crate::analysis::obj::ObjectRef<'a>> {
        if name.len() > super::MAX_BYTES {
            return None;
        }
        self.with_projection(true, |arena| {
            let receiver = arena.declaration_type(declaration)?;
            Some(
                arena
                    .lookup_field_or_method(receiver, true, consumer, name)??
                    .obj,
            )
        })
    }

    /// Unified `go/types` method expression lookup (`T.M` / `(*T).M`).
    /// Does not apply selector auto-addressing.
    pub(crate) fn lookup_method_expr(
        &mut self,
        recv: TypeId,
        origin: &Path,
        name: &str,
    ) -> Option<Option<Selection<'a>>> {
        if name == "_" {
            return Some(None);
        }
        let Some(candidate) = self.lookup_member(recv, recv, origin, name, false)? else {
            return Some(None);
        };
        if candidate.selection.kind != SelectionKind::MethodVal {
            return Some(None);
        }
        let Ty::Signature {
            parameters,
            results,
            variadic,
        } = self.nodes[usize::from(candidate.selection.ty.0)].clone()
        else {
            return None;
        };
        let mut arguments = vec![recv];
        arguments.extend_from_slice(&self.arguments[parameters]);
        let results = self.arguments[results].to_vec();
        let ty = self.store_signature(arguments, results, variadic)?;
        self.copy_signature_names(candidate.selection.ty, ty, true);
        Some(Some(Selection {
            kind: SelectionKind::MethodExpr,
            recv,
            obj: candidate.selection.obj,
            ty,
            indices: candidate.selection.indices,
            indirect: candidate.selection.indirect,
        }))
    }

    /// Enumerates all visible, unambiguous fields and methods accessible on `recv`.
    pub(crate) fn accessible_members(
        &mut self,
        recv: TypeId,
        addressable: bool,
        origin: &Path,
    ) -> Option<Vec<(String, Selection<'a>)>> {
        let root = self.unalias(recv)?;
        if matches!(self.nodes.get(usize::from(root.0))?, Ty::Named { .. })
            && let Some(pointer) = self.underlying(root)
            && matches!(self.nodes.get(usize::from(pointer.0))?, Ty::Pointer(_))
        {
            return self.collect_accessible_members(pointer, recv, origin, false, true, false);
        }
        self.collect_accessible_members(recv, recv, origin, addressable, false, false)
    }

    pub(super) fn lookup_method(
        &mut self,
        source: TypeId,
        required: &Method<'_>,
    ) -> Option<Option<TypeId>> {
        // Relations inspect declaration method sets, including external test
        // packages. Client selector visibility is a separate lookup policy.
        let Some(candidate) =
            self.lookup_member(source, source, required.origin, &required.name, true)?
        else {
            return Some(None);
        };
        Some(
            (candidate.selection.kind == SelectionKind::MethodVal)
                .then_some(candidate.selection.ty),
        )
    }

    /// Method-set queries share native promotion/collision and package rules;
    /// unlike selectors, they do not filter declaration files by consumer view.
    pub(super) fn implementation_method(
        &mut self,
        recv: TypeId,
        origin: &Path,
        name: &str,
    ) -> Option<Option<TypeId>> {
        self.implementation_selection(recv, origin, name)
            .map(|selection| selection.map(|selection| selection.ty))
    }

    fn implementation_selection(
        &mut self,
        recv: TypeId,
        origin: &Path,
        name: &str,
    ) -> Option<Option<Selection<'a>>> {
        let Some(candidate) = self.lookup_member(recv, recv, origin, name, true)? else {
            return Some(None);
        };
        Some((candidate.selection.kind == SelectionKind::MethodVal).then_some(candidate.selection))
    }

    pub(crate) fn implementation_method_object(
        &mut self,
        declaration: DeclId<'a>,
        origin: &Path,
        name: &str,
    ) -> Option<ObjectRef<'a>> {
        self.with_projection(true, |arena| {
            let receiver = arena.implementation_receiver(declaration)?;
            Some(arena.implementation_selection(receiver, origin, name)??.obj)
        })
    }

    fn lookup_interface_methods(
        &mut self,
        iface: TypeId,
        owner: Option<&'a crate::index::IndexedSymbol>,
        ctx: &InterfaceLookup<'_, 'a>,
        count: &mut usize,
        found: &mut Option<MemberCandidate<'a>>,
    ) -> Option<()> {
        let root = self.underlying(iface)?;
        let Ty::Interface { methods, .. } = self.nodes[usize::from(root.0)].clone() else {
            return None;
        };
        for method in &self.methods[methods] {
            if method.name == ctx.name
                && (crate::syntax::is_exported_name(ctx.name)
                    || self.same_field_package(method.origin, ctx.origin)?)
            {
                *count += 1;
                let indices = collect_indices(ctx.steps, ctx.index, None)?;
                *found = Some(MemberCandidate {
                    selection: Selection {
                        kind: SelectionKind::MethodVal,
                        recv: ctx.recv,
                        obj: ObjectRef::Method(MethodOrigin::Interface(MemberDeclId {
                            file: method.origin,
                            owner,
                            selection: method.selection,
                            embedded: false,
                            builtin: method.builtin,
                        })),
                        ty: method.signature,
                        indices,
                        indirect: ctx.indirect,
                    },
                });
                break;
            }
        }
        Some(())
    }

    fn concrete_method_candidates(
        &mut self,
        declaration: DeclId<'a>,
        name: &str,
    ) -> Vec<DeclId<'a>> {
        let pkg_key = declaration.file.parent().and_then(|dir| {
            let stub = self.workspace.files.get(declaration.file)?;
            Some((
                dir,
                stub.package.as_str(),
                self.workspace.is_detached(declaration.file),
            ))
        });
        if let Some(key) = pkg_key {
            if !self.pkg_methods.contains_key(&key)
                && self.pkg_methods.len() < MAX_PKG_METHOD_INDICES
            {
                let built =
                    PkgMethodIndex::build(self.workspace, declaration.file, self.build_view);
                self.pkg_methods.insert(key, built);
            }
            if let Some(Some(index)) = self.pkg_methods.get(&key) {
                let direct = index
                    .by_recv_and_name
                    .get(&(declaration.symbol.name.as_str(), name));
                let aliased = index.alias_methods_by_name.get(name);
                let decl_name = declaration.symbol.name.as_str();
                let mut out = Vec::new();
                if let Some(direct) = direct {
                    out.extend_from_slice(direct);
                }
                if let Some(aliased) = aliased {
                    for &decl in aliased {
                        if decl.symbol.receiver.as_deref() != Some(decl_name) {
                            out.push(decl);
                        }
                    }
                }
                return out;
            }
        }
        let probe = crate::bloom::Fingerprint::new(name.as_bytes());
        self.workspace
            .package_files_in_view(declaration.file, self.build_view)
            .filter(move |(_, stub)| stub.identifiers.may_contain_fingerprint(probe))
            .flat_map(|(file, stub)| stub.symbols.iter().map(move |symbol| (file, symbol)))
            .map(|(file, symbol)| DeclId {
                file: file.as_path(),
                symbol,
            })
            .collect()
    }

    /// Receiver spellings only reject impossible candidates. Alias and dot-import
    /// receivers still require exact native receiver identity during instantiation.
    fn concrete_owner_candidates(&self, declaration: DeclId<'a>) -> Option<Vec<DeclId<'a>>> {
        let mut aliases = HashSet::new();
        for (_, stub) in self
            .workspace
            .package_files_in_view(declaration.file, self.build_view)
        {
            for symbol in &stub.symbols {
                if symbol.is_type() && symbol.alias {
                    aliases.insert(symbol.name.as_str());
                    if aliases.len() > MAX_PKG_METHODS {
                        return None;
                    }
                }
            }
        }
        let mut candidates = Vec::new();
        for (file, stub) in self
            .workspace
            .package_files_in_view(declaration.file, self.build_view)
        {
            let dot = stub.imports.iter().any(|import| import.is_dot());
            for symbol in &stub.symbols {
                if !symbol.is_method() || symbol.name == "_" {
                    continue;
                }
                if symbol.receiver.as_deref().is_some_and(|receiver| {
                    receiver != declaration.symbol.name && !dot && !aliases.contains(receiver)
                }) {
                    continue;
                }
                if candidates.len() == MAX_PKG_METHODS {
                    return None;
                }
                candidates.push(DeclId { file, symbol });
            }
        }
        Some(candidates)
    }

    fn lookup_member(
        &mut self,
        source: TypeId,
        original_recv: TypeId,
        origin: &Path,
        name: &str,
        relation: bool,
    ) -> Option<Option<MemberCandidate<'a>>> {
        let mut steps = vec![Step {
            ty: source,
            indirect: false,
            parent: None,
            field_index: 0,
            owner: None,
        }];
        let mut level = vec![0];
        for _ in 0..MAX_DEPTH {
            let mut next = Vec::new();
            let mut count = 0;
            let mut found = None;
            for index in level {
                self.step()?;
                let mut input = self.unalias(steps[index].ty)?;
                let pointer = if let Ty::Pointer(element) = self.nodes[usize::from(input.0)] {
                    input = self.unalias(element)?;
                    true
                } else {
                    false
                };
                let indirect = steps[index].indirect || pointer;
                if matches!(self.nodes[usize::from(input.0)], Ty::Pointer(_)) {
                    continue;
                }
                if matches!(self.nodes[usize::from(input.0)], Ty::Parameter(_)) {
                    if pointer {
                        continue;
                    }
                    let constraint = self.parameter_constraint(input).cloned();
                    let lookup = InterfaceLookup {
                        recv: original_recv,
                        indirect,
                        steps: &steps,
                        index,
                        origin,
                        name,
                    };
                    match constraint {
                        Some(Constraint::Exact(bound)) if self.interface_constructor(bound)? => {
                            self.lookup_interface_methods(
                                bound, None, &lookup, &mut count, &mut found,
                            )?;
                        }
                        Some(Constraint::Set(set)) => {
                            if let Some(methods_iface) = set.methods {
                                self.lookup_interface_methods(
                                    methods_iface,
                                    None,
                                    &lookup,
                                    &mut count,
                                    &mut found,
                                )?;
                            }
                        }
                        _ => {}
                    }
                    continue;
                }
                if self.interface_constructor(input)? {
                    if pointer {
                        continue;
                    }
                    let owner = match self.nodes[usize::from(input.0)] {
                        Ty::Named { declaration, .. } => Some(declaration.symbol),
                        _ => None,
                    };
                    let lookup = InterfaceLookup {
                        recv: original_recv,
                        indirect,
                        steps: &steps,
                        index,
                        origin,
                        name,
                    };
                    self.lookup_interface_methods(input, owner, &lookup, &mut count, &mut found)?;
                    continue;
                }
                let previous_count = count;
                if let Ty::Named {
                    declaration,
                    arguments,
                } = self.nodes[usize::from(input.0)].clone()
                {
                    self.check_named_arguments(input)?;
                    let mut ancestor = steps[index].parent;
                    let mut cycle = false;
                    while let Some(parent) = ancestor {
                        if steps[parent].owner.is_some_and(|previous| {
                            previous.file == declaration.file
                                && previous.symbol.start == declaration.symbol.start
                        }) {
                            cycle = true;
                            break;
                        }
                        ancestor = steps[parent].parent;
                    }
                    if cycle {
                        continue;
                    }
                    steps[index].owner = Some(declaration);
                    let arguments = self.arguments[arguments.range()].to_vec();
                    let candidates = self.concrete_method_candidates(declaration, name);
                    for method_decl in candidates {
                        let file = method_decl.file;
                        let method = method_decl.symbol;
                        if !method.is_method()
                            || method.name != name
                            || !relation
                                && !self.workspace.selector_file_visible_in_view(
                                    origin,
                                    file,
                                    self.build_view,
                                )
                            || !crate::syntax::is_exported_name(&method.name)
                                && !self.same_field_package(file, origin)?
                        {
                            continue;
                        }
                        self.step()?;
                        let receiver = method.receiver.as_deref()?;
                        if receiver != declaration.symbol.name {
                            if let Some((file, symbol)) =
                                self.resolver().resolve_type_declaration(file, receiver)
                                && !symbol.alias
                                && (file.as_path() != declaration.file
                                    || symbol.start != declaration.symbol.start)
                            {
                                continue;
                            }
                            let receiver = self.parse(file, receiver)?;
                            let mut receiver = self.unalias(receiver)?;
                            if let Ty::Pointer(element) = self.nodes[usize::from(receiver.0)] {
                                receiver = self.unalias(element)?;
                            }
                            let Ty::Named {
                                declaration: candidate,
                                ..
                            } = self.nodes[usize::from(receiver.0)]
                            else {
                                continue;
                            };
                            if candidate.file != declaration.file
                                || candidate.symbol.start != declaration.symbol.start
                            {
                                continue;
                            }
                        }
                        let (pointer_receiver, sig_opt) = if arguments.is_empty()
                            && let Some(&memo) = self.concrete_methods.get(&method_decl)
                            && ((!indirect && memo.pointer_receiver) || memo.signature.is_some())
                        {
                            (memo.pointer_receiver, memo.signature)
                        } else {
                            let span = method.declaration()?;
                            let end = method.body().map_or(span.end, |body| body.start);
                            let header = self.workspace.source_slice(file, span.start..end)?;
                            let parsed = crate::syntax::parse_signature(&header);
                            if !parsed.diagnostics.is_empty() || !parsed.type_parameters.is_empty()
                            {
                                return None;
                            }
                            let receiver = parsed.receiver.first()?;
                            let names = locals::receiver_parameters(&header, &receiver.annotation);
                            if names.len() != arguments.len() {
                                return None;
                            }
                            let environment: Vec<_> = names
                                .iter()
                                .zip(&arguments)
                                .map(|(name, ty)| (&header[name.clone()], *ty))
                                .collect();
                            let receiver = self.parse_in(
                                file,
                                &header[receiver.annotation.clone()],
                                &environment,
                            )?;
                            let receiver = self.unalias(receiver)?;
                            let pointer_receiver =
                                matches!(self.nodes[usize::from(receiver.0)], Ty::Pointer(_));
                            let need_sig = !pointer_receiver || indirect;
                            let signature = if need_sig {
                                Some(self.signature(
                                    file,
                                    &header,
                                    Some(span.start),
                                    &parsed,
                                    &environment,
                                    0,
                                )?)
                            } else {
                                None
                            };
                            if arguments.is_empty() && self.concrete_methods.len() < self.node_limit
                            {
                                self.concrete_methods.insert(
                                    method_decl,
                                    ConcreteMethodMemo {
                                        pointer_receiver,
                                        signature,
                                        arena_len: if signature.is_some() {
                                            self.nodes.len()
                                        } else {
                                            0
                                        },
                                    },
                                );
                            }
                            (pointer_receiver, signature)
                        };
                        count += 1;
                        if (!pointer_receiver || indirect)
                            && let Some(signature) = sig_opt
                        {
                            let indices = collect_indices(&steps, index, None)?;
                            found = Some(MemberCandidate {
                                selection: Selection {
                                    kind: SelectionKind::MethodVal,
                                    recv: original_recv,
                                    obj: ObjectRef::Method(MethodOrigin::Concrete(method_decl)),
                                    ty: signature,
                                    indices,
                                    indirect,
                                },
                            });
                        }
                    }
                }
                // A declaration's own matching method hides all deeper methods;
                // only expand a concrete body if promotion is still needed.
                if count > previous_count {
                    continue;
                }
                let root = self.underlying(input)?;
                let Ty::Struct {
                    origin: field_origin,
                    fields,
                } = self.nodes[usize::from(root.0)].clone()
                else {
                    continue;
                };
                for (ord, field_index) in fields.enumerate() {
                    let field = self.fields[field_index].clone();
                    self.step()?;
                    let leaf_index = u16::try_from(ord).ok()?;
                    if field.name == name
                        && (crate::syntax::is_exported_name(&field.name)
                            || self.same_field_package(field_origin, origin)?)
                    {
                        count += 1;
                        let indices = collect_indices(&steps, index, Some(leaf_index))?;
                        found = Some(MemberCandidate {
                            selection: Selection {
                                kind: SelectionKind::FieldVal,
                                recv: original_recv,
                                obj: ObjectRef::Field(MemberDeclId {
                                    file: field_origin,
                                    owner: steps[index].owner.map(|d| d.symbol),
                                    selection: field.selection,
                                    embedded: field.embedded,
                                    builtin: false,
                                }),
                                ty: field.ty,
                                indices,
                                indirect,
                            },
                        });
                    }
                    if field.embedded {
                        if steps.len() >= self.node_limit {
                            return None;
                        }
                        next.push(steps.len());
                        steps.push(Step {
                            ty: field.ty,
                            indirect,
                            parent: Some(index),
                            field_index: leaf_index,
                            owner: None,
                        });
                    }
                }
            }
            if count > 0 {
                return Some(if count == 1 { found } else { None });
            }
            if next.is_empty() {
                return Some(None);
            }
            level = next;
        }
        None
    }

    fn member_name(
        &self,
        name: &str,
        origin: &'a Path,
        bytes: &mut usize,
    ) -> Option<MemberName<'a>> {
        *bytes = bytes.checked_add(name.len())?;
        if *bytes > super::MAX_BYTES {
            return None;
        }
        let package = if crate::syntax::is_exported_name(name) {
            None
        } else {
            Some((
                origin.parent()?,
                self.workspace.files.get(origin)?.package.as_str(),
            ))
        };
        Some(MemberName {
            name: name.to_owned(),
            package,
        })
    }

    /// Implementation sets include private package identities and test methods;
    /// source-selector visibility is a separate policy over the same traversal.
    pub(crate) fn implementation_member_set(
        &mut self,
        declaration: DeclId<'a>,
        required: Option<&[&str]>,
    ) -> Option<(bool, Vec<(String, ObjectRef<'a>)>)> {
        self.with_projection(true, |arena| {
            let recv = arena.implementation_receiver(declaration)?;
            let abstract_ = arena.interface_constructor(recv)?;
            let base = arena.receiver_base(recv)?;
            if arena.named_declaration(base).is_some()
                && arena.form(base)? == super::TypeForm::Pointer
            {
                return Some((false, Vec::new()));
            }
            // Public requirements need only exact native selections for those
            // names. Private names may denote multiple declaring packages, so
            // their complete package-qualified inventory remains necessary.
            if let Some(required) = required.filter(|names| {
                names
                    .iter()
                    .all(|name| crate::syntax::is_exported_name(name))
            }) {
                if required.len() > super::MAX_NODES
                    || required
                        .iter()
                        .try_fold(0usize, |bytes, name| bytes.checked_add(name.len()))?
                        > super::MAX_BYTES
                {
                    return None;
                }
                let mut members = Vec::new();
                for name in required {
                    let Some(candidate) =
                        arena.lookup_member(recv, recv, declaration.file, name, true)?
                    else {
                        continue;
                    };
                    if matches!(candidate.selection.obj, ObjectRef::Method(_)) {
                        members.push(((*name).to_owned(), candidate.selection.obj));
                    }
                }
                return Some((abstract_, members));
            }
            let members = arena.collect_accessible_members(
                recv,
                recv,
                declaration.file,
                false,
                false,
                true,
            )?;
            Some((
                abstract_,
                members
                    .into_iter()
                    .filter_map(|(name, selection)| {
                        matches!(selection.obj, ObjectRef::Method(_))
                            .then_some((name, selection.obj))
                    })
                    .collect(),
            ))
        })
    }

    fn collect_accessible_members(
        &mut self,
        source: TypeId,
        original_recv: TypeId,
        origin: &Path,
        addressable: bool,
        fields_only: bool,
        relation: bool,
    ) -> Option<Vec<(String, Selection<'a>)>> {
        let mut name_bytes = 0usize;
        let mut resolved: BTreeMap<MemberName<'a>, Option<Selection<'a>>> = BTreeMap::new();
        let mut steps = vec![Step {
            ty: source,
            indirect: false,
            parent: None,
            field_index: 0,
            owner: None,
        }];
        let mut level = vec![0];
        let unaliased_source = self.unalias(source)?;
        let root_pointer = matches!(
            self.nodes.get(usize::from(unaliased_source.0))?,
            Ty::Pointer(_)
        );
        let can_auto_addr = addressable && !fields_only && !root_pointer;
        for _ in 0..MAX_DEPTH {
            let mut next = Vec::new();
            let mut level_map: BTreeMap<MemberName<'a>, Option<Selection<'a>>> = BTreeMap::new();
            for index in level {
                self.step()?;
                let mut input = self.unalias(steps[index].ty)?;
                let pointer = if let Ty::Pointer(element) = self.nodes[usize::from(input.0)] {
                    input = self.unalias(element)?;
                    true
                } else {
                    false
                };
                let indirect = steps[index].indirect || pointer;
                if matches!(self.nodes[usize::from(input.0)], Ty::Pointer(_)) {
                    continue;
                }
                if !fields_only && matches!(self.nodes[usize::from(input.0)], Ty::Parameter(_)) {
                    if pointer {
                        continue;
                    }
                    let iface = match self.parameter_constraint(input).cloned() {
                        Some(Constraint::Exact(bound)) if self.interface_constructor(bound)? => {
                            Some(bound)
                        }
                        Some(Constraint::Set(set)) => set.methods,
                        _ => None,
                    };
                    if let Some(iface) = iface {
                        self.collect_interface_members(
                            iface,
                            None,
                            original_recv,
                            indirect,
                            &steps,
                            index,
                            origin,
                            relation,
                            &mut name_bytes,
                            &resolved,
                            &mut level_map,
                        )?;
                    }
                    continue;
                }
                if !fields_only && self.interface_constructor(input)? {
                    if pointer {
                        continue;
                    }
                    let owner = match self.nodes[usize::from(input.0)] {
                        Ty::Named { declaration, .. } => Some(declaration.symbol),
                        _ => None,
                    };
                    self.collect_interface_members(
                        input,
                        owner,
                        original_recv,
                        indirect,
                        &steps,
                        index,
                        origin,
                        relation,
                        &mut name_bytes,
                        &resolved,
                        &mut level_map,
                    )?;
                    continue;
                }
                if !fields_only
                    && let Ty::Named {
                        declaration,
                        arguments,
                    } = self.nodes[usize::from(input.0)].clone()
                {
                    self.check_named_arguments(input)?;
                    if has_owner_cycle(&steps, steps[index].parent, declaration) {
                        continue;
                    }
                    steps[index].owner = Some(declaration);
                    let arguments = self.arguments[arguments.range()].to_vec();
                    let methods = self.concrete_owner_candidates(declaration)?;
                    for DeclId {
                        file,
                        symbol: method,
                    } in methods
                    {
                        if !relation
                            && (!self.workspace.selector_file_visible_in_view(
                                origin,
                                file,
                                self.build_view,
                            ) || !crate::syntax::is_exported_name(&method.name)
                                && !self.same_field_package(file, origin)?)
                        {
                            continue;
                        }
                        self.step()?;
                        let Some((signature, pointer_receiver)) =
                            self.instantiate_named_method(file, method, declaration, &arguments)
                        else {
                            continue;
                        };
                        let candidate = if !pointer_receiver || indirect || can_auto_addr {
                            let Some(indices) = collect_indices(&steps, index, None) else {
                                continue;
                            };
                            Some(Selection {
                                kind: SelectionKind::MethodVal,
                                recv: original_recv,
                                obj: ObjectRef::Method(MethodOrigin::Concrete(DeclId {
                                    file,
                                    symbol: method,
                                })),
                                ty: signature,
                                indices,
                                indirect: indirect || pointer_receiver,
                            })
                        } else {
                            None
                        };
                        record_level_candidate(
                            &resolved,
                            &mut level_map,
                            self.member_name(&method.name, file, &mut name_bytes)?,
                            candidate,
                        );
                    }
                }
                let root = self.underlying(input)?;
                let Ty::Struct {
                    origin: field_origin,
                    fields,
                } = self.nodes[usize::from(root.0)].clone()
                else {
                    continue;
                };
                for (ord, field_index) in fields.enumerate() {
                    let field = self.fields[field_index].clone();
                    self.step()?;
                    let leaf_index = u16::try_from(ord).ok()?;
                    if field.name != "_"
                        && (relation
                            || crate::syntax::is_exported_name(&field.name)
                            || self.same_field_package(field_origin, origin)?)
                    {
                        let indices = collect_indices(&steps, index, Some(leaf_index))?;
                        let selection = Selection {
                            kind: SelectionKind::FieldVal,
                            recv: original_recv,
                            obj: ObjectRef::Field(MemberDeclId {
                                file: field_origin,
                                owner: steps[index].owner.map(|d| d.symbol),
                                selection: field.selection,
                                embedded: field.embedded,
                                builtin: false,
                            }),
                            ty: field.ty,
                            indices,
                            indirect,
                        };
                        record_level_candidate(
                            &resolved,
                            &mut level_map,
                            self.member_name(&field.name, field_origin, &mut name_bytes)?,
                            Some(selection),
                        );
                    }
                    if field.embedded {
                        if steps.len() >= self.node_limit {
                            return None;
                        }
                        next.push(steps.len());
                        steps.push(Step {
                            ty: field.ty,
                            indirect,
                            parent: Some(index),
                            field_index: leaf_index,
                            owner: None,
                        });
                    }
                }
            }
            resolved.extend(level_map);
            if next.is_empty() {
                return Some(
                    resolved
                        .into_iter()
                        .filter_map(|(name, sel)| sel.map(|s| (name.name, s)))
                        .collect(),
                );
            }
            level = next;
        }
        None
    }

    #[allow(clippy::too_many_arguments)]
    fn collect_interface_members(
        &mut self,
        iface: TypeId,
        owner: Option<&'a crate::index::IndexedSymbol>,
        recv: TypeId,
        indirect: bool,
        steps: &[Step<'a>],
        index: usize,
        origin: &Path,
        relation: bool,
        name_bytes: &mut usize,
        resolved: &BTreeMap<MemberName<'a>, Option<Selection<'a>>>,
        level_map: &mut BTreeMap<MemberName<'a>, Option<Selection<'a>>>,
    ) -> Option<()> {
        let root = self.underlying(iface)?;
        let Ty::Interface { methods, .. } = self.nodes[usize::from(root.0)].clone() else {
            return None;
        };
        let methods = self.methods[methods].to_vec();
        for method in methods {
            if method.name != "_"
                && (relation
                    || crate::syntax::is_exported_name(&method.name)
                    || self.same_field_package(method.origin, origin)?)
            {
                let indices = collect_indices(steps, index, None)?;
                let selection = Selection {
                    kind: SelectionKind::MethodVal,
                    recv,
                    obj: ObjectRef::Method(MethodOrigin::Interface(MemberDeclId {
                        file: method.origin,
                        owner,
                        selection: method.selection,
                        embedded: false,
                        builtin: method.builtin,
                    })),
                    ty: method.signature,
                    indices,
                    indirect,
                };
                let name = self.member_name(&method.name, method.origin, name_bytes)?;
                record_level_candidate(resolved, level_map, name, Some(selection));
            }
        }
        Some(())
    }

    fn instantiate_named_method(
        &mut self,
        file: &'a Path,
        method: &'a crate::index::IndexedSymbol,
        declaration: DeclId<'a>,
        arguments: &[TypeId],
    ) -> Option<(TypeId, bool)> {
        let receiver_str = method.receiver.as_deref()?;
        if receiver_str != declaration.symbol.name {
            if let Some((f, s)) = self.resolver().resolve_type_declaration(file, receiver_str)
                && !s.alias
                && (f.as_path() != declaration.file || s.start != declaration.symbol.start)
            {
                return None;
            }
            let recv_ty = self
                .parse(file, receiver_str)
                .and_then(|t| self.unalias(t))?;
            let recv_base = if let Ty::Pointer(element) = self.nodes[usize::from(recv_ty.0)] {
                self.unalias(element)?
            } else {
                recv_ty
            };
            let Ty::Named {
                declaration: candidate,
                ..
            } = self.nodes[usize::from(recv_base.0)]
            else {
                return None;
            };
            if candidate.file != declaration.file
                || candidate.symbol.start != declaration.symbol.start
            {
                return None;
            }
        }
        let span = method.declaration()?;
        let end = method.body().map_or(span.end, |body| body.start);
        let header = self.workspace.source_slice(file, span.start..end)?;
        let parsed = crate::syntax::parse_signature(&header);
        if !parsed.diagnostics.is_empty() || !parsed.type_parameters.is_empty() {
            return None;
        }
        let receiver = parsed.receiver.first()?;
        let names = locals::receiver_parameters(&header, &receiver.annotation);
        if names.len() != arguments.len() {
            return None;
        }
        let environment: Vec<_> = names
            .iter()
            .zip(arguments)
            .map(|(name, ty)| (&header[name.clone()], *ty))
            .collect();
        let receiver = self.parse_in(file, &header[receiver.annotation.clone()], &environment)?;
        let receiver = self.unalias(receiver)?;
        let pointer_receiver = matches!(self.nodes[usize::from(receiver.0)], Ty::Pointer(_));
        let signature =
            self.signature(file, &header, Some(span.start), &parsed, &environment, 0)?;
        Some((signature, pointer_receiver))
    }
}

fn has_owner_cycle(
    steps: &[Step<'_>],
    mut ancestor: Option<usize>,
    declaration: DeclId<'_>,
) -> bool {
    while let Some(parent) = ancestor {
        if steps[parent].owner.is_some_and(|previous| {
            previous.file == declaration.file && previous.symbol.start == declaration.symbol.start
        }) {
            return true;
        }
        ancestor = steps[parent].parent;
    }
    false
}

fn record_level_candidate<'a>(
    resolved: &BTreeMap<MemberName<'a>, Option<Selection<'a>>>,
    level_map: &mut BTreeMap<MemberName<'a>, Option<Selection<'a>>>,
    name: MemberName<'a>,
    candidate: Option<Selection<'a>>,
) {
    if resolved.contains_key(&name) {
        return;
    }
    match level_map.get_mut(&name) {
        Some(existing) => *existing = None,
        None => {
            level_map.insert(name, candidate);
        }
    }
}
