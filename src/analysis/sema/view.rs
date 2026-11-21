//! Lifetime-bound query views over checked regions and shared declarations.
//! None of [`RegionInfo`], [`DeclTable`], [`TypeArena`], or [`TypeId`] escape this module.

use std::path::Path;

use crate::analysis::engine::budget::MAX_BYTES;
use crate::analysis::engine::check::decls::DeclTable;
use crate::analysis::engine::check::region::{RegionInfo, WriteKind};
use crate::analysis::engine::obj::{
    DeclId, LocalId, MemberDeclId, MethodOrigin, Object, SymbolOrigin,
};
use crate::analysis::engine::resolve;
use crate::analysis::engine::scope::tree::LocalBindingKind;
use crate::analysis::engine::types::{Qualifier, TypeArena, TypeForm, TypeId};
use crate::syntax;
use crate::workspace::Workspace;

use super::CallableView;
use super::ty::{TypeAndValue, TypeInspector, TypeRef, TypeShape};

/// Requested region coverage for cursor queries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RegionMode {
    Full,
    Prefix(usize),
}

impl RegionMode {
    pub(crate) fn from_cursor(cursor_offset: Option<usize>) -> Self {
        match cursor_offset {
            None => Self::Full,
            Some(offset) => Self::Prefix(offset),
        }
    }

    pub(super) fn cursor_offset(self) -> Option<usize> {
        match self {
            Self::Full => None,
            Self::Prefix(offset) => Some(offset),
        }
    }
}

/// Resolved semantic object and span at a source position.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SymbolSite<'a> {
    pub(crate) span: [u32; 2],
    pub(crate) obj: Object<'a>,
}

/// Borrowed projection of a checked selector expression.
#[derive(Clone, Copy)]
pub(crate) struct SelectionView<'r, 'a> {
    pub(crate) obj: Object<'a>,
    pub(crate) ty: TypeRef<'r>,
}

/// Borrowed projection of a generic instantiation at a call or index site.
#[derive(Clone, Copy)]
pub(crate) struct InstanceView<'r, 'a> {
    pub(crate) target: Object<'a>,
    pub(crate) instantiated: TypeRef<'r>,
}

/// Borrowed projection of a checked call site.
pub(crate) struct CallView<'r, 'a> {
    pub(crate) span: [u32; 2],
    pub(crate) callee_span: [u32; 2],
    pub(crate) callee_ident_span: Option<[u32; 2]>,
    pub(crate) open_paren: u32,
    pub(crate) arg_spans: &'r [[u32; 2]],
    pub(crate) callee_obj: Option<Object<'a>>,
    pub(crate) signature: Option<TypeRef<'r>>,
    pub(crate) is_conversion: bool,
}

/// Semantic classification of a lexical declaration in a [`RegionView`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LocalDeclKind {
    Var,
    Const,
    Type,
    TypeParam,
    Param,
    TypeSwitchVar,
}

impl LocalDeclKind {
    fn from_binding(kind: LocalBindingKind) -> Self {
        match kind {
            LocalBindingKind::Receiver | LocalBindingKind::Param => Self::Param,
            LocalBindingKind::TypeParam => Self::TypeParam,
            LocalBindingKind::Const => Self::Const,
            LocalBindingKind::TypeDecl { .. } => Self::Type,
            LocalBindingKind::TypeSwitchVar => Self::TypeSwitchVar,
            LocalBindingKind::Var
            | LocalBindingKind::NamedResult
            | LocalBindingKind::ShortVar
            | LocalBindingKind::RangeKey
            | LocalBindingKind::RangeVal => Self::Var,
        }
    }
}

pub(crate) struct LocalBindingView {
    pub(crate) id: LocalId,
    pub(crate) kind: LocalDeclKind,
    pub(crate) def_span: [u32; 2],
    pub(crate) is_def: bool,
}

#[derive(Clone, Copy)]
pub(crate) struct VisibleLocal<'a> {
    pub(crate) name: &'a str,
    pub(crate) selection: [u32; 2],
    pub(crate) scope_start: u32,
    pub(crate) is_type: bool,
}

/// Lifetime-bound view into a checked function/declaration region plus disjoint
/// mutable access to [`TypeArena`] and [`DeclTable`] for lazy presentation/resolution.
pub(crate) struct RegionView<'r, 'a> {
    pub(super) workspace: &'a Workspace,
    pub(super) file: &'a Path,
    pub(super) source: &'r str,
    pub(super) region: &'r RegionInfo<'a>,
    pub(super) arena: &'r mut TypeArena<'a>,
    pub(super) decls: &'r mut DeclTable<'a>,
    pub(super) syntax_body: Option<(usize, &'r syntax::BlockTree)>,
    pub(super) syntax_sig: Option<(usize, &'r syntax::Signature)>,
}

/// Lifetime-bound view over file-level declarations outside any checked region.
pub(crate) struct PackageView<'r, 'a> {
    pub(super) workspace: &'a Workspace,
    pub(super) file: &'a Path,
    pub(super) source: &'r str,
    pub(super) arena: &'r mut TypeArena<'a>,
    pub(super) decls: &'r mut DeclTable<'a>,
}

impl<'r, 'a> RegionView<'r, 'a> {
    pub(crate) fn reset_steps(&mut self) {
        self.arena.reset_steps();
    }

    pub(crate) fn span(&self) -> [u32; 2] {
        self.region.span
    }

    pub(crate) fn syntax_body(&self) -> Option<(usize, &syntax::BlockTree)> {
        self.syntax_body
    }

    pub(crate) fn syntax_signature(&self) -> Option<(usize, &syntax::Signature)> {
        self.syntax_sig
    }

    pub(crate) fn object_at(&self, offset: u32) -> Option<([u32; 2], Object<'a>)> {
        self.region.object_at(offset)
    }

    pub(crate) fn def_by_span(&self, span: [u32; 2]) -> Option<Object<'a>> {
        self.region.def_by_span(span)
    }

    pub(crate) fn use_by_span(&self, span: [u32; 2]) -> Option<Object<'a>> {
        self.region.use_by_span(span)
    }

    pub(crate) fn use_or_selection(&self, span: [u32; 2]) -> Option<Object<'a>> {
        self.region
            .use_by_span(span)
            .or_else(|| self.region.selection_by_span(span).map(|sel| sel.obj))
    }

    pub(crate) fn symbol_at(&self, offset: u32) -> Option<SymbolSite<'a>> {
        let (span, obj) = self.region.object_at(offset)?;
        Some(SymbolSite { span, obj })
    }

    pub(crate) fn selection_by_span(&self, span: [u32; 2]) -> Option<SelectionView<'r, 'a>> {
        let sel = self.region.selection_by_span(span)?;
        Some(SelectionView {
            obj: sel.obj,
            ty: TypeRef::new(sel.ty),
        })
    }

    pub(crate) fn instance_by_span(&self, span: [u32; 2]) -> Option<InstanceView<'r, 'a>> {
        let inst = self.region.instance_by_span(span)?;
        Some(InstanceView {
            target: inst.target,
            instantiated: TypeRef::new(inst.instantiated),
        })
    }

    pub(crate) fn expr_type(&self, span: [u32; 2]) -> Option<TypeAndValue<'r>> {
        self.region.expr_type(span).map(TypeAndValue::from_operand)
    }

    pub(crate) fn smallest_expr_at(&self, offset: u32) -> Option<([u32; 2], TypeAndValue<'r>)> {
        let (span, op) = self.region.smallest_expr_at(offset)?;
        Some((span, TypeAndValue::from_operand(op)))
    }

    pub(crate) fn local_binding_at(
        &self,
        selection: [u32; 2],
        scope_start: usize,
    ) -> Option<(LocalId, u32)> {
        let case_pos = u32::try_from(scope_start).ok()?;
        self.region.local_binding_at(selection, case_pos)
    }

    pub(crate) fn binding_is_header_only_param(&self, id: LocalId) -> bool {
        self.region
            .scopes
            .bindings
            .get(id.0 as usize)
            .is_some_and(|b| {
                b.init == crate::analysis::engine::scope::tree::BindingInit::None
                    && matches!(
                        b.kind,
                        LocalBindingKind::Param | LocalBindingKind::NamedResult
                    )
            })
    }

    pub(crate) fn local_operand(&self, id: LocalId) -> Option<TypeAndValue<'r>> {
        self.region.local_type(id).map(TypeAndValue::from_operand)
    }

    pub(crate) fn local_init_span(&self, id: LocalId) -> Option<[u32; 2]> {
        self.region.local_init(id)
    }

    pub(crate) fn is_write(&self, span: [u32; 2]) -> bool {
        self.region.is_write(span)
    }

    pub(crate) fn write_kind_at(&self, span: [u32; 2]) -> Option<WriteKind> {
        self.region.write_kind_at(span)
    }

    /// Locate the checked struct type that directly declares `member` at `span`
    /// along with its 0-based field ordinal, using already-checked types in this
    /// region without re-parsing source text.
    pub(crate) fn field_owner_struct(
        &mut self,
        member: MemberDeclId<'a>,
        span: [u32; 2],
    ) -> Option<(TypeRef<'r>, usize)> {
        if let Some(sel) = self.region.selection_by_span(span)
            && sel.obj == Object::Field(member)
            && let [ordinal] = sel.indices.as_slice()
            && let Some(base) = self.arena.receiver_base(sel.recv)
            && self.arena.is_struct(base) == Some(true)
        {
            return Some((TypeRef::new(base), usize::from(*ordinal)));
        }
        let name = self.source.get(span[0] as usize..span[1] as usize)?;
        const MAX_CANDIDATES: usize = 64;
        let mut seen = Vec::new();
        let file = self.file;
        let mut check = |arena: &mut TypeArena<'a>, ty: TypeId| -> Option<(TypeRef<'r>, usize)> {
            if (!ty.is_decl() && !ty.is_local())
                || seen.len() >= MAX_CANDIDATES
                || seen.contains(&ty)
            {
                return None;
            }
            seen.push(ty);
            let base = arena.receiver_base(ty)?;
            if arena.is_struct(base) != Some(true) {
                return None;
            }
            let sel = arena.literal_field(base, file, name)?;
            if sel.obj == Object::Field(member)
                && let [ordinal] = sel.indices.as_slice()
            {
                Some((TypeRef::new(base), usize::from(*ordinal)))
            } else {
                None
            }
        };
        // 1. Enclosing expressions (e.g. anonymous struct composite literals).
        for &(expr_span, op) in &self.region.expr_types {
            if expr_span != span
                && expr_span[0] <= span[0]
                && span[1] <= expr_span[1]
                && let Some(found) = check(self.arena, op.ty)
            {
                return Some(found);
            }
        }
        // 2. Immediately preceding expressions (e.g. outer field whose annotation is a nested struct).
        for &(expr_span, op) in self.region.expr_types.iter().rev() {
            if expr_span[1] <= span[0]
                && let Some(found) = check(self.arena, op.ty)
            {
                return Some(found);
            }
        }
        // 3. Checked local bindings (vars, params, results, local type declarations).
        for op in self.region.local_types.iter().flatten() {
            if let Some(found) = check(self.arena, op.ty) {
                return Some(found);
            }
        }
        // 4. Region owner declaration (e.g. package-level `var X struct { ... }`).
        if let Some(owner) = self.region.owner
            && let Some(op) = self.decls.operand_of(self.arena, owner)
            && let Some(found) = check(self.arena, op.ty)
        {
            return Some(found);
        }
        None
    }

    pub(crate) fn calls(&self) -> impl Iterator<Item = CallView<'r, 'a>> + use<'r, 'a> {
        self.region.calls.iter().map(|c| CallView {
            span: c.span,
            callee_span: c.callee_span,
            callee_ident_span: c.callee_ident_span,
            open_paren: c.open_paren,
            arg_spans: &c.arg_spans,
            callee_obj: c.callee_obj,
            signature: c.signature.map(TypeRef::new),
            is_conversion: c.is_conversion,
        })
    }

    pub(crate) fn types(&mut self) -> TypeInspector<'_, 'r, 'a> {
        TypeInspector::new(self.file, self.arena)
    }

    pub(crate) fn resolver(&self) -> resolve::View<'a> {
        self.arena.resolver()
    }

    pub(crate) fn declaration_member(
        &mut self,
        declaration: DeclId<'a>,
        name: &str,
    ) -> Option<Object<'a>> {
        self.arena.declaration_member(declaration, self.file, name)
    }

    pub(crate) fn type_of_object(
        &mut self,
        obj: Object<'a>,
        span: [u32; 2],
    ) -> Option<TypeRef<'r>> {
        match obj {
            Object::PkgName { .. }
            | Object::Label { .. }
            | Object::Nil
            | Object::BuiltinFunction(_)
            | Object::TypeParam { .. } => None,
            Object::Var {
                origin: SymbolOrigin::Local { id, .. },
                ..
            }
            | Object::Const(SymbolOrigin::Local { id, .. })
            | Object::TypeName {
                origin: SymbolOrigin::Local { id, .. },
                ..
            } => self.region.local_type(id).map(|op| TypeRef::new(op.ty)),
            Object::TypeName {
                origin: SymbolOrigin::Package(decl),
                ..
            } => self.decls.type_of(self.arena, decl).map(TypeRef::new),
            Object::BuiltinType(name) => {
                let file = self.file;
                self.types().builtin_type(file, name)
            }
            Object::BuiltinConst(_) => self.region.expr_type(span).map(|op| TypeRef::new(op.ty)),
            Object::Field(_) => self
                .region
                .selection_by_span(span)
                .map(|sel| TypeRef::new(sel.ty)),
            Object::Method(origin) => {
                if let Some(sel) = self.region.selection_by_span(span) {
                    Some(TypeRef::new(sel.ty))
                } else if let MethodOrigin::Concrete(decl) = origin {
                    self.decl_callable_type(decl).map(TypeRef::new)
                } else {
                    None
                }
            }
            Object::Func(decl) => {
                if let Some(inst) = self.region.instance_by_span(span) {
                    Some(TypeRef::new(inst.instantiated))
                } else {
                    self.decl_callable_type(decl).map(TypeRef::new)
                }
            }
            Object::Const(SymbolOrigin::Package(decl))
            | Object::Var {
                origin: SymbolOrigin::Package(decl),
                ..
            } => self
                .decls
                .operand_of(self.arena, decl)
                .map(|op| TypeRef::new(op.ty)),
        }
    }

    pub(crate) fn format_type_for_local(
        &mut self,
        ty: TypeRef<'r>,
        qualifier: Qualifier<'_>,
    ) -> Option<String> {
        let file = self.file;
        self.types().format_for_local(ty, file, qualifier)
    }

    pub(crate) fn callable_view(
        &mut self,
        ty: TypeRef<'r>,
        consumer: &Path,
    ) -> Option<CallableView> {
        self.types().callable_view(ty, consumer)
    }

    pub(crate) fn decl_type_shape(&mut self, decl: DeclId<'a>, constant: bool) -> TypeShape {
        decl_type_shape_impl(
            self.workspace,
            self.file,
            self.source,
            self.arena,
            self.decls,
            decl,
            constant,
        )
    }

    pub(crate) fn type_form_in_scope(
        &mut self,
        file: &Path,
        annotation: &str,
        scope_offset: Option<u32>,
    ) -> Option<TypeForm> {
        let scopes = &self.region.scopes;
        let source = self.source;
        let mut inspector = TypeInspector::new(self.file, self.arena);
        inspector.type_form_in_scope(file, annotation, |ident| {
            scope_offset.is_some_and(|offset| scopes.resolve_at(source, offset, ident).is_some())
        })
    }

    pub(crate) fn local_shadows(&self, def_span: [u32; 2], spelling: &str) -> bool {
        self.region
            .scopes
            .has_outer_binding_at_span(self.source, def_span, spelling)
    }

    pub(crate) fn is_local_defined_at(&self, offset: u32, ident: &str) -> bool {
        self.region
            .scopes
            .resolve_at(self.source, offset, ident)
            .is_some()
    }

    pub(crate) fn is_range_blank(&self, span: [u32; 2]) -> bool {
        self.region.scopes.is_range_blank(span)
    }

    pub(crate) fn visible_locals_at(&self, offset: u32) -> Vec<VisibleLocal<'r>> {
        self.region
            .scopes
            .visible_bindings_for_completion(self.source, offset)
            .into_iter()
            .map(
                |(_id, name, selection, scope_start, is_type)| VisibleLocal {
                    name,
                    selection,
                    scope_start,
                    is_type,
                },
            )
            .collect()
    }

    pub(crate) fn local_binding_at_span(&self, span: [u32; 2]) -> Option<LocalBindingView> {
        if let Some((id, kind, def_span)) = self.region.scopes.binding_at_exact_span(span) {
            return Some(LocalBindingView {
                id,
                kind: LocalDeclKind::from_binding(kind),
                def_span,
                is_def: true,
            });
        }
        let spelling = self.source.get(span[0] as usize..span[1] as usize)?;
        if spelling == "_" {
            return None;
        }
        let id = self
            .region
            .scopes
            .resolve_at(self.source, span[0], spelling)?;
        let binding = self.region.scopes.bindings.get(id.0 as usize)?;
        Some(LocalBindingView {
            id,
            kind: LocalDeclKind::from_binding(binding.kind),
            def_span: binding.name_span,
            is_def: false,
        })
    }

    pub(crate) fn imported_name(&self, qualifier: &str) -> bool {
        self.arena.resolver().imported_name(self.file, qualifier)
    }

    fn decl_callable_type(&mut self, decl: DeclId<'a>) -> Option<TypeId> {
        self.decls.type_of(self.arena, decl).or_else(|| {
            if !decl.symbol.is_callable() {
                return None;
            }
            let range = decl.symbol.declaration()?;
            let end = decl.symbol.body().map_or(range.end, |b| b.start);
            if end.checked_sub(range.start)? > MAX_BYTES {
                return None;
            }
            let header = if decl.file == self.file {
                std::borrow::Cow::Borrowed(self.source.get(range.start..end)?)
            } else {
                std::borrow::Cow::Owned(self.workspace.source_slice(decl.file, range.start..end)?)
            };
            let parsed = syntax::parse_signature(&header);
            self.arena
                .declaration_signature(decl.file, &header, &parsed)
        })
    }
}

impl<'r, 'a> PackageView<'r, 'a> {
    pub(crate) fn reset_steps(&mut self) {
        self.arena.reset_steps();
    }

    pub(crate) fn types(&mut self) -> TypeInspector<'_, 'r, 'a> {
        TypeInspector::new(self.file, self.arena)
    }

    pub(crate) fn resolver(&self) -> resolve::View<'a> {
        self.arena.resolver()
    }

    pub(crate) fn declaration_member(
        &mut self,
        declaration: DeclId<'a>,
        name: &str,
    ) -> Option<Object<'a>> {
        self.arena.declaration_member(declaration, self.file, name)
    }

    pub(crate) fn decl_type_shape(&mut self, decl: DeclId<'a>, constant: bool) -> TypeShape {
        decl_type_shape_impl(
            self.workspace,
            self.file,
            self.source,
            self.arena,
            self.decls,
            decl,
            constant,
        )
    }

    pub(crate) fn imported_name(&self, qualifier: &str) -> bool {
        self.arena.resolver().imported_name(self.file, qualifier)
    }
}

fn decl_type_shape_impl<'a>(
    workspace: &'a Workspace,
    file: &'a Path,
    source: &str,
    arena: &mut TypeArena<'a>,
    decls: &mut DeclTable<'a>,
    decl: DeclId<'a>,
    constant: bool,
) -> TypeShape {
    if let Some(ty) = decls.type_of(arena, decl) {
        let mut inspector = TypeInspector::new(file, arena);
        let shape = inspector.type_shape(TypeRef::new(ty), constant);
        if shape != TypeShape::Unknown {
            return shape;
        }
    }
    if decl.symbol.is_type() {
        let mut inspector = TypeInspector::new(file, arena);
        return inspector
            .type_form_in_scope(decl.file, &decl.symbol.name, |_| false)
            .map_or(TypeShape::Unknown, TypeShape::Form);
    }
    if (decl.symbol.is_var() || decl.symbol.is_const())
        && let Some(decl_range) = decl.symbol.declaration()
    {
        let spec_src = if decl.file == file {
            source
                .get(decl_range.clone())
                .map(std::borrow::Cow::Borrowed)
        } else {
            workspace
                .source_slice(decl.file, decl_range.clone())
                .map(std::borrow::Cow::Owned)
        };
        if let Some(spec_src) = spec_src {
            let kind = if decl.symbol.is_const() {
                syntax::DeclSpecKind::Const
            } else {
                syntax::DeclSpecKind::Var
            };
            let tree = syntax::parse_decl_spec(&spec_src, kind);
            if let Some(syntax::StmtKind::Declaration {
                annotation: Some(ann),
                ..
            }) = tree.statement(tree.root()).map(|stmt| &stmt.kind)
                && let Some(ann_text) = spec_src.get(ann.clone())
            {
                let mut inspector = TypeInspector::new(file, arena);
                return inspector
                    .type_form_in_scope(decl.file, ann_text.trim(), |_| false)
                    .map_or(TypeShape::Unknown, TypeShape::Form);
            }
        }
    }
    TypeShape::Unknown
}
