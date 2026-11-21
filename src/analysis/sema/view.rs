//! Lifetime-bound query views over checked regions and shared declarations.
//! None of [`RegionInfo`], [`DeclTable`], [`TypeArena`], or [`TypeId`] escape this module.

use std::marker::PhantomData;
use std::path::Path;

use crate::analysis::engine::budget::MAX_BYTES;
use crate::analysis::engine::check::decls::DeclTable;
use crate::analysis::engine::check::region::RegionInfo;
use crate::analysis::engine::obj::{DeclId, LocalId, MethodOrigin, Object, SymbolOrigin};
use crate::analysis::engine::resolve;
use crate::analysis::engine::scope::tree::LocalBindingKind;
use crate::analysis::engine::types::{
    BasicKind, Constant, Mode, Qualifier, TypeArena, TypeDeclaration, TypeForm, TypeId,
    type_form_in_scope,
};
use crate::index::IndexedSymbol;
use crate::syntax::{self, Lexer};
use crate::workspace::Workspace;

use super::CallableView;

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

/// Lifetime-locked handle to a [`TypeId`] within the active [`RegionView`] or [`PackageView`].
/// Prevents dangling handles across `LocalArena` epoch resets at compile time.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TypeHandle<'r> {
    pub(super) id: TypeId,
    _lifetime: PhantomData<&'r ()>,
}

impl<'r> TypeHandle<'r> {
    pub(super) fn new(id: TypeId) -> Self {
        Self {
            id,
            _lifetime: PhantomData,
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
    pub(crate) ty: TypeHandle<'r>,
}

/// Borrowed projection of a generic instantiation at a call or index site.
#[derive(Clone, Copy)]
pub(crate) struct InstanceView<'r, 'a> {
    pub(crate) target: Object<'a>,
    pub(crate) instantiated: TypeHandle<'r>,
}

/// Borrowed projection of a typed operand.
#[derive(Clone, Copy)]
pub(crate) struct OperandView<'r> {
    pub(crate) ty: TypeHandle<'r>,
}

/// Borrowed projection of a checked call site.
pub(crate) struct CallView<'a> {
    pub(crate) span: [u32; 2],
    pub(crate) callee_ident_span: Option<[u32; 2]>,
    pub(crate) callee_obj: Option<Object<'a>>,
    pub(crate) is_conversion: bool,
}

/// Result of inspecting a type's semantic-token modifier shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TypeShape {
    Form(TypeForm),
    /// Explicitly suppressed (e.g., type parameter or untyped bool/string constant).
    Suppressed,
    Unknown,
}

/// Resolved doc-link component inside a Go doc comment.
pub(crate) struct DocLinkTarget<'a> {
    pub(crate) name: String,
    pub(crate) kind: DocLinkKind<'a>,
}

pub(crate) enum DocLinkKind<'a> {
    Namespace,
    Symbol(&'a IndexedSymbol),
    Member(Object<'a>),
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

/// Checked printf-style call details for format-string highlighting.
pub(crate) struct FormatCallView {
    pub(crate) span: [u32; 2],
    pub(crate) arg_spans: Vec<[u32; 2]>,
    pub(crate) format_index: usize,
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
            ty: TypeHandle::new(sel.ty),
        })
    }

    pub(crate) fn instance_by_span(&self, span: [u32; 2]) -> Option<InstanceView<'r, 'a>> {
        let inst = self.region.instance_by_span(span)?;
        Some(InstanceView {
            target: inst.target,
            instantiated: TypeHandle::new(inst.instantiated),
        })
    }

    pub(crate) fn expr_type(&self, span: [u32; 2]) -> Option<OperandView<'r>> {
        let op = self.region.expr_type(span)?;
        Some(OperandView {
            ty: TypeHandle::new(op.ty),
        })
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

    pub(crate) fn local_binding_rendered_is_error(
        &mut self,
        selection: [u32; 2],
        scope_start: usize,
    ) -> bool {
        let Some(case_pos) = u32::try_from(scope_start).ok() else {
            return false;
        };
        let Some((id, _)) = self.region.local_binding_at(selection, case_pos) else {
            return false;
        };
        let Some(op) = self.region.local_type(id) else {
            return false;
        };
        self.arena
            .render(op.ty, self.file)
            .as_deref()
            .is_some_and(|text| text == "error")
    }

    pub(crate) fn local_operand(&self, id: LocalId) -> Option<OperandView<'r>> {
        let op = self.region.local_type(id)?;
        Some(OperandView {
            ty: TypeHandle::new(op.ty),
        })
    }

    pub(crate) fn calls(&self) -> impl Iterator<Item = CallView<'a>> + '_ {
        self.region.calls.iter().map(|c| CallView {
            span: c.span,
            callee_ident_span: c.callee_ident_span,
            callee_obj: c.callee_obj,
            is_conversion: c.is_conversion,
        })
    }

    pub(crate) fn format_call_at(
        &mut self,
        open_byte: u32,
        close_end: u32,
    ) -> Option<FormatCallView> {
        let idx = self.region.calls.iter().position(|c| {
            !c.is_conversion
                && c.span[1] == close_end
                && (c.open_paren == open_byte
                    || (c.callee_span[1] <= open_byte
                        && self
                            .source
                            .get(c.callee_span[1] as usize..open_byte as usize)
                            .is_some_and(|gap| Lexer::new(gap).next().is_none())))
        })?;
        let call = &self.region.calls[idx];
        let format_index = call_format_parameter_impl(
            self.workspace,
            self.file,
            self.source,
            self.arena,
            call.callee_obj,
            call.signature,
        )?;
        Some(FormatCallView {
            span: call.span,
            arg_spans: call.arg_spans.clone(),
            format_index,
        })
    }

    pub(crate) fn collect_format_spans(
        &mut self,
        max_calls: usize,
        candidates: &mut usize,
        has_candidate_literal: impl Fn(&str) -> bool,
    ) -> Vec<[u32; 2]> {
        let mut out = Vec::new();
        for call in &self.region.calls {
            if *candidates >= max_calls {
                break;
            }
            if call.is_conversion || call.arg_spans.is_empty() {
                continue;
            }
            if !call.arg_spans.iter().any(|&[start, end]| {
                self.source
                    .get(start as usize..end as usize)
                    .is_some_and(&has_candidate_literal)
            }) {
                continue;
            }
            *candidates += 1;
            let Some(format_index) = call_format_parameter_impl(
                self.workspace,
                self.file,
                self.source,
                self.arena,
                call.callee_obj,
                call.signature,
            ) else {
                continue;
            };
            if let Some(&span) = call.arg_spans.get(format_index) {
                out.push(span);
            }
        }
        out
    }

    pub(crate) fn format_type(
        &mut self,
        ty: TypeHandle<'_>,
        qualifier: Qualifier<'_>,
    ) -> Option<String> {
        self.arena.format_type(ty.id, qualifier)
    }

    pub(crate) fn format_type_for_local(
        &mut self,
        ty: TypeHandle<'_>,
        qualifier: Qualifier<'_>,
    ) -> Option<String> {
        self.arena.format_type(ty.id, qualifier).or_else(|| {
            self.arena
                .has_local_declaration(ty.id)
                .filter(|has| *has)
                .and_then(|_| {
                    self.arena
                        .format_type(ty.id, Qualifier::Presentation(self.file))
                })
        })
    }

    pub(crate) fn callable_view(
        &mut self,
        ty: TypeHandle<'_>,
        consumer: &Path,
    ) -> Option<CallableView> {
        CallableView::from_type(self.arena, ty.id, consumer)
    }

    pub(crate) fn type_shape(&mut self, ty: TypeHandle<'_>, constant: bool) -> TypeShape {
        compute_type_shape(self.arena, ty.id, constant)
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
        self.arena.reset_steps();
        let scopes = &self.region.scopes;
        let source = self.source;
        type_form_in_scope(self.arena, file, annotation, |ident| {
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

    pub(crate) fn receiver_is_error_type(&mut self, operand: [u32; 2]) -> bool {
        let Some(found) = self.local_binding_at_span(operand) else {
            return false;
        };
        if annotation_after_decl_name(self.source, found.def_span[1] as usize)
            .is_some_and(|text| text == "error")
        {
            return true;
        }
        if let Some(scope_id) = self.region.scopes.binding_scope(found.id)
            && let Some(node) = self.region.scopes.nodes.get(scope_id.0 as usize)
        {
            return self.local_binding_rendered_is_error(found.def_span, node.extent[0] as usize);
        }
        false
    }

    pub(crate) fn local_has_func_syntax(&self, id: LocalId) -> bool {
        if let Some(init_span) = self
            .region
            .local_inits
            .get(id.0 as usize)
            .copied()
            .flatten()
            && let Some(text) = self
                .source
                .get(init_span[0] as usize..init_span[1] as usize)
            && syntax::Lexer::new(text)
                .next()
                .is_some_and(|t| t.kind == syntax::TokenKind::Func)
        {
            return true;
        }
        let Some(binding) = self.region.scopes.bindings.get(id.0 as usize) else {
            return false;
        };
        matches!(
            binding.kind,
            LocalBindingKind::Param
                | LocalBindingKind::NamedResult
                | LocalBindingKind::Receiver
                | LocalBindingKind::Var
                | LocalBindingKind::Const
        ) && annotation_after_decl_name(self.source, binding.name_span[1] as usize)
            .and_then(|text| syntax::Lexer::new(text).next())
            .is_some_and(|token| token.kind == syntax::TokenKind::Func)
    }

    pub(crate) fn local_binding_annotation_form(&mut self, id: LocalId) -> Option<TypeForm> {
        let binding = self.region.scopes.bindings.get(id.0 as usize)?;
        if matches!(binding.kind, LocalBindingKind::TypeParam) {
            return None;
        }
        let (ann_start, ann_text) =
            annotation_text_after_decl_name(self.source, binding.name_span[1] as usize)?;
        if ann_text.starts_with("...") {
            return Some(TypeForm::Slice);
        }
        let off = u32::try_from(ann_start).ok();
        self.type_form_in_scope(self.file, ann_text, off)
    }

    pub(crate) fn resolve_doc_link(&mut self, name: &str) -> Vec<DocLinkTarget<'a>> {
        doc_link_lookup(self.arena, self.file, name)
    }

    pub(crate) fn imported_name(&self, qualifier: &str) -> bool {
        self.arena.resolver().imported_name(self.file, qualifier)
    }

    /// Separate constant inspection preserving `Option<i128>` when constant value is non-Int.
    /// Formats a checked local variable's type relative to `consumer`, with a
    /// narrow unshadowed-named-annotation fallback when typechecking produced no operand.
    pub(crate) fn local_var_hover_type(
        &mut self,
        id: LocalId,
        def_span: [u32; 2],
        consumer: &Path,
    ) -> Option<String> {
        if let Some(op) = self.region.local_type(id)
            && let Some(rendered) = self
                .format_type_for_local(TypeHandle::new(op.ty), Qualifier::Presentation(consumer))
        {
            return Some(rendered);
        }
        let ann = annotation_after_decl_name(self.source, def_span[1] as usize)?;
        let named = crate::analysis::engine::scope::named_type(ann)?;
        let qualifier = crate::analysis::engine::scope::named_root(&named)
            .split('.')
            .next()?;
        self.region
            .scopes
            .resolve_at(self.source, def_span[1], qualifier)
            .is_none()
            .then_some(named)
    }

    pub(crate) fn local_constant_info_by_id(
        &mut self,
        id: LocalId,
    ) -> Option<(String, Option<i128>, [u32; 2])> {
        let operand = self.region.local_type(id)?;
        (operand.mode == Mode::Constant).then_some(())?;
        let span = self.region.local_init(id)?;
        let type_text = self
            .arena
            .format_type(operand.ty, Qualifier::Presentation(self.file))?;
        let int_val = match operand.value {
            Some(Constant::Int(v)) => Some(v),
            _ => None,
        };
        Some((type_text, int_val, span))
    }

    pub(crate) fn type_declarations_at(
        &mut self,
        offset: u32,
        selected_ident: Option<(std::ops::Range<usize>, bool)>,
    ) -> Result<Vec<TypeDeclaration<'a>>, (i32, String)> {
        if let Some((span, obj)) = self.region.object_at(offset) {
            return match obj {
                Object::PkgName { .. }
                | Object::Label { .. }
                | Object::Nil
                | Object::BuiltinFunction(_) => Ok(Vec::new()),
                Object::TypeParam { span: tp_span, .. } => {
                    match self.source.get(tp_span[0] as usize..tp_span[1] as usize) {
                        Some(name) => {
                            Err((0, format!("cannot find type name(s) from type {name}")))
                        }
                        None => Ok(Vec::new()),
                    }
                }
                Object::Var {
                    origin: SymbolOrigin::Local { id, .. },
                    ..
                }
                | Object::Const(SymbolOrigin::Local { id, .. }) => Ok(self
                    .region
                    .local_type(id)
                    .and_then(|op| self.arena.projected_type_declarations(op.ty))
                    .unwrap_or_else(|| Ok(Vec::new()))?),
                Object::TypeName {
                    origin:
                        SymbolOrigin::Local {
                            file: f, span: s, ..
                        },
                    ..
                } => Ok(match self.workspace.files.get_key_value(f) {
                    Some((file_buf, _)) => {
                        vec![TypeDeclaration::Local(
                            file_buf,
                            s[0] as usize..s[1] as usize,
                        )]
                    }
                    None => Vec::new(),
                }),
                Object::TypeName {
                    origin: SymbolOrigin::Package(decl),
                    ..
                } => Ok(match self.workspace.files.get_key_value(decl.file) {
                    Some((file_buf, _)) => vec![TypeDeclaration::Named(file_buf, decl.symbol)],
                    None => Vec::new(),
                }),
                Object::BuiltinType(name) => Ok(self
                    .arena
                    .builtin_type(self.file, name)
                    .and_then(|ty| self.arena.projected_type_declarations(ty))
                    .unwrap_or_else(|| Ok(Vec::new()))?),
                Object::BuiltinConst(_) => Ok(self
                    .region
                    .expr_type(span)
                    .and_then(|op| self.arena.projected_type_declarations(op.ty))
                    .unwrap_or_else(|| Ok(Vec::new()))?),
                Object::Field(_) | Object::Method(_) => {
                    let ty = if let Some(sel) = self.region.selection_by_span(span) {
                        Some(sel.ty)
                    } else if let Object::Method(MethodOrigin::Concrete(decl)) = obj {
                        self.decl_callable_type(decl)
                    } else {
                        None
                    };
                    Ok(ty
                        .and_then(|ty| self.arena.projected_type_declarations(ty))
                        .unwrap_or_else(|| Ok(Vec::new()))?)
                }
                Object::Func(decl) => {
                    let ty = if let Some(inst) = self.region.instance_by_span(span) {
                        Some(inst.instantiated)
                    } else {
                        self.decl_callable_type(decl)
                    };
                    Ok(ty
                        .and_then(|ty| self.arena.projected_type_declarations(ty))
                        .unwrap_or_else(|| Ok(Vec::new()))?)
                }
                Object::Const(SymbolOrigin::Package(decl))
                | Object::Var {
                    origin: SymbolOrigin::Package(decl),
                    ..
                } => Ok(self
                    .decls
                    .operand_of(self.arena, decl)
                    .and_then(|op| self.arena.projected_type_declarations(op.ty))
                    .unwrap_or_else(|| Ok(Vec::new()))?),
            };
        }
        if let Some((ident_span, false)) = selected_ident {
            let key = [ident_span.start as u32, ident_span.end as u32];
            let guard_ty = self
                .region
                .scopes
                .bindings
                .iter()
                .enumerate()
                .find(|(_, b)| b.kind == LocalBindingKind::TypeSwitchVar && b.name_span == key)
                .and_then(|(idx, _)| self.region.local_init(LocalId(idx as u32)))
                .and_then(|guard_span| self.region.expr_type(guard_span));
            return guard_ty
                .and_then(|op| self.arena.projected_type_declarations(op.ty))
                .unwrap_or_else(|| Ok(Vec::new()));
        }
        if selected_ident.is_none() {
            let expr_cand = self.region.smallest_expr_at(offset);
            let call_cand = self
                .region
                .calls
                .iter()
                .filter(|call| {
                    !call.is_conversion
                        && call.signature.is_some()
                        && call.span[0] <= offset
                        && offset <= call.span[1]
                })
                .min_by_key(|call| call.span[1].saturating_sub(call.span[0]));
            let target_ty = match (expr_cand, call_cand) {
                (Some((es, op)), Some(call)) => {
                    let call_len = call.span[1].saturating_sub(call.span[0]);
                    let expr_len = es[1].saturating_sub(es[0]);
                    if call_len < expr_len {
                        call.signature
                    } else {
                        Some(op.ty)
                    }
                }
                (Some((_, op)), None) => Some(op.ty),
                (None, Some(call)) => call.signature,
                (None, None) => None,
            };
            return target_ty
                .and_then(|ty| self.arena.projected_type_declarations(ty))
                .unwrap_or_else(|| Ok(Vec::new()));
        }
        Ok(Vec::new())
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

    pub(crate) fn type_form_in_scope(&mut self, file: &Path, annotation: &str) -> Option<TypeForm> {
        self.arena.reset_steps();
        type_form_in_scope(self.arena, file, annotation, |_| false)
    }

    pub(crate) fn resolve_doc_link(&mut self, name: &str) -> Vec<DocLinkTarget<'a>> {
        doc_link_lookup(self.arena, self.file, name)
    }

    pub(crate) fn imported_name(&self, qualifier: &str) -> bool {
        self.arena.resolver().imported_name(self.file, qualifier)
    }
}

/// Scans after a declared variable/parameter name (`a, b func(...)`) to locate
/// the trailing type annotation text slice without parsing the outer declaration.
fn annotation_text_after_decl_name(source: &str, mut pos: usize) -> Option<(usize, &str)> {
    let tail = source.get(pos..pos.saturating_add(4096).min(source.len()))?;
    let mut tokens = syntax::Lexer::new(tail).peekable();
    // Skip comma-separated sibling names in `a, b, c T`.
    while tokens
        .peek()
        .is_some_and(|tok| tok.kind == syntax::TokenKind::Comma)
    {
        tokens.next();
        let next = tokens.next()?;
        if next.kind != syntax::TokenKind::Ident {
            return None;
        }
    }
    let first = tokens.peek()?;
    if matches!(
        first.kind,
        syntax::TokenKind::Assign
            | syntax::TokenKind::RParen
            | syntax::TokenKind::Semicolon
            | syntax::TokenKind::RBrace
    ) {
        return None;
    }
    let start = pos + first.span.start;
    let mut depth = 0usize;
    let mut end = start;
    for tok in tokens {
        if matches!(
            tok.kind,
            syntax::TokenKind::LParen | syntax::TokenKind::LBrack | syntax::TokenKind::LBrace
        ) {
            depth += 1;
        } else if depth > 0
            && matches!(
                tok.kind,
                syntax::TokenKind::RParen | syntax::TokenKind::RBrack | syntax::TokenKind::RBrace
            )
        {
            depth -= 1;
        } else if depth == 0
            && matches!(
                tok.kind,
                syntax::TokenKind::Assign
                    | syntax::TokenKind::Comma
                    | syntax::TokenKind::RParen
                    | syntax::TokenKind::Semicolon
                    | syntax::TokenKind::LBrace
            )
        {
            break;
        }
        end = pos + tok.span.end;
    }
    pos = end;
    (start < pos)
        .then(|| source.get(start..pos).map(|s| (start, s.trim_end())))
        .flatten()
}

fn annotation_after_decl_name(source: &str, name_end: usize) -> Option<&str> {
    annotation_text_after_decl_name(source, name_end).map(|(_, s)| s)
}

fn compute_type_shape(arena: &mut TypeArena<'_>, ty: TypeId, constant: bool) -> TypeShape {
    if let Some(root) = arena.underlying(ty)
        && arena.basic_kind(root) == Some(BasicKind::UnsafePointer)
    {
        return TypeShape::Form(TypeForm::Pointer);
    }
    if arena.is_type_param(ty) {
        return TypeShape::Suppressed;
    }
    let Some(form) = arena.form(ty) else {
        return TypeShape::Unknown;
    };
    if constant && arena.is_untyped_bool_or_string(ty) {
        return TypeShape::Suppressed;
    }
    TypeShape::Form(form)
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
        let shape = compute_type_shape(arena, ty, constant);
        if shape != TypeShape::Unknown {
            return shape;
        }
    }
    if decl.symbol.is_type() {
        return type_form_in_scope(arena, decl.file, &decl.symbol.name, |_| false)
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
                return type_form_in_scope(arena, decl.file, ann_text.trim(), |_| false)
                    .map_or(TypeShape::Unknown, TypeShape::Form);
            }
        }
    }
    TypeShape::Unknown
}

fn call_format_parameter_impl<'a>(
    workspace: &'a Workspace,
    file: &'a Path,
    source: &str,
    arena: &mut TypeArena<'_>,
    callee_obj: Option<Object<'a>>,
    signature: Option<TypeId>,
) -> Option<usize> {
    let sig_param = |arena: &mut TypeArena<'_>| signature.and_then(|ty| arena.format_parameter(ty));
    match callee_obj {
        Some(Object::Func(decl)) => {
            decl_format_parameter(workspace, file, source, decl).or_else(|| sig_param(arena))
        }
        Some(Object::Method(MethodOrigin::Concrete(decl))) => {
            sig_param(arena).or_else(|| decl_format_parameter(workspace, file, source, decl))
        }
        _ => sig_param(arena),
    }
}

fn decl_format_parameter(
    workspace: &Workspace,
    file: &Path,
    source: &str,
    decl: DeclId<'_>,
) -> Option<usize> {
    if !decl.symbol.is_callable() {
        return None;
    }
    let range = decl.symbol.declaration()?;
    let end = decl.symbol.body().map_or(range.end, |body| body.start);
    if end.checked_sub(range.start)? > MAX_BYTES {
        return None;
    }
    let header = if decl.file == file {
        std::borrow::Cow::Borrowed(source.get(range.start..end)?)
    } else {
        std::borrow::Cow::Owned(workspace.source_slice(decl.file, range.start..end)?)
    };
    let signature = syntax::parse_signature(&header);
    (signature.diagnostics.is_empty() && signature.parameters.last().is_some_and(|p| p.variadic))
        .then(|| signature.parameters.len().checked_sub(2))
        .flatten()
}

fn doc_link_lookup<'a>(
    arena: &mut TypeArena<'a>,
    file: &Path,
    name: &str,
) -> Vec<DocLinkTarget<'a>> {
    let parts: Vec<&str> = name.split('.').collect();
    if parts.is_empty() || parts.len() > 3 {
        return Vec::new();
    }
    if parts.len() > 1 {
        if import_path(arena, file, parts[0]).is_some() {
            return imported_doc_link(arena, file, parts[0], &parts[1..]);
        }
        return current_member_doc_link(arena, file, parts[0], parts[1], parts.len() == 3);
    }
    let Some((_, symbol)) = arena.resolver().package_symbol(file, parts[0]) else {
        return Vec::new();
    };
    if !resolve::exported(&symbol.name) {
        return Vec::new();
    }
    vec![DocLinkTarget {
        name: parts[0].to_owned(),
        kind: DocLinkKind::Symbol(symbol),
    }]
}

fn import_path<'a>(arena: &TypeArena<'a>, file: &Path, alias: &str) -> Option<&'a str> {
    let resolver = arena.resolver();
    arena
        .workspace()
        .files
        .get(file)?
        .imports
        .iter()
        .find(|import| {
            !import.is_dot()
                && !import.is_blank()
                && resolver.import_name(file, import) == Some(alias)
        })
        .map(|import| import.path.as_str())
}

fn imported_doc_link<'a>(
    arena: &mut TypeArena<'a>,
    file: &Path,
    alias: &str,
    rest: &[&str],
) -> Vec<DocLinkTarget<'a>> {
    let mut objects = vec![DocLinkTarget {
        name: alias.to_owned(),
        kind: DocLinkKind::Namespace,
    }];
    if rest.len() == 1 {
        let name = rest[0];
        let Some((_, symbol)) = arena
            .resolver()
            .imported_symbol_candidates(
                file,
                alias,
                Some(crate::bloom::Fingerprint::new(name.as_bytes())),
            )
            .find(|(_, symbol)| symbol.name == name && symbol.receiver.is_none())
        else {
            return Vec::new();
        };
        if !resolve::exported(name) {
            return Vec::new();
        }
        objects.push(DocLinkTarget {
            name: name.to_owned(),
            kind: DocLinkKind::Symbol(symbol),
        });
        return objects;
    }
    if rest.len() != 2 {
        return Vec::new();
    }
    let (receiver_name, member_name) = (rest[0], rest[1]);
    let Some((origin, receiver)) = arena
        .resolver()
        .resolve_type_declaration(file, &format!("{alias}.{receiver_name}"))
        .filter(|(_, receiver)| !receiver.alias)
    else {
        return Vec::new();
    };
    let Some(member) = arena.declaration_member(
        DeclId {
            file: origin,
            symbol: receiver,
        },
        file,
        member_name,
    ) else {
        return Vec::new();
    };
    objects.push(DocLinkTarget {
        name: receiver_name.to_owned(),
        kind: DocLinkKind::Symbol(receiver),
    });
    objects.push(DocLinkTarget {
        name: member_name.to_owned(),
        kind: DocLinkKind::Member(member),
    });
    objects
}

fn current_member_doc_link<'a>(
    arena: &mut TypeArena<'a>,
    file: &Path,
    receiver_name: &str,
    member_name: &str,
    has_extra: bool,
) -> Vec<DocLinkTarget<'a>> {
    if has_extra {
        return Vec::new();
    }
    let Some((origin, receiver)) = arena
        .resolver()
        .package_symbol(file, receiver_name)
        .filter(|(_, symbol)| symbol.is_type() && !symbol.alias)
    else {
        return Vec::new();
    };
    let Some(member) = arena.declaration_member(
        DeclId {
            file: origin,
            symbol: receiver,
        },
        file,
        member_name,
    ) else {
        return Vec::new();
    };
    vec![
        DocLinkTarget {
            name: receiver_name.to_owned(),
            kind: DocLinkKind::Symbol(receiver),
        },
        DocLinkTarget {
            name: member_name.to_owned(),
            kind: DocLinkKind::Member(member),
        },
    ]
}
