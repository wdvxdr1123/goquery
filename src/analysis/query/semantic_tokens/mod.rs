//! Semantic token classification and type-shape modifiers backed by `Semantics`.
mod comments;
mod format;
mod lexical;

use crate::analysis::check::decls::DeclCache;
use crate::analysis::check::region::RegionInfo;
use crate::analysis::obj::{DeclId, MethodOrigin, ObjectRef, SymbolOrigin, VarKind};
use crate::analysis::query::Context;
use crate::analysis::scope::universe::{builtin, builtin_function, builtin_type, imported_name};
use crate::analysis::scope::{Binding, DeclarationKind, Locals, ObjectKind};
use crate::analysis::source::{Name, Role, SourceAnalysis};
use crate::analysis::types::{TypeArena, TypeForm, TypeId};
use crate::index::IndexedSymbol;
use crate::syntax::{Lexer, TokenKind};
use crate::workspace::Workspace;
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
        &self,
        window: Option<Range<usize>>,
        include_shapes: bool,
    ) -> Result<Vec<SemanticToken>, (i32, String)> {
        let workspace = self.workspace();
        let file = self.file();
        let source = self.source();
        let analysis = self.semantic_analysis()?;
        let builtin_buf = workspace.builtin_file();
        let builtin_file = builtin_buf.as_deref();

        let window_bounds = window.as_ref().map(|w| (w.start, w.end));
        let in_window = move |span: &Range<usize>| {
            window_bounds.is_none_or(|(start, end)| span.start < end && span.end > start)
        };

        let mut exact_symbols: HashMap<[u32; 2], (&'a Path, &'a IndexedSymbol)> = HashMap::new();
        if let Some((path, stub)) = workspace.files.get_key_value(file) {
            for symbol in &stub.symbols {
                exact_symbols.insert(
                    [symbol.start as u32, symbol.end as u32],
                    (path.as_path(), symbol),
                );
            }
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

        let mut exact_bindings: HashMap<[u32; 2], &Binding> = HashMap::new();
        for binding in analysis.locals.bindings() {
            if binding.name == "_" {
                continue;
            }
            if let (Ok(start), Ok(end)) = (
                u32::try_from(binding.selection.start),
                u32::try_from(binding.selection.end),
            ) {
                exact_bindings.entry([start, end]).or_insert(binding);
            }
        }

        let mut raw_lexical = lexical::tokens(source)
            .filter(|(span, _)| in_window(span))
            .peekable();

        let names_in_window: Vec<&Name> = analysis
            .names
            .iter()
            .filter(|name| in_window(&name.span))
            .collect();

        let mut tokens = Vec::new();
        self.with_sema(|sema| {
            let state = ClassifyEnv {
                workspace,
                file,
                source,
                analysis: &analysis,
                builtin_file,
                exact_symbols: &exact_symbols,
                outer_package: &outer_package,
                exact_bindings: &exact_bindings,
                include_shapes,
            };

            let has_potential_format = source.contains('%') || source.contains('\\');
            let mut formatting: HashMap<Range<usize>, Vec<Range<usize>>> = HashMap::new();
            let mut printf_candidates = 0usize;
            let mut name_idx = 0usize;
            sema.stream_file_regions(file, source, window, |region, arena, decls| {
                while let Some(&name) = names_in_window.get(name_idx) {
                    if name.span.start >= region.span[0] as usize {
                        break;
                    }
                    push_window_name(
                        &state,
                        &formatting,
                        window_bounds,
                        &mut raw_lexical,
                        &mut tokens,
                        name,
                        None,
                        arena,
                        decls,
                    );
                    name_idx += 1;
                }
                if has_potential_format {
                    crate::analysis::query::highlight::collect_region_format_directives(
                        file,
                        source,
                        region,
                        arena,
                        window_bounds,
                        &mut printf_candidates,
                        &mut formatting,
                    );
                }
                while let Some(&name) = names_in_window.get(name_idx) {
                    if name.span.start >= region.span[1] as usize {
                        break;
                    }
                    push_window_name(
                        &state,
                        &formatting,
                        window_bounds,
                        &mut raw_lexical,
                        &mut tokens,
                        name,
                        Some(region),
                        arena,
                        decls,
                    );
                    name_idx += 1;
                }
            });

            let (arena, decls) = sema.arena_and_decls();
            while let Some(&name) = names_in_window.get(name_idx) {
                push_window_name(
                    &state,
                    &formatting,
                    window_bounds,
                    &mut raw_lexical,
                    &mut tokens,
                    name,
                    None,
                    arena,
                    decls,
                );
                name_idx += 1;
            }

            for (span, kind) in raw_lexical {
                emit_raw_lexical_token(&state, &formatting, window_bounds, span, kind, &mut tokens);
            }
        });

        Ok(tokens)
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_raw_lexical_token(
    state: &ClassifyEnv<'_, '_>,
    formatting: &HashMap<Range<usize>, Vec<Range<usize>>>,
    window_bounds: Option<(usize, usize)>,
    span: Range<usize>,
    kind: SemanticTokenKind,
    tokens: &mut Vec<SemanticToken>,
) {
    let in_window =
        |s: &Range<usize>| window_bounds.is_none_or(|(start, end)| s.start < end && s.end > start);
    let push_if_valid =
        |s: Range<usize>, k: SemanticTokenKind, m: u64, out: &mut Vec<SemanticToken>| {
            if in_window(&s)
                && (!matches!(&state.source[s.clone()], "=" | ":=")
                    || !state.analysis.declaration_operator(s.start))
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
            for (s, k, m) in comments::split(state.workspace, state.file, state.source, &span) {
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
fn push_window_name<'a, 'q>(
    state: &ClassifyEnv<'a, 'q>,
    formatting: &HashMap<Range<usize>, Vec<Range<usize>>>,
    window_bounds: Option<(usize, usize)>,
    lexical: &mut std::iter::Peekable<impl Iterator<Item = (Range<usize>, SemanticTokenKind)>>,
    tokens: &mut Vec<SemanticToken>,
    name: &Name,
    region: Option<&RegionInfo<'a>>,
    arena: &mut TypeArena<'a>,
    decls: &mut DeclCache<'a>,
) {
    while lexical
        .peek()
        .is_some_and(|(span, _)| span.start < name.span.start)
    {
        let (span, kind) = lexical.next().expect("peeked lexical token");
        emit_raw_lexical_token(state, formatting, window_bounds, span, kind, tokens);
    }

    if matches!(name.role, Role::Ignored)
        || matches!(name.role, Role::Namespace) && &state.source[name.span.clone()] == "_"
    {
        return;
    }

    if matches!(name.role, Role::Namespace) {
        tokens.push(SemanticToken {
            span: name.span.clone(),
            kind: SemanticTokenKind::Namespace,
            modifiers: 0,
        });
        return;
    }

    let (kind, mods) = state.classify_name(region, arena, decls, name);
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
    analysis: &'q SourceAnalysis,
    builtin_file: Option<&'q Path>,
    exact_symbols: &'q HashMap<[u32; 2], (&'a Path, &'a IndexedSymbol)>,
    outer_package: &'q HashMap<&'a str, Option<(&'a PathBuf, &'a IndexedSymbol)>>,
    exact_bindings: &'q HashMap<[u32; 2], &'q Binding>,
    include_shapes: bool,
}

impl<'a, 'q> ClassifyEnv<'a, 'q> {
    fn has_outer_name(&self, name: &str) -> bool {
        self.outer_package.get(name).copied().flatten().is_some()
            || imported_name(self.workspace, self.file, name)
    }

    fn classify_name(
        &self,
        region: Option<&RegionInfo<'a>>,
        arena: &mut TypeArena<'a>,
        decls: &mut DeclCache<'a>,
        name: &Name,
    ) -> (SemanticTokenKind, u64) {
        arena.reset_steps();
        let span_u32 = [name.span.start as u32, name.span.end as u32];
        let spelling = &self.source[name.span.clone()];

        let mut is_def = false;
        let mut obj = None;
        let mut local_ty: Option<TypeId> = None;
        let mut expr_ty: Option<TypeId> = None;
        let mut sel_ty: Option<TypeId> = None;

        if let Some(region) = region {
            if let Some(found) = region.def_by_span(span_u32) {
                is_def = true;
                obj = Some(found);
            } else if let Some(found) = region.use_by_span(span_u32) {
                obj = Some(found);
            }
            if let Some(
                ObjectRef::Var {
                    origin: SymbolOrigin::Local { id, .. },
                    ..
                }
                | ObjectRef::Const(SymbolOrigin::Local { id, .. })
                | ObjectRef::TypeName {
                    origin: SymbolOrigin::Local { id, .. },
                    ..
                },
            ) = obj
            {
                local_ty = region.local_type(id).map(|op| op.ty);
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
            obj = if sym.is_const() {
                Some(ObjectRef::Const(SymbolOrigin::Package(decl)))
            } else if sym.is_var() {
                Some(ObjectRef::Var {
                    origin: SymbolOrigin::Package(decl),
                    kind: VarKind::Package,
                })
            } else if sym.is_type() {
                Some(ObjectRef::TypeName {
                    origin: SymbolOrigin::Package(decl),
                    is_alias: sym.alias,
                })
            } else if sym.is_function() {
                Some(ObjectRef::Func(decl))
            } else if sym.is_method() {
                Some(ObjectRef::Method(MethodOrigin::Concrete(decl)))
            } else {
                None
            };
        }

        if let Role::EmbeddedType(qualifier) = &name.role {
            let shape = if !self.include_shapes {
                0
            } else if let Some(ObjectRef::TypeName { origin, .. }) = obj {
                match origin {
                    SymbolOrigin::Package(decl) => self.decl_type_shape(arena, decls, decl, false),
                    SymbolOrigin::Local { .. } => local_ty
                        .or(expr_ty)
                        .map_or(0, |ty| shape_of_type(arena, ty, false)),
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
                .and_then(|(decl_file, sym)| {
                    crate::analysis::types::type_form_in_scope(
                        self.workspace,
                        decl_file,
                        &sym.name,
                        &Locals::empty(),
                        0,
                    )
                })
                .map_or(0, shape_of_form)
            } else {
                0
            };
            return (SemanticTokenKind::Property, modifiers::DEFINITION | shape);
        }

        if let Some(found) = obj {
            return self.classify_object(
                arena, decls, name, span_u32, spelling, is_def, found, local_ty, expr_ty, sel_ty,
            );
        }

        self.classify_fallback(arena, decls, name, spelling, expr_ty)
    }

    #[allow(clippy::too_many_arguments)]
    fn classify_object(
        &self,
        arena: &mut TypeArena<'a>,
        decls: &mut DeclCache<'a>,
        name: &Name,
        span_u32: [u32; 2],
        spelling: &str,
        is_def: bool,
        obj: ObjectRef<'a>,
        local_ty: Option<TypeId>,
        expr_ty: Option<TypeId>,
        sel_ty: Option<TypeId>,
    ) -> (SemanticTokenKind, u64) {
        let mut mods = if matches!(name.role, Role::FieldDeclaration(_)) {
            modifiers::DEFINITION
        } else {
            0
        };

        match obj {
            ObjectRef::PkgName { .. } => (SemanticTokenKind::Namespace, 0),
            ObjectRef::BuiltinFunction(_) => {
                (SemanticTokenKind::Function, modifiers::DEFAULT_LIBRARY)
            }
            ObjectRef::BuiltinType(b) => {
                let shape = if self.include_shapes {
                    TypeForm::builtin(b).map_or(0, shape_of_form)
                } else {
                    0
                };
                (SemanticTokenKind::Type, modifiers::DEFAULT_LIBRARY | shape)
            }
            ObjectRef::BuiltinConst(b) => {
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
            ObjectRef::Nil => (
                SemanticTokenKind::Variable,
                modifiers::DEFAULT_LIBRARY | modifiers::READONLY,
            ),
            ObjectRef::Label { span: def_span, .. } => {
                if is_def || def_span == span_u32 {
                    mods |= modifiers::DEFINITION;
                }
                (SemanticTokenKind::Variable, mods)
            }
            ObjectRef::TypeParam { span: def_span, .. } => {
                if is_def || def_span == span_u32 {
                    mods |= modifiers::DEFINITION;
                    if self.local_shadows(def_span, spelling) {
                        mods |= modifiers::SHADOWING;
                    }
                }
                (SemanticTokenKind::TypeParameter, mods)
            }
            ObjectRef::TypeName { origin, .. } => {
                let shape = match origin {
                    SymbolOrigin::Package(decl) => {
                        mods |= self.package_decl_modifiers(decl, name, spelling);
                        if self.include_shapes {
                            self.decl_type_shape(arena, decls, decl, false)
                        } else {
                            0
                        }
                    }
                    SymbolOrigin::Local { span: def_span, .. } => {
                        mods |= self.local_def_modifiers(is_def, def_span, span_u32, spelling);
                        if self.include_shapes {
                            self.local_type_shape(
                                arena,
                                def_span,
                                spelling,
                                local_ty.or(expr_ty),
                                false,
                                false,
                            )
                        } else {
                            0
                        }
                    }
                };
                (SemanticTokenKind::Type, mods | shape)
            }
            ObjectRef::Const(origin) => {
                mods |= modifiers::READONLY;
                let shape = match origin {
                    SymbolOrigin::Package(decl) => {
                        mods |= self.package_decl_modifiers(decl, name, spelling);
                        if self.include_shapes {
                            self.decl_type_shape(arena, decls, decl, true)
                        } else {
                            0
                        }
                    }
                    SymbolOrigin::Local { span: def_span, .. } => {
                        mods |= self.local_def_modifiers(is_def, def_span, span_u32, spelling);
                        if self.include_shapes {
                            self.local_type_shape(
                                arena,
                                def_span,
                                spelling,
                                local_ty.or(expr_ty),
                                true,
                                false,
                            )
                        } else {
                            0
                        }
                    }
                };
                (SemanticTokenKind::Variable, mods | shape)
            }
            ObjectRef::Var {
                origin,
                kind: var_kind,
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
                            self.decl_type_shape(arena, decls, decl, false)
                        } else {
                            0
                        }
                    }
                    SymbolOrigin::Local { span: def_span, .. } => {
                        mods |= self.local_def_modifiers(is_def, def_span, span_u32, spelling);
                        if let Some(binding) = self
                            .exact_bindings
                            .get(&def_span)
                            .copied()
                            .filter(|b| b.name == spelling)
                            && self.binding_has_func_syntax(binding)
                        {
                            mods |= modifiers::SIGNATURE;
                        }
                        if !self.include_shapes {
                            0
                        } else {
                            self.local_type_shape(
                                arena,
                                def_span,
                                spelling,
                                local_ty.or(expr_ty),
                                false,
                                true,
                            )
                        }
                    }
                };
                (token_kind, mods | shape)
            }
            ObjectRef::Func(decl) => {
                mods |= modifiers::SIGNATURE;
                mods |= self.package_decl_modifiers(decl, name, spelling);
                (SemanticTokenKind::Function, mods)
            }
            ObjectRef::Method(origin) => {
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
                            && matches!(name.role, Role::FieldDeclaration(_))
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
            ObjectRef::Field(member) => {
                if member.file == self.file && matches!(name.role, Role::FieldDeclaration(_)) {
                    mods |= modifiers::DEFINITION;
                }
                if member.builtin || self.builtin_file == Some(member.file) {
                    mods |= modifiers::DEFAULT_LIBRARY;
                }
                let shape = if !self.include_shapes {
                    0
                } else if let Some(ty) = sel_ty.or(expr_ty) {
                    shape_of_type(arena, ty, false)
                } else {
                    member
                        .to_member_declaration(self.workspace)
                        .and_then(|decl| decl.data(self.workspace))
                        .and_then(|data| data.annotation)
                        .and_then(|ann| {
                            crate::analysis::types::type_form_in_scope(
                                self.workspace,
                                member.file,
                                &ann,
                                &Locals::empty(),
                                0,
                            )
                        })
                        .map_or(0, shape_of_form)
                };
                (SemanticTokenKind::Property, mods | shape)
            }
        }
    }

    fn classify_fallback(
        &self,
        arena: &mut TypeArena<'a>,
        decls: &mut DeclCache<'a>,
        name: &Name,
        spelling: &str,
        expr_ty: Option<TypeId>,
    ) -> (SemanticTokenKind, u64) {
        if let Some(binding) = self
            .analysis
            .locals
            .definition_at(&name.span, spelling)
            .filter(|_| {
                matches!(
                    name.role,
                    Role::Name
                        | Role::Type(None)
                        | Role::LiteralKey(_)
                        | Role::LiteralElementKey(_)
                )
            })
        {
            let kind = from_object_kind(binding.object_kind);
            let mut mods = 0u64;
            if binding.selection == name.span {
                mods |= modifiers::DEFINITION;
                if self.analysis.locals.has_outer_binding(binding)
                    || self.has_outer_name(spelling)
                    || builtin(spelling)
                {
                    mods |= modifiers::SHADOWING;
                }
            }
            match binding.declaration {
                DeclarationKind::Constant => mods |= modifiers::READONLY,
                DeclarationKind::TypeParameter => return (SemanticTokenKind::TypeParameter, mods),
                _ => {}
            }
            if self.binding_has_func_syntax(binding) {
                mods |= modifiers::SIGNATURE;
            }
            let shape = if !self.include_shapes
                || (binding.selection == name.span
                    && self.analysis.locals.switch_case(binding).is_some())
            {
                0
            } else if let Some(ty) = expr_ty {
                shape_of_type(arena, ty, binding.declaration == DeclarationKind::Constant)
            } else {
                self.binding_fallback_shape(binding)
            };
            return (kind, mods | shape);
        }

        match &name.role {
            Role::BlankDeclaration { kind, constant } => (
                from_object_kind(*kind),
                modifiers::DEFINITION | if *constant { modifiers::READONLY } else { 0 },
            ),
            Role::FieldDeclaration(owner_idx) => {
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
                        expr_ty.map_or(0, |ty| shape_of_type(arena, ty, false))
                    } else {
                        0
                    };
                    (SemanticTokenKind::Property, modifiers::DEFINITION | shape)
                }
            }
            Role::Parameter | Role::Result => {
                let kind = if matches!(name.role, Role::Parameter) {
                    SemanticTokenKind::Parameter
                } else {
                    SemanticTokenKind::Variable
                };
                let shape = if !self.include_shapes {
                    0
                } else if let Some(ty) = expr_ty {
                    shape_of_type(arena, ty, false)
                } else if let Some((span, variadic)) = self.analysis.signature_type(name.span.start)
                {
                    if variadic {
                        modifiers::SLICE
                    } else {
                        self.source
                            .get(span.clone())
                            .and_then(|text| {
                                crate::analysis::types::type_form_in_scope(
                                    self.workspace,
                                    self.file,
                                    text,
                                    &self.analysis.locals,
                                    span.start,
                                )
                            })
                            .map_or(0, shape_of_form)
                    }
                } else {
                    0
                };
                (kind, modifiers::DEFINITION | shape)
            }
            Role::Type(Some(qualifier)) => {
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
            Role::Type(None) => {
                if spelling == "_" && self.analysis.locals.range_blank_at(&name.span).is_some() {
                    let shape = if self.include_shapes {
                        expr_ty.map_or(0, |ty| shape_of_type(arena, ty, false))
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
            Role::Name => {
                if spelling == "_" && self.analysis.locals.range_blank_at(&name.span).is_some() {
                    let shape = if self.include_shapes {
                        expr_ty.map_or(0, |ty| shape_of_type(arena, ty, false))
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
                    let obj = if sym.is_const() {
                        ObjectRef::Const(SymbolOrigin::Package(decl))
                    } else if sym.is_var() {
                        ObjectRef::Var {
                            origin: SymbolOrigin::Package(decl),
                            kind: VarKind::Package,
                        }
                    } else if sym.is_type() {
                        ObjectRef::TypeName {
                            origin: SymbolOrigin::Package(decl),
                            is_alias: sym.alias,
                        }
                    } else if sym.is_function() {
                        ObjectRef::Func(decl)
                    } else {
                        ObjectRef::Method(MethodOrigin::Concrete(decl))
                    };
                    return self.classify_object(
                        arena,
                        decls,
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
            Role::Member(_) | Role::ExpressionMember(_) => {
                let mut kind = if is_unresolved_imported_selector(
                    self.analysis,
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
                        Role::Member(Some(span)) | Role::ExpressionMember(span) => {
                            Some(span.clone())
                        }
                        _ => None,
                    }
                    && let Some(receiver) = self.source.get(operand.clone())
                    && let Some(binding) = self.analysis.locals.definition_at(&operand, receiver)
                    && (binding
                        .annotation()
                        .and_then(|span| self.source.get(span.clone()))
                        .is_some_and(|text| text == "error")
                        || crate::analysis::query::Context::borrowed(
                            self.workspace,
                            self.file,
                            self.source,
                        )
                        .binding_type(&self.analysis.locals, binding)
                        .as_deref()
                        .is_some_and(|text| text == "error"))
                {
                    kind = SemanticTokenKind::Method;
                    mods |= modifiers::DEFAULT_LIBRARY | modifiers::SIGNATURE;
                }
                (kind, mods)
            }
            Role::LiteralKey(_) | Role::LiteralElementKey(_) => {
                if unresolved_imported_literal_type(
                    self.analysis,
                    self.workspace,
                    self.file,
                    self.source,
                    name,
                ) {
                    (SemanticTokenKind::Variable, 0)
                } else {
                    (SemanticTokenKind::Property, 0)
                }
            }
            Role::Field => (SemanticTokenKind::Property, 0),
            Role::Ignored | Role::Namespace | Role::EmbeddedType(_) => {
                (SemanticTokenKind::Variable, 0)
            }
        }
    }

    fn package_decl_modifiers(&self, decl: DeclId<'a>, name: &Name, spelling: &str) -> u64 {
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

    fn local_def_modifiers(
        &self,
        is_def: bool,
        def_span: [u32; 2],
        span_u32: [u32; 2],
        spelling: &str,
    ) -> u64 {
        let mut mods = 0u64;
        if is_def || def_span == span_u32 {
            mods |= modifiers::DEFINITION;
            if self.local_shadows(def_span, spelling) {
                mods |= modifiers::SHADOWING;
            }
        }
        mods
    }

    fn local_shadows(&self, def_span: [u32; 2], spelling: &str) -> bool {
        let has_outer_local = self
            .exact_bindings
            .get(&def_span)
            .copied()
            .filter(|b| b.name == spelling)
            .is_some_and(|b| self.analysis.locals.has_outer_binding(b));
        has_outer_local || self.has_outer_name(spelling) || builtin(spelling)
    }

    fn binding_has_func_syntax(&self, binding: &Binding) -> bool {
        if binding.declaration == DeclarationKind::TypeParameter {
            return false;
        }
        let annotation = binding.annotation().cloned().or_else(|| {
            let (span, result, count) = binding.initializer()?;
            (result == 0 && count == 1).then(|| span.clone())
        });
        annotation
            .and_then(|span| self.source.get(span))
            .and_then(|text| Lexer::new(text).next())
            .is_some_and(|token| token.kind == TokenKind::Func)
    }

    fn binding_fallback_shape(&self, binding: &Binding) -> u64 {
        if binding.declaration == DeclarationKind::Variadic {
            return modifiers::SLICE;
        }
        if let Some(span) = binding.annotation()
            && let Some(text) = self.source.get(span.clone())
            && let Some(form) = crate::analysis::types::type_form_in_scope(
                self.workspace,
                self.file,
                text,
                &self.analysis.locals,
                span.start,
            )
        {
            return shape_of_form(form);
        }
        let Some(rendered) =
            crate::analysis::query::Context::borrowed(self.workspace, self.file, self.source)
                .binding_type(&self.analysis.locals, binding)
        else {
            return 0;
        };
        let Some(form) = crate::analysis::types::type_form_in_scope(
            self.workspace,
            self.file,
            &rendered,
            &self.analysis.locals,
            binding.selection.start,
        ) else {
            return 0;
        };
        let mut bits = shape_of_form(form);
        if binding.declaration == DeclarationKind::Constant
            && !rendered.contains("bool")
            && !rendered.contains("string")
        {
            // Keep number shape etc.; untyped string/bool constants don't have bool/string form in gopls
            // when inferred only via legacy string rendering without concrete type.
        }
        if binding.declaration == DeclarationKind::Constant
            && form == TypeForm::Bool
            && rendered == "untyped bool"
        {
            bits &= !modifiers::BOOL;
        }
        bits
    }

    fn local_type_shape(
        &self,
        arena: &mut TypeArena<'a>,
        def_span: [u32; 2],
        spelling: &str,
        ty: Option<TypeId>,
        constant: bool,
        fallback_on_none: bool,
    ) -> u64 {
        if let Some(ty) = ty {
            let shape = shape_of_type(arena, ty, constant);
            if shape != 0 {
                return shape;
            }
            if (constant && arena.is_untyped_bool_or_string(ty)) || arena.is_type_param(ty) {
                return 0;
            }
        } else if !fallback_on_none {
            return 0;
        }
        self.exact_bindings
            .get(&def_span)
            .copied()
            .filter(|b| b.name == spelling)
            .map_or(0, |binding| self.binding_fallback_shape(binding))
    }

    fn decl_type_shape(
        &self,
        arena: &mut TypeArena<'a>,
        decls: &mut DeclCache<'a>,
        decl: DeclId<'a>,
        constant: bool,
    ) -> u64 {
        if let Some(ty) = decls.type_of(arena, decl) {
            let shape = shape_of_type(arena, ty, constant);
            if shape != 0 {
                return shape;
            }
            if (constant && arena.is_untyped_bool_or_string(ty)) || arena.is_type_param(ty) {
                return 0;
            }
        }
        if decl.symbol.is_type() {
            return crate::analysis::types::type_form_in_scope(
                self.workspace,
                decl.file,
                &decl.symbol.name,
                &Locals::empty(),
                0,
            )
            .map_or(0, shape_of_form);
        }
        if (decl.symbol.is_var() || decl.symbol.is_const())
            && let Some(decl_range) = decl.symbol.declaration()
        {
            let spec_src = if decl.file == self.file {
                self.source
                    .get(decl_range.clone())
                    .map(std::borrow::Cow::Borrowed)
            } else {
                self.workspace
                    .source_slice(decl.file, decl_range.clone())
                    .map(std::borrow::Cow::Owned)
            };
            if let Some(spec_src) = spec_src {
                let kind = if decl.symbol.is_const() {
                    crate::syntax::DeclSpecKind::Const
                } else {
                    crate::syntax::DeclSpecKind::Var
                };
                let tree = crate::syntax::parse_decl_spec(&spec_src, kind);
                if let Some(crate::syntax::StmtKind::Declaration {
                    annotation: Some(ann),
                    ..
                }) = tree.statement(tree.root()).map(|stmt| &stmt.kind)
                    && let Some(ann_text) = spec_src.get(ann.clone())
                {
                    return crate::analysis::types::type_form_in_scope(
                        self.workspace,
                        decl.file,
                        ann_text.trim(),
                        &Locals::empty(),
                        decl_range.start + ann.start,
                    )
                    .map_or(0, shape_of_form);
                }
            }
        }
        0
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

fn shape_of_type(arena: &mut TypeArena<'_>, ty: TypeId, constant: bool) -> u64 {
    if let Some(root) = arena.underlying(ty)
        && arena.basic_kind(root) == Some(crate::analysis::types::BasicKind::UnsafePointer)
    {
        return modifiers::POINTER;
    }
    let Some(form) = arena.form(ty) else {
        return 0;
    };
    let mut bits = shape_of_form(form);
    if constant && arena.is_untyped_bool_or_string(ty) {
        bits &= !(modifiers::BOOL | modifiers::STRING);
    }
    bits
}

fn from_object_kind(kind: ObjectKind) -> SemanticTokenKind {
    match kind {
        ObjectKind::Type => SemanticTokenKind::Type,
        ObjectKind::Variable => SemanticTokenKind::Variable,
        ObjectKind::Parameter => SemanticTokenKind::Parameter,
        ObjectKind::TypeParameter => SemanticTokenKind::TypeParameter,
    }
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

fn is_unresolved_imported_selector(
    analysis: &SourceAnalysis,
    workspace: &Workspace,
    file: &Path,
    source: &str,
    name: &Name,
) -> bool {
    let (qualifier, offset) = match &name.role {
        Role::Member(Some(qualifier)) => {
            let Some(text) = source.get(qualifier.clone()) else {
                return false;
            };
            (text, qualifier.start)
        }
        Role::ExpressionMember(operand) => {
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
    analysis.locals.resolve_at(offset, qualifier).is_none()
        && imported_name(workspace, file, qualifier)
}

fn unresolved_imported_literal_type(
    analysis: &SourceAnalysis,
    workspace: &Workspace,
    file: &Path,
    source: &str,
    name: &Name,
) -> bool {
    let (text, offset) = match &name.role {
        Role::LiteralKey(ty) => match source.get(ty.clone()) {
            Some(text) => (text.to_owned(), ty.start),
            None => return false,
        },
        Role::LiteralElementKey(id) => {
            match analysis.literal_element_type(workspace, file, source, *id) {
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
        && analysis.locals.resolve_at(offset, qualifier).is_none()
        && imported_name(workspace, file, qualifier)
}

fn is_simple_identifier(text: &str) -> bool {
    let mut bytes = text.bytes();
    bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}
