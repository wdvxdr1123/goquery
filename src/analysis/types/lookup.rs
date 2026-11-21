//! Bounded field/method lookup sharing visibility, promotion and ambiguity checks.
use super::eval::TypeEval;
use super::ty::CheckMode;
use super::{MAX_DEPTH, Method, PromotionIndices, Selection, SelectionKind, Ty, TypeId, TypeStore};
use crate::analysis::engine::obj::{DeclId, MemberDeclId, MethodOrigin, ObjectRef};
use std::path::Path;

#[derive(Clone, Copy)]
pub(in crate::analysis::engine::types) struct ConcreteMethodMemo {
    pub(super) pointer_receiver: bool,
    pub(super) signature: Option<TypeId>,
}

#[derive(Clone)]
struct MemberCandidate<'a> {
    selection: Selection<'a>,
}

pub(super) struct Step<'a> {
    pub(super) ty: TypeId,
    pub(super) indirect: bool,
    pub(super) parent: Option<usize>,
    pub(super) field_index: u16,
    pub(super) owner: Option<DeclId<'a>>,
}

struct InterfaceLookup<'b, 'a> {
    recv: TypeId,
    indirect: bool,
    steps: &'b [Step<'a>],
    index: usize,
    origin: &'b Path,
    name: &'b str,
}

pub(super) fn collect_indices(
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

pub(super) fn has_owner_cycle(
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

/// Standalone field/method selection engine over [`TypeEval`].
pub(crate) struct MethodLookup<'s, 'a> {
    eval: TypeEval<'s, 'a>,
}

impl<'s, 'a> MethodLookup<'s, 'a> {
    pub(crate) fn new(store: &'s mut TypeStore<'a>, mode: CheckMode) -> Self {
        Self {
            eval: TypeEval::new(store, mode),
        }
    }

    #[allow(dead_code)]
    pub(crate) const fn mode(&self) -> CheckMode {
        self.eval.mode()
    }

    pub(super) fn store(&self) -> &TypeStore<'a> {
        self.eval.store()
    }

    fn store_mut(&mut self) -> &mut TypeStore<'a> {
        self.eval.store_mut()
    }

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
            let pointer = self.store_mut().pointer(recv)?;
            candidate = self.lookup_member(pointer, recv, origin, name, false)?;
        }
        if let Some(candidate) = candidate {
            return Some(Some(candidate.selection));
        }
        // Defined pointers permit fields, but must not acquire methods
        // from their underlying pointer or from selector auto-addressing.
        let root = self.store_mut().unalias(recv)?;
        if matches!(self.store().node(root)?, Ty::Named { .. })
            && let Some(pointer) = self.store_mut().underlying(root)
            && matches!(self.store().node(pointer)?, Ty::Pointer(_))
            && let Some(candidate) = self.lookup_member(pointer, recv, origin, name, false)?
            && candidate.selection.kind == SelectionKind::FieldVal
        {
            return Some(Some(candidate.selection));
        }
        Some(None)
    }

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
        let sig = self
            .store()
            .signature_data(candidate.selection.ty)
            .cloned()?;
        let mut arguments = vec![recv];
        arguments.extend_from_slice(
            self.store()
                .arg_range(candidate.selection.ty, sig.parameters),
        );
        let results = self
            .store()
            .arg_range(candidate.selection.ty, sig.results)
            .to_vec();
        let ty = self
            .store_mut()
            .store_signature(arguments, results, sig.variadic)?;
        self.store_mut()
            .copy_signature_names(candidate.selection.ty, ty, true);
        Some(Some(Selection {
            kind: SelectionKind::MethodExpr,
            recv,
            obj: candidate.selection.obj,
            ty,
            indices: candidate.selection.indices,
            indirect: candidate.selection.indirect,
        }))
    }

    fn lookup_interface_methods(
        &mut self,
        iface: TypeId,
        owner: Option<&'a crate::index::IndexedSymbol>,
        ctx: &InterfaceLookup<'_, 'a>,
        count: &mut usize,
        found: &mut Option<MemberCandidate<'a>>,
    ) -> Option<()> {
        let root = self.store_mut().underlying(iface)?;
        let methods = self.store().interface_data(root)?.methods.clone();
        let methods = self.store().methods_slice(root, methods).to_vec();
        for method in methods {
            if method.name == ctx.name
                && (crate::syntax::is_exported_name(ctx.name)
                    || self.store().same_field_package(method.origin, ctx.origin)?)
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
        let store = self.store_mut();
        if let Some(index) = store
            .env_cache
            .pkg_method_index(store.workspace, declaration.file)
        {
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
        let probe = crate::bloom::Fingerprint::new(name.as_bytes());
        store
            .workspace
            .package_files_in_view(declaration.file, store.build_view)
            .filter(move |(_, stub)| stub.identifiers.may_contain_fingerprint(probe))
            .flat_map(|(file, stub)| stub.symbols.iter().map(move |symbol| (file, symbol)))
            .map(|(file, symbol)| DeclId {
                file: file.as_path(),
                symbol,
            })
            .collect()
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
                self.eval.step()?;
                let mut input = self.store_mut().unalias(steps[index].ty)?;
                let pointer = if let Ty::Pointer(element) = *self.store().node(input)? {
                    input = self.store_mut().unalias(element)?;
                    true
                } else {
                    false
                };
                let indirect = steps[index].indirect || pointer;
                if matches!(self.store().node(input)?, Ty::Pointer(_)) {
                    continue;
                }
                if matches!(self.store().node(input)?, Ty::TypeParam(_)) {
                    if pointer {
                        continue;
                    }
                    let lookup = InterfaceLookup {
                        recv: original_recv,
                        indirect,
                        steps: &steps,
                        index,
                        origin,
                        name,
                    };
                    if let Some(set) = self.store_mut().parameter_set(input)
                        && let Some(methods_iface) = set.methods
                    {
                        self.lookup_interface_methods(
                            methods_iface,
                            None,
                            &lookup,
                            &mut count,
                            &mut found,
                        )?;
                    }
                    continue;
                }
                if self.store_mut().interface_constructor(input)? {
                    if pointer {
                        continue;
                    }
                    let owner = self
                        .store()
                        .named_info(input)
                        .map(|(decl, _, _)| decl.symbol);
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
                if let Some((declaration, arguments, false)) = self.store().named_info(input) {
                    self.store_mut().check_named_arguments(input)?;
                    if has_owner_cycle(&steps, steps[index].parent, declaration) {
                        continue;
                    }
                    steps[index].owner = Some(declaration);
                    let arguments = self.store().arguments_slice(input, arguments).to_vec();
                    let candidates = self.concrete_method_candidates(declaration, name);
                    for method_decl in candidates {
                        let file = method_decl.file;
                        let method = method_decl.symbol;
                        if !method.is_method()
                            || method.name != name
                            || !relation
                                && !self.store().workspace.selector_file_visible_in_view(
                                    origin,
                                    file,
                                    self.store().build_view,
                                )
                            || !crate::syntax::is_exported_name(&method.name)
                                && !self.store().same_field_package(file, origin)?
                        {
                            continue;
                        }
                        self.eval.step()?;
                        let receiver = method.receiver.as_deref()?;
                        if receiver != declaration.symbol.name {
                            if let Some((file, symbol)) = self
                                .store()
                                .resolver()
                                .resolve_type_declaration(file, receiver)
                                && !symbol.alias
                                && (file.as_path() != declaration.file
                                    || symbol.start != declaration.symbol.start)
                            {
                                continue;
                            }
                            let receiver = self.store_mut().parse(file, receiver)?;
                            let mut receiver = self.store_mut().unalias(receiver)?;
                            if let Ty::Pointer(element) = *self.store().node(receiver)? {
                                receiver = self.store_mut().unalias(element)?;
                            }
                            let Some((candidate, _, false)) = self.store().named_info(receiver)
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
                            && let Some(&memo) =
                                self.store().decl.concrete_methods.get(&method_decl)
                            && ((!indirect && memo.pointer_receiver) || memo.signature.is_some())
                        {
                            (memo.pointer_receiver, memo.signature)
                        } else {
                            let header = {
                                let store = self.store_mut();
                                store
                                    .env_cache
                                    .method_header(store.workspace, method_decl)?
                            };
                            if header.receiver_names.len() != arguments.len() {
                                return None;
                            }
                            let compute =
                                |store: &mut TypeStore<'a>| -> Option<(bool, Option<TypeId>)> {
                                    let receiver = header.parsed.receiver.first()?;
                                    let environment: Vec<_> = header
                                        .receiver_names
                                        .iter()
                                        .zip(&arguments)
                                        .map(|(name, ty)| (&header.text[name.clone()], *ty))
                                        .collect();
                                    let receiver = store.parse_in(
                                        file,
                                        &header.text[receiver.annotation.clone()],
                                        &environment,
                                    )?;
                                    let receiver = store.unalias(receiver)?;
                                    let pointer_receiver =
                                        matches!(store.node(receiver)?, Ty::Pointer(_));
                                    let need_sig = !pointer_receiver || indirect;
                                    let signature = if need_sig {
                                        Some(store.signature(
                                            file,
                                            &header.text,
                                            Some(header.start),
                                            &header.parsed,
                                            &environment,
                                            0,
                                        )?)
                                    } else {
                                        None
                                    };
                                    Some((pointer_receiver, signature))
                                };
                            let (pointer_receiver, signature) = if arguments.is_empty() {
                                self.store_mut().with_decl_alloc(compute)?
                            } else {
                                compute(self.store_mut())?
                            };
                            if arguments.is_empty()
                                && signature.is_none_or(|s| s.is_decl_safe())
                                && self.store().decl.concrete_methods.len()
                                    < self.store().decl.pool.node_limit
                            {
                                self.store_mut().decl.concrete_methods.insert(
                                    method_decl,
                                    ConcreteMethodMemo {
                                        pointer_receiver,
                                        signature,
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
                let root = self.store_mut().underlying(input)?;
                let Some((field_origin, fields)) = self
                    .store()
                    .struct_data(root)
                    .map(|d| (d.origin, d.fields.clone()))
                else {
                    continue;
                };
                let fields_vec = self.store().fields_slice(root, fields)?.to_vec();
                for (ord, field) in fields_vec.into_iter().enumerate() {
                    self.eval.step()?;
                    let leaf_index = u16::try_from(ord).ok()?;
                    if field.name == name
                        && (crate::syntax::is_exported_name(&field.name)
                            || self.store().same_field_package(field_origin, origin)?)
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
                        if steps.len() >= self.store().active_node_limit() {
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
}

impl<'a> TypeStore<'a> {
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
                let methods = self.interface_data(interface)?.methods.clone();
                self.methods_slice(interface, methods)
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
        TypeEval::from_store(self)
            .method_lookup()
            .lookup_field_or_method(recv, addressable, origin, name)
    }

    /// A documentation receiver denotes its declaration's formal instance;
    /// keep lookup in the same arena and return only the canonical object.
    pub(crate) fn declaration_member(
        &mut self,
        declaration: DeclId<'a>,
        consumer: &Path,
        name: &str,
    ) -> Option<ObjectRef<'a>> {
        if name.len() > super::MAX_BYTES {
            return None;
        }
        TypeEval::from_store(self).with_projection(CheckMode::Symbolic, |eval| {
            let receiver = eval.store_mut().declaration_type(declaration)?;
            Some(
                eval.method_lookup()
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
        TypeEval::from_store(self)
            .method_lookup()
            .lookup_method_expr(recv, origin, name)
    }

    pub(super) fn lookup_method(
        &mut self,
        source: TypeId,
        required: &Method<'_>,
    ) -> Option<Option<TypeId>> {
        let Some(candidate) = TypeEval::from_store(self).method_lookup().lookup_member(
            source,
            source,
            required.origin,
            &required.name,
            true,
        )?
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
        let Some(candidate) = TypeEval::from_store(self)
            .method_lookup()
            .lookup_member(recv, recv, origin, name, true)?
        else {
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
        TypeEval::from_store(self).with_projection(CheckMode::Symbolic, |eval| {
            let receiver = eval.store_mut().implementation_receiver(declaration)?;
            let selection = eval
                .method_lookup()
                .lookup_member(receiver, receiver, origin, name, true)??
                .selection;
            (selection.kind == SelectionKind::MethodVal).then_some(selection.obj)
        })
    }
}
