//! Accessible member-set and implementation method-set enumeration (`methodset.go`).
use super::lookup::{Step, collect_indices, has_owner_cycle};
use super::ty::CheckMode;
use super::{MAX_DEPTH, MethodLookup, Selection, SelectionKind, Ty, TypeArena, TypeId};
use crate::analysis::engine::obj::{DeclId, MemberDeclId, MethodOrigin, ObjectRef};
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

const MAX_PKG_METHODS: usize = 4096;

#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct MemberName<'a> {
    name: String,
    package: Option<(&'a Path, &'a str)>,
}

impl<'a> TypeArena<'a> {
    /// Enumerates all visible, unambiguous fields and methods accessible on `recv`.
    pub(crate) fn accessible_members(
        &mut self,
        recv: TypeId,
        addressable: bool,
        origin: &Path,
    ) -> Option<Vec<(String, Selection<'a>)>> {
        let root = self.unalias(recv)?;
        if matches!(self.node(root)?, Ty::Named { .. })
            && let Some(pointer) = self.underlying(root)
            && matches!(self.node(pointer)?, Ty::Pointer(_))
        {
            return self.collect_accessible_members(pointer, recv, origin, false, true, false);
        }
        self.collect_accessible_members(recv, recv, origin, addressable, false, false)
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
                    let Some(candidate) = MethodLookup::new(arena, CheckMode::Symbolic)
                        .lookup_field_or_method(recv, false, declaration.file, name)?
                    else {
                        continue;
                    };
                    if matches!(candidate.obj, ObjectRef::Method(_)) {
                        members.push(((*name).to_owned(), candidate.obj));
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

    /// Receiver spellings only reject impossible candidates. Alias and dot-import
    /// receivers still require exact native receiver identity during instantiation.
    fn concrete_owner_candidates(&mut self, declaration: DeclId<'a>) -> Option<Vec<DeclId<'a>>> {
        if let Some(index) = self
            .env_cache
            .pkg_method_index(self.workspace, declaration.file)
        {
            let decl_name = declaration.symbol.name.as_str();
            let direct = index.by_recv.get(decl_name);
            let mut candidates =
                Vec::with_capacity(direct.map_or(0, Vec::len) + index.alias_methods.len());
            if let Some(direct) = direct {
                candidates.extend_from_slice(direct);
            }
            for &decl in &index.alias_methods {
                if decl.symbol.receiver.as_deref() != Some(decl_name) {
                    candidates.push(decl);
                }
            }
            return Some(candidates);
        }
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
        let root_pointer = matches!(self.node(unaliased_source)?, Ty::Pointer(_));
        let can_auto_addr = addressable && !fields_only && !root_pointer;
        for _ in 0..MAX_DEPTH {
            let mut next = Vec::new();
            let mut level_map: BTreeMap<MemberName<'a>, Option<Selection<'a>>> = BTreeMap::new();
            for index in level {
                self.step()?;
                let mut input = self.unalias(steps[index].ty)?;
                let pointer = if let Ty::Pointer(element) = *self.node(input)? {
                    input = self.unalias(element)?;
                    true
                } else {
                    false
                };
                let indirect = steps[index].indirect || pointer;
                if matches!(self.node(input)?, Ty::Pointer(_)) {
                    continue;
                }
                if !fields_only && matches!(self.node(input)?, Ty::TypeParam(_)) {
                    if pointer {
                        continue;
                    }
                    if let Some(set) = self.parameter_set(input)
                        && let Some(iface) = set.methods
                    {
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
                    let owner = self.named_info(input).map(|(decl, _, _)| decl.symbol);
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
                    && let Some((declaration, arguments, false)) = self.named_info(input)
                {
                    self.check_named_arguments(input)?;
                    if has_owner_cycle(&steps, steps[index].parent, declaration) {
                        continue;
                    }
                    steps[index].owner = Some(declaration);
                    let arguments = self.arguments_slice(input, arguments).to_vec();
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
                let Some((field_origin, fields)) =
                    self.struct_data(root).map(|d| (d.origin, d.fields.clone()))
                else {
                    continue;
                };
                let fields_vec = self.fields_slice(root, fields)?.to_vec();
                for (ord, field) in fields_vec.into_iter().enumerate() {
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
                        if steps.len() >= self.active_node_limit() {
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
        let methods = self.interface_data(root)?.methods.clone();
        let methods = self.methods_slice(root, methods).to_vec();
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

    pub(super) fn instantiate_named_method(
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
            let recv_base = if let Ty::Pointer(element) = *self.node(recv_ty)? {
                self.unalias(element)?
            } else {
                recv_ty
            };
            let Some((candidate, _, false)) = self.named_info(recv_base) else {
                return None;
            };
            if candidate.file != declaration.file
                || candidate.symbol.start != declaration.symbol.start
            {
                return None;
            }
        }
        let header = self.env_cache.method_header(
            self.workspace,
            DeclId {
                file,
                symbol: method,
            },
        )?;
        if header.receiver_names.len() != arguments.len() {
            return None;
        }
        let receiver = header.parsed.receiver.first()?;
        let environment: Vec<_> = header
            .receiver_names
            .iter()
            .zip(arguments)
            .map(|(name, ty)| (&header.text[name.clone()], *ty))
            .collect();
        let receiver = self.parse_in(
            file,
            &header.text[receiver.annotation.clone()],
            &environment,
        )?;
        let receiver = self.unalias(receiver)?;
        let pointer_receiver = matches!(self.node(receiver)?, Ty::Pointer(_));
        let signature = self.signature(
            file,
            &header.text,
            Some(header.start),
            &header.parsed,
            &environment,
            0,
        )?;
        Some((signature, pointer_receiver))
    }
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
