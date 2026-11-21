//! Type declaration origins (package & local), alias chains, and gopls-style type projection.
use super::store::LocalTypeMeta;
use super::ty::{NamedOriginData, NamedState, OriginId, SliceRef, TypeOrigin};
use super::{MAX_BYTES, MAX_DEPTH, MAX_NODES, Ty, TypeArena, TypeId};
use crate::analysis::engine::obj::DeclId;
use crate::index::IndexedSymbol as Symbol;
use std::ops::Range;
use std::path::Path;

pub(crate) enum TypeDeclaration<'a> {
    Named(&'a Path, &'a Symbol),
    Builtin(&'static str),
    Local(&'a Path, Range<usize>),
}

/// One written local `type` declaration, borrowed for the duration of the query.
/// The RHS byte origin keeps nested anonymous member identities exact.
#[allow(dead_code)]
pub(crate) struct LocalTypeDefinition<'s> {
    pub(crate) name: &'s str,
    pub(crate) selection: Range<usize>,
    pub(crate) annotation: &'s str,
    pub(crate) annotation_start: usize,
    pub(crate) alias: bool,
}

// Local defined types/aliases use the documented 64-entry, 16 KiB budget.
const MAX_LOCAL_TYPES: usize = 64;

impl<'a> TypeArena<'a> {
    /// Immediate alias RHS: preserve alias chains instead of unaliasing them.
    pub(crate) fn alias_named_rhs(&mut self, declaration: DeclId<'a>) -> Option<DeclId<'a>> {
        if !declaration.symbol.alias {
            return None;
        }
        let alias = self.declaration_type(declaration)?;
        let rhs = self.expand(alias)?;
        self.named_info(rhs).map(|(declaration, _, _)| declaration)
    }

    /// Register a function-local `type` declaration directly as a `LocalArena`
    /// `NamedOriginData` without cloning `String`s when the file is open.
    pub(crate) fn register_local_type(
        &mut self,
        origin: &Path,
        name: &str,
        selection: Range<usize>,
        annotation: &str,
        annotation_span: Range<usize>,
        alias: bool,
    ) -> Option<TypeId> {
        if name.is_empty() || annotation.is_empty() {
            return None;
        }
        let origin = self.workspace.files.get_key_value(origin)?.0.as_path();
        let start = u32::try_from(selection.start).ok()?;
        let end = u32::try_from(selection.end).ok()?;
        if let Some(&existing) = self.local.local_origins.get(&(origin, start, end)) {
            let (oid, _, _, existing_alias) = self.local_decl_info(existing)?;
            let name_ok = self
                .local_type_name_str(oid, origin, &selection)
                .is_none_or(|stored| stored == name);
            let ann_ok = self
                .local_type_annotation_raw(oid, origin)
                .is_none_or(|stored| stored == annotation);
            return (existing_alias == alias && name_ok && ann_ok).then_some(existing);
        }
        if self.local.local_origins.len() >= MAX_LOCAL_TYPES {
            return None;
        }
        let open_src = self.workspace.open_source(origin);
        let name_in_open = open_src
            .and_then(|src| src.get(selection.clone()))
            .is_some_and(|s| s == name);
        let ann_in_open = open_src
            .and_then(|src| src.get(annotation_span.clone()))
            .is_some_and(|s| s == annotation);

        let name_buf = if name_in_open {
            SliceRef::default()
        } else {
            let buf_start = self.local.local_text.len();
            if buf_start.checked_add(name.len())? > MAX_BYTES {
                return None;
            }
            self.local.local_text.push_str(name);
            SliceRef::from_range(buf_start..self.local.local_text.len())?
        };
        let (ann_start, ann_end, ann_in_buf) = if ann_in_open {
            (
                u32::try_from(annotation_span.start).ok()?,
                u32::try_from(annotation_span.end).ok()?,
                false,
            )
        } else {
            let buf_start = self.local.local_text.len();
            if buf_start.checked_add(annotation.len())? > MAX_BYTES {
                return None;
            }
            self.local.local_text.push_str(annotation);
            (
                u32::try_from(buf_start).ok()?,
                u32::try_from(self.local.local_text.len()).ok()?,
                true,
            )
        };

        let mut data = NamedOriginData::new(TypeOrigin::Local { origin, start, end }, alias);
        data.state = NamedState::Loading;
        let (origin_id, node) = self.local.pool.push_origin_node(data, alias)?;
        self.local.local_origins.insert((origin, start, end), node);
        self.local.local_type_meta.insert(
            origin_id,
            LocalTypeMeta {
                name_buf,
                ann_start,
                ann_end,
                ann_in_buf,
            },
        );
        Some(node)
    }

    /// Populate (or fail) the RHS of a previously registered local `NamedOrigin`.
    pub(crate) fn set_local_type_rhs(&mut self, local_ty: TypeId, rhs: Option<TypeId>) {
        let Some(origin_id) = self.local_origin_id(local_ty) else {
            return;
        };
        if let Some(data) = self.origin_data_mut(origin_id) {
            match rhs {
                Some(ty) => {
                    data.rhs = ty;
                    data.state = NamedState::Loaded;
                }
                None => {
                    data.rhs = TypeId::INVALID;
                    data.state = NamedState::Failed;
                }
            }
        }
    }

    /// Lower an already-parsed `TypeSyntax` directly with an environment of
    /// local types and type parameters (zero `LocalTypeDefinition` reconstruction).
    pub(crate) fn lower_type_syntax_in(
        &mut self,
        origin: &Path,
        text: &str,
        parsed_syntax: &crate::syntax::TypeSyntax,
        file_offset: Option<usize>,
        environment: &[(&str, TypeId)],
    ) -> Option<TypeId> {
        self.lower_type_syntax_checked(origin, text, parsed_syntax, file_offset, environment)
            .ok()
    }

    /// Register query-local local-type facts and parse one written annotation in
    /// that environment. Identity is `(file, selection)`; the spelling is display only.
    #[allow(dead_code)]
    pub(crate) fn declared_scope_type_in(
        &mut self,
        origin: &Path,
        text: &str,
        file_offset: Option<usize>,
        definitions: &[LocalTypeDefinition<'_>],
        extra_env: &[(&str, TypeId)],
    ) -> Option<TypeId> {
        let parsed = crate::syntax::parse_type(text);
        self.declared_scope_type_in_parsed(
            origin,
            text,
            &parsed,
            file_offset,
            definitions,
            extra_env,
        )
    }

    #[allow(dead_code)]
    pub(crate) fn declared_scope_type_in_parsed(
        &mut self,
        origin: &Path,
        text: &str,
        parsed_syntax: &crate::syntax::TypeSyntax,
        file_offset: Option<usize>,
        definitions: &[LocalTypeDefinition<'_>],
        extra_env: &[(&str, TypeId)],
    ) -> Option<TypeId> {
        if text.len() > MAX_BYTES || definitions.len() > MAX_LOCAL_TYPES {
            return None;
        }
        let origin = self.workspace.files.get_key_value(origin)?.0.as_path();
        let mut environment: Vec<(&str, TypeId)> =
            Vec::with_capacity(definitions.len() + extra_env.len());
        let mut registered: Vec<TypeId> = Vec::with_capacity(definitions.len());
        for definition in definitions {
            if definition.name.is_empty()
                || definition.annotation.is_empty()
                || environment.iter().any(|(name, _)| *name == definition.name)
            {
                return None;
            }
            let ann_end = definition
                .annotation_start
                .checked_add(definition.annotation.len())?;
            let node = self.register_local_type(
                origin,
                definition.name,
                definition.selection.clone(),
                definition.annotation,
                definition.annotation_start..ann_end,
                definition.alias,
            )?;
            environment.push((definition.name, node));
            registered.push(node);
        }
        for &(name, ty) in extra_env {
            if !environment.iter().any(|(existing, _)| *existing == name) {
                environment.push((name, ty));
            }
        }
        for (definition, node) in definitions.iter().zip(registered) {
            let origin_id = self.local_origin_id(node)?;
            if self.origin_data(origin_id)?.state != NamedState::Loading {
                continue;
            }
            let parsed = self.parse_in_at(
                origin,
                definition.annotation,
                definition.annotation_start,
                &environment,
            );
            self.set_local_type_rhs(node, parsed);
        }
        self.lower_type_syntax_checked(origin, text, parsed_syntax, file_offset, &environment)
            .ok()
    }

    pub(super) fn is_ready_defined_local(&self, input: TypeId) -> bool {
        let Some((origin_id, _, _, false)) = self.local_decl_info(input) else {
            return false;
        };
        let Some(data) = self.origin_data(origin_id) else {
            return false;
        };
        !data.is_alias() && data.state == NamedState::Loaded
    }

    pub(super) fn is_defined_type(&self, input: TypeId) -> bool {
        self.named_info(input).is_some()
            || matches!(self.node(input), Some(Ty::Basic(_) | Ty::TypeParam(_)))
            || self.is_ready_defined_local(input)
    }

    pub(super) fn is_known_non_opaque(&self, input: TypeId) -> bool {
        self.local_origin_id(input).is_none() || self.is_ready_defined_local(input)
    }

    fn local_type_name_str(
        &self,
        origin_id: OriginId,
        origin: &Path,
        selection: &Range<usize>,
    ) -> Option<&str> {
        if let Some(meta) = self.local.local_type_meta.get(&origin_id)
            && !meta.name_buf.is_empty()
        {
            return self.local.local_text.get(meta.name_buf.range());
        }
        self.workspace
            .open_source(origin)
            .and_then(|src| src.get(selection.clone()))
    }

    fn local_type_annotation_raw(&self, origin_id: OriginId, origin: &Path) -> Option<&str> {
        let meta = self.local.local_type_meta.get(&origin_id)?;
        let span = meta.ann_start as usize..meta.ann_end as usize;
        if meta.ann_in_buf {
            self.local.local_text.get(span)
        } else {
            self.workspace
                .open_source(origin)
                .and_then(|src| src.get(span))
        }
    }

    /// Display name of a registered local declaration; the spelling never proves
    /// identity. Unregistered opaque declarations fall back to a bounded read.
    pub(super) fn local_type_name(
        &self,
        origin_id: OriginId,
        origin: &Path,
        selection: &Range<usize>,
    ) -> Option<String> {
        if let Some(name) = self.local_type_name_str(origin_id, origin, selection) {
            return Some(name.to_owned());
        }
        self.workspace.source_slice(origin, selection.clone())
    }

    /// Expand the outer local declaration chain with an active set; the depth and
    /// node budgets bound every step. Package types are returned for the caller's
    /// existing lazy expansion.
    pub(super) fn local_declaration_underlying(&mut self, input: TypeId) -> Option<TypeId> {
        let mut active: Vec<OriginId> = Vec::new();
        let mut current = input;
        for _ in 0..MAX_DEPTH {
            let Some(origin_id) = self.local_origin_id(current) else {
                return Some(current);
            };
            if active.contains(&origin_id) {
                return None;
            }
            active.push(origin_id);
            let data = self.origin_data(origin_id)?;
            if data.state != NamedState::Loaded {
                return None;
            }
            current = data.rhs_type()?;
        }
        None
    }

    /// One alias step: `Some(Some(next))` for a registered local alias,
    /// `Some(None)` for a non-local or defined type, `None` when unknown.
    pub(super) fn local_alias(&self, input: TypeId) -> Option<Option<TypeId>> {
        let Some(origin_id) = self.local_origin_id(input) else {
            return Some(None);
        };
        let data = self.origin_data(origin_id)?;
        if !data.is_alias() {
            return Some(None);
        }
        if data.state != NamedState::Loaded {
            return None;
        }
        Some(Some(data.rhs_type()?))
    }

    /// Whether a compound type contains a query-local declaration, so callers can
    /// keep such spellings display-only instead of reparsing them as identities.
    pub(crate) fn has_local_declaration(&mut self, input: TypeId) -> Option<bool> {
        let mut pending = vec![input];
        let mut visited = Vec::new();
        while let Some(ty) = pending.pop() {
            if visited.contains(&ty) {
                continue;
            }
            if visited.len() >= MAX_NODES {
                return None;
            }
            visited.push(ty);
            if self.local_origin_id(ty).is_some() {
                return Some(true);
            }
            match *self.node(ty)? {
                Ty::Invalid => return None,
                Ty::Named { arguments, .. } | Ty::Alias { arguments, .. } => {
                    pending.extend_from_slice(self.arguments_slice(ty, arguments))
                }
                Ty::Tuple(elements) => {
                    pending.extend_from_slice(self.arguments_slice(ty, elements))
                }
                Ty::Struct(_) => {
                    let fields = self.fields_slice(ty, self.struct_data(ty)?.fields.clone())?;
                    pending.extend(fields.iter().map(|field| field.ty));
                }
                Ty::Interface(_) => {
                    let data = self.interface_data(ty)?;
                    let methods = self.methods_slice(ty, data.methods.clone());
                    pending.extend(methods.iter().map(|method| method.signature));
                    pending.extend_from_slice(self.arguments_slice(ty, data.embeddeds));
                }
                Ty::Union(slice) => {
                    pending.extend(self.terms_slice(ty, slice).iter().map(|term| term.ty));
                }
                Ty::Signature(_) => {
                    let sig = self.signature_data(ty)?;
                    pending.extend_from_slice(self.arg_range(ty, sig.parameters.clone()));
                    pending.extend_from_slice(self.arg_range(ty, sig.results.clone()));
                }
                Ty::Pointer(element)
                | Ty::Slice(element)
                | Ty::Array { element, .. }
                | Ty::Chan { element, .. } => pending.push(element),
                Ty::Map { key, value } => pending.extend([key, value]),
                Ty::Basic(_) | Ty::TypeParam(_) => {}
            }
        }
        Some(false)
    }

    /// A declaration view imports its actual formal identities and bounds from the
    /// shared `DeclArena` origin template without re-reading or re-parsing.
    pub(crate) fn declaration_type(&mut self, declaration: DeclId<'a>) -> Option<TypeId> {
        let origin_id = self.get_or_insert_package_origin(declaration, declaration.symbol.alias)?;
        let arguments = if declaration.symbol.generic {
            self.ensure_origin_tparams(origin_id, true)?
        } else {
            Vec::new()
        };
        self.push_named(declaration, &arguments, declaration.symbol.alias)
    }

    /// Explicit named embeddings, proved in this declaration's native formal
    /// environment. Metadata selects candidates; it does not resolve an edge.
    pub(crate) fn direct_embedding_declarations(
        &mut self,
        declaration: DeclId<'a>,
    ) -> Option<Vec<DeclId<'a>>> {
        if declaration.symbol.type_info.as_ref()?.embedded.is_empty() {
            return Some(Vec::new());
        }
        let receiver = self.declaration_type(declaration)?;
        self.validate_type(receiver)?;
        let root = self.underlying(receiver)?;
        let mut embedded = Vec::new();
        match *self.node(root)? {
            Ty::Struct(_) => {
                let fields = self.struct_data(root)?.fields.clone();
                embedded.extend(
                    self.fields_slice(root, fields)?
                        .iter()
                        .filter(|field| field.embedded)
                        .map(|field| field.ty),
                );
            }
            Ty::Interface(_) => {
                let span = declaration.symbol.end..declaration.symbol.declaration()?.end;
                let source = self
                    .workspace
                    .source_slice(declaration.file, span.clone())?;
                let (header, signature) =
                    super::named::parse_type_parameters_of(&source, declaration.symbol.generic)?;
                let names = super::constraints::parameter_names(header, &signature)?;
                let (_, arguments, _) = self.named_declaration(receiver)?;
                if names.len() != arguments.len() {
                    return None;
                }
                let environment = names.into_iter().zip(arguments).collect::<Vec<_>>();
                let body = super::declaration_body(&source)?;
                let offset = span.start + source.len() - body.len();
                let parsed = crate::syntax::parse_type(body);
                if !parsed.diagnostics.is_empty() {
                    return None;
                }
                let aggregate = crate::syntax::aggregate_offset(body)?;
                let opening = crate::syntax::Lexer::new(body.get(aggregate..)?).nth(1)?;
                if opening.kind != crate::syntax::TokenKind::LBrace {
                    return None;
                }
                let container = aggregate + opening.span.start;
                for member in parsed
                    .interface_members
                    .iter()
                    .filter(|member| member.container == container && member.name.is_none())
                {
                    embedded.push(self.parse_in_at(
                        declaration.file,
                        body.get(member.annotation.clone())?,
                        offset + member.annotation.start,
                        &environment,
                    )?);
                }
            }
            _ => return Some(Vec::new()),
        }
        let mut declarations = Vec::new();
        for embedded in embedded {
            if let Some(declaration) = self.receiver_declaration(embedded)
                && !declarations.contains(&declaration)
            {
                declarations.push(declaration);
            }
        }
        Some(declarations)
    }

    /// Projection-only schema: opaque formals preserve declaration references.
    /// It deliberately proves no constraints and must not check value calls.
    pub(crate) fn declaration_signature(
        &mut self,
        origin: &Path,
        header: &str,
        parsed: &crate::syntax::Signature,
    ) -> Option<TypeId> {
        let mut names = super::constraints::parameter_names(header, parsed)?
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if let Some(receiver) = parsed.receiver.first() {
            for span in
                crate::analysis::engine::scope::receiver_parameters(header, &receiver.annotation)
            {
                let name = header.get(span)?.to_owned();
                if !names.contains(&name) {
                    names.push(name);
                }
            }
        }
        if names.len() > self.active_node_limit() {
            return None;
        }
        let mut environment = Vec::new();
        for (index, name) in names.iter().enumerate() {
            environment.push((name.as_str(), self.push_type_param(index)?));
        }
        if header.len() > super::MAX_BYTES || !parsed.diagnostics.is_empty() {
            return None;
        }
        let mut results = Vec::new();
        for result in &parsed.results {
            results.push(self.parse_in(
                origin,
                header.get(result.annotation.clone())?,
                &environment,
            )?);
        }
        self.store_signature(Vec::new(), results, false)
    }

    /// Named/alias origins stop traversal; unnamed containers unwrap elements,
    /// signatures visit results only, and maps/anonymous aggregates have no name.
    /// Repeated results are retained, matching the reference server's ordering.
    pub(crate) fn type_declarations(&mut self, input: TypeId) -> Option<Vec<TypeDeclaration<'a>>> {
        let mut result = Vec::new();
        let mut pending = vec![(input, 0)];
        while let Some((input, depth)) = pending.pop() {
            self.step()?;
            if depth >= MAX_DEPTH || result.len() >= MAX_NODES {
                return None;
            }
            if let Some((declaration, _, _)) = self.named_info(input) {
                result.push(TypeDeclaration::Named(declaration.file, declaration.symbol));
            } else if let Some((_, origin, selection, _)) = self.local_decl_info(input) {
                result.push(TypeDeclaration::Local(origin, selection));
            } else if self.error_interface(input).is_some() {
                result.push(TypeDeclaration::Builtin("error"));
            } else {
                match *self.node(input)? {
                    Ty::Invalid => return None,
                    Ty::Basic(super::BasicKind::UnsafePointer) => {
                        if let Some(unsafe_file) = self
                            .workspace
                            .builtin_file()
                            .and_then(|b| Some(b.parent()?.parent()?.join("unsafe/unsafe.go")))
                            && let Some((path, stub)) =
                                self.workspace.files.get_key_value(&unsafe_file)
                            && let Some(sym) = stub
                                .symbols
                                .iter()
                                .find(|s| s.is_type() && s.name == "Pointer")
                        {
                            result.push(TypeDeclaration::Named(path.as_path(), sym));
                        }
                    }
                    Ty::Basic(kind) if !kind.untyped() => {
                        result.push(TypeDeclaration::Builtin(kind.name()))
                    }
                    Ty::Interface(_) if self.interface_data(input)?.builtin_any => {
                        result.push(TypeDeclaration::Builtin("any"))
                    }
                    Ty::Pointer(element)
                    | Ty::Slice(element)
                    | Ty::Array { element, .. }
                    | Ty::Chan { element, .. } => pending.push((element, depth + 1)),
                    Ty::Tuple(elements) => pending.extend(
                        self.arguments_slice(input, elements)
                            .iter()
                            .rev()
                            .map(|ty| (*ty, depth + 1)),
                    ),
                    Ty::Signature(_) => {
                        let results = self.signature_data(input)?.results.clone();
                        pending.extend(
                            self.arg_range(input, results)
                                .iter()
                                .rev()
                                .map(|ty| (*ty, depth + 1)),
                        );
                    }
                    Ty::Basic(_)
                    | Ty::TypeParam(_)
                    | Ty::Named { .. }
                    | Ty::Alias { .. }
                    | Ty::Union(_)
                    | Ty::Interface(_)
                    | Ty::Struct(_)
                    | Ty::Map { .. } => {}
                }
            }
            if pending.len() > MAX_NODES {
                return None;
            }
        }
        Some(result)
    }

    pub(crate) fn projected_type_declarations(
        &mut self,
        ty: TypeId,
    ) -> Option<Result<Vec<TypeDeclaration<'a>>, (i32, String)>> {
        let names = self.type_declarations(ty)?;
        Some(if names.is_empty() {
            if let Some(display) = self.diagnostic_type(ty) {
                Err((0, format!("cannot find type name(s) from type {display}")))
            } else {
                Ok(names)
            }
        } else {
            Ok(names)
        })
    }

    pub(crate) fn local_type_annotation(&self, id: TypeId) -> Option<(&'a Path, &str)> {
        let (origin_id, origin, _, false) = self.local_decl_info(id)? else {
            return None;
        };
        let annotation = self.local_type_annotation_raw(origin_id, origin)?;
        Some((origin, annotation))
    }
}
