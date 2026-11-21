//! Semantic token classification and type-shape modifiers backed by `Semantics`.
mod comments;
mod format;
mod lexical;
mod roles;

use crate::analysis::query::Context;
use crate::analysis::query::documentation::{self, DocLinkTarget};
use crate::analysis::sema::{
    DeclId, LocalDeclKind, MethodOrigin, PackageView, RegionView, SemanticObject, SymbolOrigin,
    TypeForm, TypeInspector, TypeRef, TypeShape, VarKind, decl_object,
    imported_name_in as imported_name, is_builtin_function as builtin_function,
    is_builtin_type as builtin_type, is_builtin_universe as builtin,
};
use crate::index::IndexedSymbol;
use crate::workspace::Workspace;
use roles::{TokenLayout, TokenName, TokenRole};
use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SemanticTokenKind {
    Namespace,
    Type,
    Function,
    Method,
    Variable,
    Property,
    Parameter,
    Keyword,
    String,
    Number,
    Comment,
    Operator,
    TypeParameter,
}

impl SemanticTokenKind {
    pub(crate) const ALL: [Self; 13] = [
        Self::Namespace,
        Self::Type,
        Self::Function,
        Self::Method,
        Self::Variable,
        Self::Property,
        Self::Parameter,
        Self::Keyword,
        Self::String,
        Self::Number,
        Self::Comment,
        Self::Operator,
        Self::TypeParameter,
    ];

    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Namespace => "namespace",
            Self::Type => "type",
            Self::Function => "function",
            Self::Method => "method",
            Self::Variable => "variable",
            Self::Property => "property",
            Self::Parameter => "parameter",
            Self::Keyword => "keyword",
            Self::String => "string",
            Self::Number => "number",
            Self::Comment => "comment",
            Self::Operator => "operator",
            Self::TypeParameter => "typeParameter",
        }
    }
}

pub(crate) mod modifiers {
    pub(crate) const DEFINITION: u64 = 1 << 1;
    pub(crate) const READONLY: u64 = 1 << 2;
    pub(crate) const DEFAULT_LIBRARY: u64 = 1 << 3;
    pub(crate) const STATIC: u64 = 1 << 4;
    pub(crate) const SIGNATURE: u64 = 1 << 5;
    pub(crate) const NUMBER: u64 = 1 << 6;
    pub(crate) const BOOL: u64 = 1 << 7;
    pub(crate) const STRING: u64 = 1 << 8;
    pub(crate) const INTERFACE: u64 = 1 << 9;
    pub(crate) const STRUCT: u64 = 1 << 10;
    pub(crate) const POINTER: u64 = 1 << 11;
    pub(crate) const ARRAY: u64 = 1 << 12;
    pub(crate) const SLICE: u64 = 1 << 13;
    pub(crate) const MAP: u64 = 1 << 14;
    pub(crate) const CHAN: u64 = 1 << 15;
    pub(crate) const SHADOWING: u64 = 1 << 16;
    /// Non-standard modifier for printf format directives inside string literals.
    pub(crate) const FORMAT: u64 = 1 << 17;
}

pub(crate) struct SemanticToken {
    pub(crate) span: Range<usize>,
    pub(crate) kind: SemanticTokenKind,
    pub(crate) modifiers: u64,
}

impl<'a> Context<'a> {
    pub(crate) fn semantic_tokens(
        &mut self,
        window: Option<Range<usize>>,
        include_shapes: bool,
    ) -> Result<Vec<SemanticToken>, (i32, String)> {
        let workspace = self.workspace();
        let file = self.file();
        let source = self.source();
        let Some(stub) = workspace.files.get(file) else {
            return Ok(Vec::new());
        };
        let window_bounds = window.as_ref().map(|w| (w.start, w.end));
        let mut layout = TokenLayout::collect(workspace, file, source, stub, window_bounds)?;
        let builtin_buf = workspace.builtin_file();
        let builtin_file = builtin_buf.as_deref();

        let in_window = move |span: &Range<usize>| {
            window_bounds.is_none_or(|(start, end)| span.start < end && span.end > start)
        };

        let mut exact_symbols: HashMap<[u32; 2], (&'a Path, &'a IndexedSymbol)> = HashMap::new();
        for symbol in &stub.symbols {
            exact_symbols.insert([symbol.start as u32, symbol.end as u32], (file, symbol));
        }

        let mut outer_package: HashMap<&'a str, Option<(&'a PathBuf, &'a IndexedSymbol)>> =
            HashMap::new();
        for (path, symbol) in crate::analysis::resolve::file_symbol_iter(workspace, file) {
            if symbol.name == "_" {
                continue;
            }
            outer_package
                .entry(symbol.name.as_str())
                .and_modify(|entry| *entry = None)
                .or_insert(Some((path, symbol)));
        }

        let mut tokens = Vec::new();
        let has_potential_format = source.contains('%') || source.contains('\\');
        let (doc, sema) = self.split();
        let source = doc.source();
        let mut raw_lexical = lexical::tokens(source)
            .filter(|(span, _)| in_window(span))
            .peekable();

        let mut formatting: HashMap<Range<usize>, Vec<Range<usize>>> = HashMap::new();
        let mut printf_candidates = 0usize;
        let mut name_idx = 0usize;
        let mut region_error = None;
        sema.visit_file_regions(file, source, window, |view| {
            if region_error.is_some() {
                return;
            }
            let region_span = view.span();
            {
                let state = ClassifyEnv {
                    workspace,
                    file,
                    source,
                    layout: &layout,
                    builtin_file,
                    exact_symbols: &exact_symbols,
                    outer_package: &outer_package,
                    include_shapes,
                };
                while let Some(name) = layout.names.get(name_idx) {
                    if name.span.start >= region_span[0] as usize {
                        break;
                    }
                    if in_window(&name.span) {
                        let mut scope = SemaScope::Region(view, false);
                        push_window_name(
                            &state,
                            &formatting,
                            window_bounds,
                            &mut raw_lexical,
                            &mut tokens,
                            name,
                            &mut scope,
                        );
                    }
                    name_idx += 1;
                }
            }
            if let Err(err) = layout.classify_region(
                source,
                region_span,
                view.syntax_signature(),
                view.syntax_body(),
            ) {
                region_error = Some(err);
                return;
            }
            if has_potential_format {
                crate::analysis::query::highlight::collect_region_format_directives(
                    workspace,
                    file,
                    source,
                    view,
                    window_bounds,
                    &mut printf_candidates,
                    &mut formatting,
                );
            }
            let state = ClassifyEnv {
                workspace,
                file,
                source,
                layout: &layout,
                builtin_file,
                exact_symbols: &exact_symbols,
                outer_package: &outer_package,
                include_shapes,
            };
            while let Some(name) = layout.names.get(name_idx) {
                if name.span.start >= region_span[1] as usize {
                    break;
                }
                if in_window(&name.span) {
                    let mut scope = SemaScope::Region(view, true);
                    push_window_name(
                        &state,
                        &formatting,
                        window_bounds,
                        &mut raw_lexical,
                        &mut tokens,
                        name,
                        &mut scope,
                    );
                }
                name_idx += 1;
            }
        });
        if let Some(err) = region_error {
            return Err(err);
        }

        if let Some(mut pkg) = sema.package_view(file, source) {
            let state = ClassifyEnv {
                workspace,
                file,
                source,
                layout: &layout,
                builtin_file,
                exact_symbols: &exact_symbols,
                outer_package: &outer_package,
                include_shapes,
            };
            let mut scope = SemaScope::Package(&mut pkg);
            while let Some(name) = layout.names.get(name_idx) {
                if in_window(&name.span) {
                    push_window_name(
                        &state,
                        &formatting,
                        window_bounds,
                        &mut raw_lexical,
                        &mut tokens,
                        name,
                        &mut scope,
                    );
                }
                name_idx += 1;
            }

            for (span, kind) in raw_lexical {
                emit_raw_lexical_token(
                    &state,
                    &formatting,
                    window_bounds,
                    span,
                    kind,
                    &mut tokens,
                    &mut scope,
                );
            }
        }

        Ok(tokens)
    }
}

enum SemaScope<'b, 'r, 'a> {
    Region(&'b mut RegionView<'r, 'a>, bool),
    Package(&'b mut PackageView<'r, 'a>),
}

impl<'r, 'a> SemaScope<'_, 'r, 'a> {
    fn region(&self) -> Option<&RegionView<'r, 'a>> {
        match self {
            Self::Region(view, true) => Some(view),
            Self::Region(_, false) | Self::Package(_) => None,
        }
    }

    fn region_mut(&mut self) -> Option<&mut RegionView<'r, 'a>> {
        match self {
            Self::Region(view, true) => Some(view),
            Self::Region(_, false) | Self::Package(_) => None,
        }
    }

    fn reset_steps(&mut self) {
        match self {
            Self::Region(view, _) => view.reset_steps(),
            Self::Package(pkg) => pkg.reset_steps(),
        }
    }

    fn decl_type_shape(&mut self, decl: DeclId<'a>, constant: bool) -> TypeShape {
        match self {
            Self::Region(view, _) => view.decl_type_shape(decl, constant),
            Self::Package(pkg) => pkg.decl_type_shape(decl, constant),
        }
    }

    fn types(&mut self) -> TypeInspector<'_, 'r, 'a> {
        match self {
            Self::Region(view, _) => view.types(),
            Self::Package(pkg) => pkg.types(),
        }
    }

    fn type_shape(&mut self, ty: TypeRef<'r>, constant: bool) -> TypeShape {
        self.types().type_shape(ty, constant)
    }

    fn type_form_in_scope(
        &mut self,
        file: &Path,
        annotation: &str,
        scope_offset: Option<u32>,
    ) -> Option<TypeForm> {
        match self {
            Self::Region(view, _) => view.type_form_in_scope(file, annotation, scope_offset),
            Self::Package(pkg) => pkg.types().type_form_in_scope(file, annotation, |_| false),
        }
    }

    fn is_local_defined_at(&self, offset: u32, ident: &str) -> bool {
        match self {
            Self::Region(view, _) => view.is_local_defined_at(offset, ident),
            Self::Package(_) => false,
        }
    }

    fn resolver(&self) -> crate::analysis::resolve::View<'a> {
        match self {
            Self::Region(view, _) => view.resolver(),
            Self::Package(pkg) => pkg.resolver(),
        }
    }

    fn declaration_member(
        &mut self,
        declaration: DeclId<'a>,
        name: &str,
    ) -> Option<SemanticObject<'a>> {
        match self {
            Self::Region(view, _) => view.declaration_member(declaration, name),
            Self::Package(pkg) => pkg.declaration_member(declaration, name),
        }
    }

    fn resolve_doc_link(&mut self, file: &Path, name: &str) -> Vec<DocLinkTarget<'a>> {
        let resolver = self.resolver();
        documentation::resolve_doc_link(resolver, file, name, |decl, member| {
            self.declaration_member(decl, member)
        })
    }

    fn imported_name(&self, qualifier: &str) -> bool {
        match self {
            Self::Region(view, _) => view.imported_name(qualifier),
            Self::Package(pkg) => pkg.imported_name(qualifier),
        }
    }
}

fn emit_raw_lexical_token<'r, 'a>(
    state: &ClassifyEnv<'_, '_>,
    formatting: &HashMap<Range<usize>, Vec<Range<usize>>>,
    window_bounds: Option<(usize, usize)>,
    span: Range<usize>,
    kind: SemanticTokenKind,
    tokens: &mut Vec<SemanticToken>,
    scope: &mut SemaScope<'_, 'r, 'a>,
) {
    let in_window =
        |s: &Range<usize>| window_bounds.is_none_or(|(start, end)| s.start < end && s.end > start);
    let push_if_valid =
        |s: Range<usize>, k: SemanticTokenKind, m: u64, out: &mut Vec<SemanticToken>| {
            if in_window(&s)
                && (!matches!(&state.source[s.clone()], "=" | ":=")
                    || !state.layout.declaration_operator(s.start))
            {
                out.push(SemanticToken {
                    span: s,
                    kind: k,
                    modifiers: m,
                });
            }
        };
    match kind {
        SemanticTokenKind::Comment => {
            for (s, k, m) in comments::split(state.source, &span, |name| {
                scope.resolve_doc_link(state.file, name)
            }) {
                push_if_valid(s, k, m, tokens);
            }
        }
        SemanticTokenKind::String => {
            // gopls colors multiline strings as plain string tokens and
            // skips printf splitting for them entirely.
            if !state.source[span.clone()].contains('\n')
                && let Some(directives) = formatting.get(&span)
            {
                for (s, k, m) in format::split(span, directives) {
                    push_if_valid(s, k, m, tokens);
                }
            } else {
                push_if_valid(span, kind, 0, tokens);
            }
        }
        _ => push_if_valid(span, kind, 0, tokens),
    }
}

#[allow(clippy::too_many_arguments)]
fn push_window_name<'a, 'q, 'r>(
    state: &ClassifyEnv<'a, 'q>,
    formatting: &HashMap<Range<usize>, Vec<Range<usize>>>,
    window_bounds: Option<(usize, usize)>,
    lexical: &mut std::iter::Peekable<impl Iterator<Item = (Range<usize>, SemanticTokenKind)>>,
    tokens: &mut Vec<SemanticToken>,
    name: &TokenName,
    scope: &mut SemaScope<'_, 'r, 'a>,
) {
    while lexical
        .peek()
        .is_some_and(|(span, _)| span.start < name.span.start)
    {
        let (span, kind) = lexical.next().expect("peeked lexical token");
        emit_raw_lexical_token(state, formatting, window_bounds, span, kind, tokens, scope);
    }

    if matches!(name.role, TokenRole::Ignored)
        || matches!(name.role, TokenRole::Namespace) && &state.source[name.span.clone()] == "_"
    {
        return;
    }

    if matches!(name.role, TokenRole::Namespace) {
        tokens.push(SemanticToken {
            span: name.span.clone(),
            kind: SemanticTokenKind::Namespace,
            modifiers: 0,
        });
        return;
    }

    let (kind, mods) = state.classify_name(scope, name);
    tokens.push(SemanticToken {
        span: name.span.clone(),
        kind,
        modifiers: mods,
    });
}

struct ClassifyEnv<'a, 'q> {
    workspace: &'a Workspace,
    file: &'a Path,
    source: &'q str,
    layout: &'q TokenLayout,
    builtin_file: Option<&'q Path>,
    exact_symbols: &'q HashMap<[u32; 2], (&'a Path, &'a IndexedSymbol)>,
    outer_package: &'q HashMap<&'a str, Option<(&'a PathBuf, &'a IndexedSymbol)>>,
    include_shapes: bool,
}

impl<'a, 'q> ClassifyEnv<'a, 'q> {
    fn has_outer_name(&self, name: &str) -> bool {
        self.outer_package.get(name).copied().flatten().is_some()
            || imported_name(self.workspace, self.file, name)
    }

    fn classify_name<'r>(
        &self,
        scope: &mut SemaScope<'_, 'r, 'a>,
        name: &TokenName,
    ) -> (SemanticTokenKind, u64) {
        scope.reset_steps();
        let span_u32 = [name.span.start as u32, name.span.end as u32];
        let spelling = &self.source[name.span.clone()];

        let mut is_def = false;
        let mut obj = None;
        let mut local_ty: Option<TypeRef<'r>> = None;
        let mut expr_ty: Option<TypeRef<'r>> = None;
        let mut sel_ty: Option<TypeRef<'r>> = None;

        if let Some(region) = scope.region() {
            if let Some(found) = region.def_by_span(span_u32) {
                is_def = true;
                obj = Some(found);
            } else if let Some(found) = region.use_by_span(span_u32) {
                obj = Some(found);
            }
            if let Some(
                SemanticObject::Var {
                    origin: SymbolOrigin::Local { id, .. },
                    ..
                }
                | SemanticObject::Const(SymbolOrigin::Local { id, .. })
                | SemanticObject::TypeName {
                    origin: SymbolOrigin::Local { id, .. },
                    ..
                },
            ) = obj
            {
                local_ty = region.local_operand(id).map(|op| op.ty);
            }
            expr_ty = region.expr_type(span_u32).map(|op| op.ty);
            if let Some(sel) = region.selection_by_span(span_u32) {
                sel_ty = Some(sel.ty);
                if obj.is_none() {
                    obj = Some(sel.obj);
                }
            }
        }

        if obj.is_none()
            && let Some(&(decl_file, sym)) = self.exact_symbols.get(&span_u32)
        {
            is_def = true;
            let decl = DeclId {
                file: decl_file,
                symbol: sym,
            };
            obj = Some(decl_object(decl));
        }

        if let TokenRole::EmbeddedType(qualifier) = &name.role {
            let shape = if !self.include_shapes {
                0
            } else if let Some(SemanticObject::TypeName { origin, .. }) = obj {
                match origin {
                    SymbolOrigin::Package(decl) => {
                        shape_of_result(scope.decl_type_shape(decl, false))
                    }
                    SymbolOrigin::Local { .. } => local_ty
                        .or(expr_ty)
                        .map_or(0, |ty| shape_of_handle(scope, ty, false)),
                }
            } else if obj.is_none() {
                let target_text = qualifier
                    .as_ref()
                    .and_then(|q| {
                        let q_text = self.source.get(q.clone())?;
                        Some(format!("{q_text}.{spelling}"))
                    })
                    .unwrap_or_else(|| spelling.to_owned());
                crate::analysis::resolve::resolve_type_declaration(
                    self.workspace,
                    self.file,
                    &target_text,
                )
                .and_then(|(decl_file, sym)| scope.type_form_in_scope(decl_file, &sym.name, None))
                .map_or(0, shape_of_form)
            } else {
                0
            };
            return (SemanticTokenKind::Property, modifiers::DEFINITION | shape);
        }

        if let Some(found) = obj {
            return self.classify_object(
                scope, name, span_u32, spelling, is_def, found, local_ty, expr_ty, sel_ty,
            );
        }

        self.classify_fallback(scope, name, spelling, expr_ty)
    }

    #[allow(clippy::too_many_arguments)]
    fn classify_object<'r>(
        &self,
        scope: &mut SemaScope<'_, 'r, 'a>,
        name: &TokenName,
        span_u32: [u32; 2],
        spelling: &str,
        is_def: bool,
        obj: SemanticObject<'a>,
        local_ty: Option<TypeRef<'r>>,
        expr_ty: Option<TypeRef<'r>>,
        sel_ty: Option<TypeRef<'r>>,
    ) -> (SemanticTokenKind, u64) {
        let mut mods = if matches!(name.role, TokenRole::FieldDeclaration(_)) {
            modifiers::DEFINITION
        } else {
            0
        };

        match obj {
            SemanticObject::PkgName { .. } => (SemanticTokenKind::Namespace, 0),
            SemanticObject::BuiltinFunction(_) => {
                (SemanticTokenKind::Function, modifiers::DEFAULT_LIBRARY)
            }
            SemanticObject::BuiltinType(b) => {
                let shape = if self.include_shapes {
                    TypeForm::builtin(b).map_or(0, shape_of_form)
                } else {
                    0
                };
                (SemanticTokenKind::Type, modifiers::DEFAULT_LIBRARY | shape)
            }
            SemanticObject::BuiltinConst(b) => {
                let shape = if self.include_shapes && b == "iota" {
                    modifiers::NUMBER
                } else {
                    0
                };
                (
                    SemanticTokenKind::Variable,
                    modifiers::DEFAULT_LIBRARY | modifiers::READONLY | shape,
                )
            }
            SemanticObject::Nil => (
                SemanticTokenKind::Variable,
                modifiers::DEFAULT_LIBRARY | modifiers::READONLY,
            ),
            SemanticObject::Label { span: def_span, .. } => {
                if is_def || def_span == span_u32 {
                    mods |= modifiers::DEFINITION;
                }
                (SemanticTokenKind::Variable, mods)
            }
            SemanticObject::TypeParam { span: def_span, .. } => {
                if is_def || def_span == span_u32 {
                    mods |= modifiers::DEFINITION;
                    if self.local_shadows(scope, def_span, spelling) {
                        mods |= modifiers::SHADOWING;
                    }
                }
                (SemanticTokenKind::TypeParameter, mods)
            }
            SemanticObject::TypeName { origin, .. } => {
                let shape = match origin {
                    SymbolOrigin::Package(decl) => {
                        mods |= self.package_decl_modifiers(decl, name, spelling);
                        if self.include_shapes {
                            shape_of_result(scope.decl_type_shape(decl, false))
                        } else {
                            0
                        }
                    }
                    SymbolOrigin::Local { span: def_span, .. } => {
                        mods |=
                            self.local_def_modifiers(scope, is_def, def_span, span_u32, spelling);
                        if self.include_shapes {
                            local_ty
                                .or(expr_ty)
                                .map_or(0, |ty| shape_of_handle(scope, ty, false))
                        } else {
                            0
                        }
                    }
                };
                (SemanticTokenKind::Type, mods | shape)
            }
            SemanticObject::Const(origin) => {
                mods |= modifiers::READONLY;
                let shape = match origin {
                    SymbolOrigin::Package(decl) => {
                        mods |= self.package_decl_modifiers(decl, name, spelling);
                        if self.include_shapes {
                            shape_of_result(scope.decl_type_shape(decl, true))
                        } else {
                            0
                        }
                    }
                    SymbolOrigin::Local { span: def_span, .. } => {
                        mods |=
                            self.local_def_modifiers(scope, is_def, def_span, span_u32, spelling);
                        if self.include_shapes {
                            local_ty
                                .or(expr_ty)
                                .map_or(0, |ty| shape_of_handle(scope, ty, true))
                        } else {
                            0
                        }
                    }
                };
                (SemanticTokenKind::Variable, mods | shape)
            }
            SemanticObject::Var {
                origin,
                kind: var_kind,
                ..
            } => {
                let token_kind = match var_kind {
                    VarKind::Receiver | VarKind::Param => SemanticTokenKind::Parameter,
                    VarKind::ResultVar | VarKind::Local | VarKind::Package => {
                        SemanticTokenKind::Variable
                    }
                };
                let shape = match origin {
                    SymbolOrigin::Package(decl) => {
                        mods |= modifiers::STATIC;
                        mods |= self.package_decl_modifiers(decl, name, spelling);
                        if self.include_shapes {
                            shape_of_result(scope.decl_type_shape(decl, false))
                        } else {
                            0
                        }
                    }
                    SymbolOrigin::Local {
                        id, span: def_span, ..
                    } => {
                        mods |=
                            self.local_def_modifiers(scope, is_def, def_span, span_u32, spelling);
                        // gopls omits shadowing for parameters in a function
                        // type: there is no function body in which to use them.
                        if scope
                            .region()
                            .is_some_and(|region| region.binding_is_header_only_param(id))
                        {
                            mods &= !modifiers::SHADOWING;
                        }
                        let ty = local_ty.or(expr_ty);
                        if ty.is_some_and(|ty| {
                            scope
                                .region_mut()
                                .is_some_and(|region| region.types().is_callable(ty) == Some(true))
                        }) {
                            mods |= modifiers::SIGNATURE;
                        }
                        if !self.include_shapes {
                            0
                        } else {
                            ty.map_or(0, |ty| shape_of_handle(scope, ty, false))
                        }
                    }
                };
                (token_kind, mods | shape)
            }
            SemanticObject::Func(decl) => {
                mods |= modifiers::SIGNATURE;
                mods |= self.package_decl_modifiers(decl, name, spelling);
                (SemanticTokenKind::Function, mods)
            }
            SemanticObject::Method(origin) => {
                mods |= modifiers::SIGNATURE;
                match origin {
                    MethodOrigin::Concrete(decl) => {
                        if decl.file == self.file
                            && decl.symbol.start == name.span.start
                            && decl.symbol.end == name.span.end
                        {
                            mods |= modifiers::DEFINITION;
                        }
                        if self.builtin_file == Some(decl.file) {
                            mods |= modifiers::DEFAULT_LIBRARY;
                        }
                    }
                    MethodOrigin::Interface(member) => {
                        if member.file == self.file
                            && matches!(name.role, TokenRole::FieldDeclaration(_))
                        {
                            mods |= modifiers::DEFINITION;
                        }
                        if member.builtin || self.builtin_file == Some(member.file) {
                            mods |= modifiers::DEFAULT_LIBRARY;
                        }
                    }
                }
                (SemanticTokenKind::Method, mods)
            }
            SemanticObject::Field(member) => {
                if member.file == self.file && matches!(name.role, TokenRole::FieldDeclaration(_)) {
                    mods |= modifiers::DEFINITION;
                }
                if member.builtin || self.builtin_file == Some(member.file) {
                    mods |= modifiers::DEFAULT_LIBRARY;
                }
                let shape = if !self.include_shapes {
                    0
                } else if let Some(ty) = sel_ty.or(expr_ty) {
                    shape_of_handle(scope, ty, false)
                } else {
                    member
                        .describe(self.workspace)
                        .and_then(|decl| decl.data(self.workspace))
                        .and_then(|data| data.annotation)
                        .and_then(|ann| scope.type_form_in_scope(member.file, &ann, None))
                        .map_or(0, shape_of_form)
                };
                (SemanticTokenKind::Property, mods | shape)
            }
        }
    }

    fn classify_fallback<'r>(
        &self,
        scope: &mut SemaScope<'_, 'r, 'a>,
        name: &TokenName,
        spelling: &str,
        expr_ty: Option<TypeRef<'r>>,
    ) -> (SemanticTokenKind, u64) {
        let span_u32 = [name.span.start as u32, name.span.end as u32];
        if let Some(binding) = scope
            .region()
            .filter(|_| {
                matches!(
                    name.role,
                    TokenRole::Name
                        | TokenRole::Type(None)
                        | TokenRole::LiteralKey(_)
                        | TokenRole::LiteralElementKey(_)
                )
            })
            .and_then(|region| region.local_binding_at_span(span_u32))
        {
            let kind = match binding.kind {
                LocalDeclKind::Type => SemanticTokenKind::Type,
                LocalDeclKind::Param => SemanticTokenKind::Parameter,
                LocalDeclKind::TypeParam => SemanticTokenKind::TypeParameter,
                LocalDeclKind::Var | LocalDeclKind::Const | LocalDeclKind::TypeSwitchVar => {
                    SemanticTokenKind::Variable
                }
            };
            let mut mods = 0u64;
            if binding.is_def {
                mods |= modifiers::DEFINITION;
                if self.local_shadows(scope, binding.def_span, spelling) {
                    mods |= modifiers::SHADOWING;
                }
            }
            match binding.kind {
                LocalDeclKind::Const => mods |= modifiers::READONLY,
                LocalDeclKind::TypeParam => return (SemanticTokenKind::TypeParameter, mods),
                _ => {}
            }
            let ty = if binding.is_def && binding.kind == LocalDeclKind::TypeSwitchVar {
                None
            } else {
                scope
                    .region()
                    .and_then(|region| region.local_operand(binding.id).map(|tv| tv.ty))
                    .or(expr_ty)
            };
            if ty.is_some_and(|ty| {
                scope
                    .region_mut()
                    .is_some_and(|region| region.types().is_callable(ty) == Some(true))
            }) {
                mods |= modifiers::SIGNATURE;
            }
            let shape = if !self.include_shapes {
                0
            } else {
                ty.map_or(0, |ty| {
                    shape_of_handle(scope, ty, binding.kind == LocalDeclKind::Const)
                })
            };
            return (kind, mods | shape);
        }

        match &name.role {
            TokenRole::BlankDeclaration { is_type, constant } => (
                if *is_type {
                    SemanticTokenKind::Type
                } else {
                    SemanticTokenKind::Variable
                },
                modifiers::DEFINITION | if *constant { modifiers::READONLY } else { 0 },
            ),
            TokenRole::FieldDeclaration(owner_idx) => {
                let is_interface_method = self
                    .workspace
                    .files
                    .get(self.file)
                    .and_then(|stub| {
                        stub.symbols
                            .iter()
                            .find(|s| s.start == *owner_idx && s.is_type())
                    })
                    .and_then(|sym| sym.type_info.as_ref())
                    .is_some_and(|info| info.is_interface());
                if is_interface_method {
                    (
                        SemanticTokenKind::Method,
                        modifiers::DEFINITION | modifiers::SIGNATURE,
                    )
                } else {
                    let shape = if self.include_shapes {
                        expr_ty.map_or(0, |ty| shape_of_handle(scope, ty, false))
                    } else {
                        0
                    };
                    (SemanticTokenKind::Property, modifiers::DEFINITION | shape)
                }
            }
            TokenRole::Parameter | TokenRole::Result => {
                let kind = if matches!(name.role, TokenRole::Parameter) {
                    SemanticTokenKind::Parameter
                } else {
                    SemanticTokenKind::Variable
                };
                let shape = if !self.include_shapes {
                    0
                } else if let Some(ty) = expr_ty {
                    shape_of_handle(scope, ty, false)
                } else if let Some((span, variadic)) = self.layout.signature_type(name.span.start) {
                    if variadic {
                        modifiers::SLICE
                    } else {
                        self.source
                            .get(span.clone())
                            .and_then(|text| {
                                scope.type_form_in_scope(
                                    self.file,
                                    text,
                                    u32::try_from(span.start).ok(),
                                )
                            })
                            .map_or(0, shape_of_form)
                    }
                } else {
                    0
                };
                (kind, modifiers::DEFINITION | shape)
            }
            TokenRole::Type(Some(qualifier)) => {
                let q_text = self.source.get(qualifier.clone()).unwrap_or_default();
                if imported_name(self.workspace, self.file, q_text) {
                    (
                        unresolved_selector_token_kind(self.source, name.span.end),
                        0,
                    )
                } else {
                    (SemanticTokenKind::Type, 0)
                }
            }
            TokenRole::Type(None) => {
                if spelling == "_" && scope.region().is_some_and(|r| r.is_range_blank(span_u32)) {
                    let shape = if self.include_shapes {
                        expr_ty.map_or(0, |ty| shape_of_handle(scope, ty, false))
                    } else {
                        0
                    };
                    return (SemanticTokenKind::Variable, modifiers::DEFINITION | shape);
                }
                if builtin_type(spelling) {
                    let shape = if self.include_shapes {
                        TypeForm::builtin(spelling).map_or(0, shape_of_form)
                    } else {
                        0
                    };
                    (SemanticTokenKind::Type, modifiers::DEFAULT_LIBRARY | shape)
                } else {
                    (SemanticTokenKind::Type, 0)
                }
            }
            TokenRole::Name => {
                if spelling == "_" && scope.region().is_some_and(|r| r.is_range_blank(span_u32)) {
                    let shape = if self.include_shapes {
                        expr_ty.map_or(0, |ty| shape_of_handle(scope, ty, false))
                    } else {
                        0
                    };
                    return (SemanticTokenKind::Variable, modifiers::DEFINITION | shape);
                }
                if spelling == "_" {
                    return (SemanticTokenKind::Variable, 0);
                }
                if imported_name(self.workspace, self.file, spelling) {
                    return (SemanticTokenKind::Namespace, 0);
                }
                if let Some((path, sym)) = self.outer_package.get(spelling).copied().flatten() {
                    let decl = DeclId {
                        file: path.as_path(),
                        symbol: sym,
                    };
                    let obj = decl_object(decl);
                    return self.classify_object(
                        scope,
                        name,
                        [name.span.start as u32, name.span.end as u32],
                        spelling,
                        false,
                        obj,
                        None,
                        expr_ty,
                        None,
                    );
                }
                if builtin_type(spelling) {
                    let shape = if self.include_shapes {
                        TypeForm::builtin(spelling).map_or(0, shape_of_form)
                    } else {
                        0
                    };
                    return (SemanticTokenKind::Type, modifiers::DEFAULT_LIBRARY | shape);
                }
                if builtin_function(spelling) {
                    return (SemanticTokenKind::Function, modifiers::DEFAULT_LIBRARY);
                }
                if matches!(spelling, "true" | "false" | "nil" | "iota") {
                    let shape = if self.include_shapes && spelling == "iota" {
                        modifiers::NUMBER
                    } else {
                        0
                    };
                    return (
                        SemanticTokenKind::Variable,
                        modifiers::DEFAULT_LIBRARY | modifiers::READONLY | shape,
                    );
                }
                (SemanticTokenKind::Variable, 0)
            }
            TokenRole::Member(_) | TokenRole::ExpressionMember(_) => {
                let mut kind = if is_unresolved_imported_selector(
                    scope,
                    self.workspace,
                    self.file,
                    self.source,
                    name,
                ) {
                    unresolved_selector_token_kind(self.source, name.span.end)
                } else {
                    match unresolved_selector_token_kind(self.source, name.span.end) {
                        SemanticTokenKind::Function => SemanticTokenKind::Function,
                        _ => SemanticTokenKind::Property,
                    }
                };
                let mut mods = 0u64;
                if spelling == "Error"
                    && let Some(operand) = match &name.role {
                        TokenRole::Member(Some(span)) | TokenRole::ExpressionMember(span) => {
                            Some(span.clone())
                        }
                        _ => None,
                    }
                    && let (Ok(s0), Ok(s1)) =
                        (u32::try_from(operand.start), u32::try_from(operand.end))
                    && scope.region_mut().is_some_and(|region| {
                        let ty = region.expr_type([s0, s1]).map(|tv| tv.ty).or_else(|| {
                            let binding = region.local_binding_at_span([s0, s1])?;
                            region.local_operand(binding.id).map(|tv| tv.ty)
                        });
                        ty.is_some_and(|ty| region.types().is_error(ty))
                    })
                {
                    kind = SemanticTokenKind::Method;
                    mods |= modifiers::DEFAULT_LIBRARY | modifiers::SIGNATURE;
                }
                (kind, mods)
            }
            TokenRole::LiteralKey(_) | TokenRole::LiteralElementKey(_) => {
                if unresolved_imported_literal_type(scope, self.layout, self.source, name) {
                    (SemanticTokenKind::Variable, 0)
                } else {
                    (SemanticTokenKind::Property, 0)
                }
            }
            TokenRole::Field => (SemanticTokenKind::Property, 0),
            TokenRole::Ignored | TokenRole::Namespace | TokenRole::EmbeddedType(_) => {
                (SemanticTokenKind::Variable, 0)
            }
        }
    }

    fn package_decl_modifiers(&self, decl: DeclId<'a>, name: &TokenName, spelling: &str) -> u64 {
        let mut mods = 0u64;
        if decl.file == self.file
            && decl.symbol.start == name.span.start
            && decl.symbol.end == name.span.end
        {
            mods |= modifiers::DEFINITION;
            if !decl.symbol.is_method() && builtin(spelling) {
                mods |= modifiers::SHADOWING;
            }
        }
        mods
    }

    fn local_def_modifiers<'r>(
        &self,
        scope: &SemaScope<'_, 'r, 'a>,
        is_def: bool,
        def_span: [u32; 2],
        span_u32: [u32; 2],
        spelling: &str,
    ) -> u64 {
        let mut mods = 0u64;
        if is_def || def_span == span_u32 {
            mods |= modifiers::DEFINITION;
            if self.local_shadows(scope, def_span, spelling) {
                mods |= modifiers::SHADOWING;
            }
        }
        mods
    }

    fn local_shadows<'r>(
        &self,
        scope: &SemaScope<'_, 'r, 'a>,
        def_span: [u32; 2],
        spelling: &str,
    ) -> bool {
        let has_outer_local = scope
            .region()
            .is_some_and(|region| region.local_shadows(def_span, spelling));
        has_outer_local || self.has_outer_name(spelling) || builtin(spelling)
    }
}

fn shape_of_form(form: TypeForm) -> u64 {
    match form {
        TypeForm::Number => modifiers::NUMBER,
        TypeForm::Bool => modifiers::BOOL,
        TypeForm::String => modifiers::STRING,
        TypeForm::Interface => modifiers::INTERFACE,
        TypeForm::Struct => modifiers::STRUCT,
        TypeForm::Signature => modifiers::SIGNATURE,
        TypeForm::Pointer => modifiers::POINTER,
        TypeForm::Array => modifiers::ARRAY,
        TypeForm::Slice => modifiers::SLICE,
        TypeForm::Map => modifiers::MAP,
        TypeForm::Chan => modifiers::CHAN,
    }
}

fn shape_of_result(shape: TypeShape) -> u64 {
    match shape {
        TypeShape::Form(form) => shape_of_form(form),
        TypeShape::Suppressed | TypeShape::Unknown => 0,
    }
}

fn shape_of_handle<'r, 'a>(
    scope: &mut SemaScope<'_, 'r, 'a>,
    ty: TypeRef<'r>,
    constant: bool,
) -> u64 {
    shape_of_result(scope.type_shape(ty, constant))
}

fn unresolved_selector_token_kind(source: &str, end: usize) -> SemanticTokenKind {
    let mut index = end.min(source.len());
    loop {
        while index < source.len() && source.as_bytes()[index].is_ascii_whitespace() {
            index += 1;
        }
        if source.get(index..index + 2) == Some("//") {
            index = source[index..]
                .find('\n')
                .map_or(source.len(), |offset| index + offset + 1);
            continue;
        }
        if source.get(index..index + 2) == Some("/*") {
            index = source[index + 2..]
                .find("*/")
                .map_or(source.len(), |offset| index + 2 + offset + 2);
            continue;
        }
        break;
    }
    if source.as_bytes().get(index) == Some(&b'(') {
        SemanticTokenKind::Function
    } else {
        SemanticTokenKind::Variable
    }
}

fn is_unresolved_imported_selector<'r, 'a>(
    scope: &SemaScope<'_, 'r, 'a>,
    workspace: &Workspace,
    file: &Path,
    source: &str,
    name: &TokenName,
) -> bool {
    let (qualifier, offset) = match &name.role {
        TokenRole::Member(Some(qualifier)) => {
            let Some(text) = source.get(qualifier.clone()) else {
                return false;
            };
            (text, qualifier.start)
        }
        TokenRole::ExpressionMember(operand) => {
            let Some(text) = source.get(operand.clone()) else {
                return false;
            };
            if !is_simple_identifier(text) {
                return false;
            }
            (text, operand.start)
        }
        _ => return false,
    };
    !scope.is_local_defined_at(offset as u32, qualifier)
        && imported_name(workspace, file, qualifier)
}

fn unresolved_imported_literal_type<'r, 'a>(
    scope: &SemaScope<'_, 'r, 'a>,
    layout: &TokenLayout,
    source: &str,
    name: &TokenName,
) -> bool {
    let (text, offset) = match &name.role {
        TokenRole::LiteralKey(ty) => match source.get(ty.clone()) {
            Some(text) => (text.to_owned(), ty.start),
            None => return false,
        },
        TokenRole::LiteralElementKey(id) => {
            match unresolved_literal_element_spelling(layout, source, *id) {
                Some((text, offset)) => (text, offset),
                None => return false,
            }
        }
        _ => return false,
    };
    let text = text.trim_start_matches(['*', '&']).trim_start();
    let Some((qualifier, _)) = text.split_once('.') else {
        return false;
    };
    is_simple_identifier(qualifier)
        && !scope.is_local_defined_at(offset as u32, qualifier)
        && scope.imported_name(qualifier)
}

// Presentation-only fallback for unavailable literal imports. This spelling
// never establishes a field identity or enters reference/rename matching.
fn unresolved_literal_element_spelling(
    layout: &TokenLayout,
    source: &str,
    context: usize,
) -> Option<(String, usize)> {
    let (span, projections) = layout.literal_element_projection(context)?;
    let mut ty = source.get(span.clone())?.to_owned();
    for key in projections.into_iter().rev() {
        ty = lexical::container_element(&ty, key)?.to_owned();
    }
    Some((ty, span.start))
}

fn is_simple_identifier(text: &str) -> bool {
    let mut bytes = text.bytes();
    bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}
