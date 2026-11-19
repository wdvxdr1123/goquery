//! Read-only semantic operations over the admitted request source and metadata.
use super::locations::NavigationLocation;
use super::{Context, Location, SourceRange};
use crate::analysis::budget::MAX_BYTES;
use crate::analysis::obj::{DeclId, LocalId, MethodOrigin, ObjectRef, SymbolOrigin};
use crate::analysis::scope::occurrences::{Name, Occurrences, Role};
use crate::analysis::scope::tree::LocalBindingKind;
use crate::analysis::scope::{Binding, CursorScope, Locals};
use crate::analysis::types::{Qualifier, TypeDeclaration};
use crate::syntax::{self, Lexer, TokenKind};
use std::borrow::Cow;

impl<'a> Context<'a> {
    pub(in crate::analysis::query) fn cursor_scope(
        &self,
        offset: usize,
    ) -> Result<CursorScope, (i32, String)> {
        self.cursor_scope_with_hint(offset, None)
    }

    pub(in crate::analysis::query) fn cursor_scope_with_hint(
        &self,
        offset: usize,
        cursor: Option<usize>,
    ) -> Result<CursorScope, (i32, String)> {
        let function =
            super::cursor_function(self.workspace, self.view.file, self.source(), offset);
        self.with_sema(|sema| {
            sema.cursor_scope(self.file(), self.source(), offset, function, cursor)
        })
    }

    pub(in crate::analysis::query) fn type_position(
        &self,
        offset: usize,
        scope: &CursorScope,
    ) -> bool {
        super::completion::type_position(
            self.workspace,
            self.view.file,
            self.source(),
            offset,
            scope,
        )
    }

    pub(crate) fn binding_type(&self, _locals: &Locals, binding: &Binding) -> Option<String> {
        self.binding_type_in(self.file(), self.source(), binding)
    }

    pub(in crate::analysis::query) fn binding_presentation(
        &self,
        binding: &Binding,
    ) -> Option<String> {
        self.binding_type_formatted(
            self.file(),
            self.source(),
            binding,
            Qualifier::Presentation(self.file()),
        )
    }

    pub(crate) fn type_definition_locations(
        &self,
        offset: usize,
    ) -> Result<Vec<NavigationLocation<'a>>, (i32, String)> {
        let declarations = self.type_declarations(offset)?;
        let mut current_map = None;
        Ok(declarations
            .into_iter()
            .filter_map(|target| {
                let (file, range) = match target {
                    TypeDeclaration::Named(file, symbol) => (
                        Cow::Borrowed(file),
                        SourceRange::indexed_selection(&symbol.selection),
                    ),
                    TypeDeclaration::Local(file, span) => (
                        Cow::Borrowed(file),
                        self.cross_file_range(file, &span, &mut current_map)?,
                    ),
                    TypeDeclaration::Builtin(name) => {
                        let (file, range) = self.builtin_symbol_location(name, true)?;
                        (Cow::Owned(file), range)
                    }
                };
                Some(Location { file, range })
            })
            .collect())
    }

    fn type_declarations(&self, offset: usize) -> Result<Vec<TypeDeclaration<'a>>, (i32, String)> {
        let file = self.view.file;
        let source = self.source();
        let workspace = self.workspace;
        if let Some(stub) = workspace.files.get(file) {
            for symbol in &stub.symbols {
                let Some(declaration) = symbol.declaration() else {
                    continue;
                };
                if let Some(body) = symbol.body() {
                    if declaration.start <= offset && offset <= body.start {
                        let _ = source.get(declaration.start..body.start).ok_or((
                            crate::error::INVALID_PARAMS,
                            "stale function header".into(),
                        ))?;
                    } else if body.contains(&offset)
                        || (offset == source.len() && body.end == offset)
                    {
                        let _ = body.start.checked_sub(declaration.start).ok_or((
                            crate::error::INVALID_PARAMS,
                            "stale function boundary".into(),
                        ))?;
                        let _ = source
                            .get(declaration.start..body.end)
                            .ok_or((crate::error::INVALID_PARAMS, "stale function range".into()))?;
                    }
                }
            }
        }
        let Ok(off) = u32::try_from(offset) else {
            return Ok(Vec::new());
        };
        let mut previous = TokenKind::Illegal;
        let mut selected = None;
        for token in Lexer::new(source).take_while(|token| token.span.start <= offset) {
            if token.kind == TokenKind::Ident
                && token.span.start <= offset
                && offset <= token.span.end
            {
                selected = Some((token.span.clone(), previous == TokenKind::Period));
                break;
            }
            previous = token.kind;
        }
        let cursor_offset = (offset == source.len()).then_some(offset);

        self.with_sema(|sema| -> Result<Vec<TypeDeclaration<'a>>, (i32, String)> {
            enum Action<'a> {
                Done(Result<Vec<TypeDeclaration<'a>>, (i32, String)>),
                TypeOfDecl(DeclId<'a>),
                OperandOfDecl(DeclId<'a>),
            }

            let action = {
                let Some((region, arena)) =
                    sema.region_and_arena_at(file, source, off, cursor_offset)
                else {
                    return Ok(Vec::new());
                };
                if let Some((span, obj)) = region.object_at(off) {
                    match obj {
                        ObjectRef::PkgName { .. }
                        | ObjectRef::Label { .. }
                        | ObjectRef::Nil
                        | ObjectRef::BuiltinFunction(_) => Action::Done(Ok(Vec::new())),
                        ObjectRef::TypeParam { span: tp_span, .. } => {
                            let msg = source
                                .get(tp_span[0] as usize..tp_span[1] as usize)
                                .map_or_else(
                                    || Ok(Vec::new()),
                                    |name| {
                                        Err((
                                            0,
                                            format!("cannot find type name(s) from type {name}"),
                                        ))
                                    },
                                );
                            Action::Done(msg)
                        }
                        ObjectRef::Var {
                            origin: SymbolOrigin::Local { id, .. },
                            ..
                        }
                        | ObjectRef::Const(SymbolOrigin::Local { id, .. }) => Action::Done(
                            region
                                .local_type(id)
                                .and_then(|op| arena.projected_type_declarations(op.ty))
                                .unwrap_or_else(|| Ok(Vec::new())),
                        ),
                        ObjectRef::TypeName {
                            origin:
                                SymbolOrigin::Local {
                                    file: f, span: s, ..
                                },
                            ..
                        } => Action::Done(match workspace.files.get_key_value(f) {
                            Some((file_buf, _)) => Ok(vec![TypeDeclaration::Local(
                                file_buf,
                                s[0] as usize..s[1] as usize,
                            )]),
                            None => Ok(Vec::new()),
                        }),
                        ObjectRef::TypeName {
                            origin: SymbolOrigin::Package(decl),
                            ..
                        } => Action::Done(match workspace.files.get_key_value(decl.file) {
                            Some((file_buf, _)) => {
                                Ok(vec![TypeDeclaration::Named(file_buf, decl.symbol)])
                            }
                            None => Ok(Vec::new()),
                        }),
                        ObjectRef::BuiltinType(name) => Action::Done(
                            arena
                                .builtin_type(file, name)
                                .and_then(|ty| arena.projected_type_declarations(ty))
                                .unwrap_or_else(|| Ok(Vec::new())),
                        ),
                        ObjectRef::BuiltinConst(_) => Action::Done(
                            region
                                .expr_type(span)
                                .and_then(|op| arena.projected_type_declarations(op.ty))
                                .unwrap_or_else(|| Ok(Vec::new())),
                        ),
                        ObjectRef::Field(_) | ObjectRef::Method(_) => {
                            if let Some(sel) = region.selection_by_span(span) {
                                Action::Done(
                                    arena
                                        .projected_type_declarations(sel.ty)
                                        .unwrap_or_else(|| Ok(Vec::new())),
                                )
                            } else if let ObjectRef::Method(MethodOrigin::Concrete(decl)) = obj {
                                Action::TypeOfDecl(decl)
                            } else {
                                Action::Done(Ok(Vec::new()))
                            }
                        }
                        ObjectRef::Func(decl) => {
                            if let Some(inst) = region.instance_by_span(span) {
                                Action::Done(
                                    arena
                                        .projected_type_declarations(inst.instantiated)
                                        .unwrap_or_else(|| Ok(Vec::new())),
                                )
                            } else {
                                Action::TypeOfDecl(decl)
                            }
                        }
                        ObjectRef::Const(SymbolOrigin::Package(decl))
                        | ObjectRef::Var {
                            origin: SymbolOrigin::Package(decl),
                            ..
                        } => Action::OperandOfDecl(decl),
                    }
                } else if let Some((ident_span, false)) = selected.as_ref() {
                    let key = [ident_span.start as u32, ident_span.end as u32];
                    let guard_ty = region
                        .scopes
                        .bindings
                        .iter()
                        .enumerate()
                        .find(|(_, b)| {
                            b.kind == LocalBindingKind::TypeSwitchVar && b.name_span == key
                        })
                        .and_then(|(idx, _)| region.local_init(LocalId(idx as u32)))
                        .and_then(|guard_span| region.expr_type(guard_span));
                    Action::Done(
                        guard_ty
                            .and_then(|op| arena.projected_type_declarations(op.ty))
                            .unwrap_or_else(|| Ok(Vec::new())),
                    )
                } else if selected.is_none() {
                    let expr_cand = region.smallest_expr_at(off);
                    let call_cand = region
                        .calls
                        .iter()
                        .filter(|call| {
                            !call.is_conversion
                                && call.signature.is_some()
                                && call.span[0] <= off
                                && off <= call.span[1]
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
                    Action::Done(
                        target_ty
                            .and_then(|ty| arena.projected_type_declarations(ty))
                            .unwrap_or_else(|| Ok(Vec::new())),
                    )
                } else {
                    Action::Done(Ok(Vec::new()))
                }
            };

            match action {
                Action::Done(result) => result,
                Action::TypeOfDecl(decl) => {
                    let ty = sema.type_of_decl(decl).or_else(|| {
                        if !decl.symbol.is_callable() {
                            return None;
                        }
                        let range = decl.symbol.declaration()?;
                        let end = decl.symbol.body().map_or(range.end, |b| b.start);
                        if end.checked_sub(range.start)? > MAX_BYTES {
                            return None;
                        }
                        let header = if decl.file == file {
                            std::borrow::Cow::Borrowed(source.get(range.start..end)?)
                        } else {
                            std::borrow::Cow::Owned(
                                workspace.source_slice(decl.file, range.start..end)?,
                            )
                        };
                        let parsed = syntax::parse_signature(&header);
                        sema.arena_mut()
                            .declaration_signature(decl.file, &header, &parsed)
                    });
                    ty.and_then(|ty| sema.arena_mut().projected_type_declarations(ty))
                        .unwrap_or_else(|| Ok(Vec::new()))
                }
                Action::OperandOfDecl(decl) => sema
                    .operand_of_decl(decl)
                    .and_then(|op| sema.arena_mut().projected_type_declarations(op.ty))
                    .unwrap_or_else(|| Ok(Vec::new())),
            }
        })
    }

    pub(crate) fn semantic_analysis(&self) -> Result<Occurrences, (i32, String)> {
        let mut analysis =
            Occurrences::for_semantic(self.source(), &self.workspace.files[self.view.file])?;
        // Aggregate members written in anonymous type literals are declarations:
        // semantic tokens report them as property/definition like gopls. The
        // anonymous owner has no package-level symbol, so a sentinel owner is
        // resolved to no identity rather than to an unrelated declaration.
        for name in &mut analysis.names {
            if matches!(name.role, Role::Field) {
                name.role = Role::FieldDeclaration(usize::MAX);
            }
        }
        analysis.names.extend(self.import_names(self.source()));
        analysis.names.sort_unstable_by_key(|name| name.span.start);
        Ok(analysis)
    }

    // Match a default import's virtual declaration against the dependency's
    // actual package name, rather than assuming the import-path suffix.
    fn import_names(&self, source: &str) -> Vec<Name> {
        let mut names = Vec::new();
        let mut importing = false;
        let mut grouped = false;
        let mut explicit = false;
        for token in Lexer::new(source) {
            if token.kind == TokenKind::Import {
                importing = true;
                explicit = false;
                continue;
            }
            if !importing {
                if matches!(
                    token.kind,
                    TokenKind::Func | TokenKind::Type | TokenKind::Var | TokenKind::Const
                ) {
                    break;
                }
                continue;
            }
            match token.kind {
                TokenKind::LParen => grouped = true,
                TokenKind::RParen => {
                    importing = false;
                    grouped = false;
                }
                TokenKind::Ident | TokenKind::Period => explicit = true,
                TokenKind::Semicolon => {
                    explicit = false;
                    if !grouped {
                        importing = false;
                    }
                }
                TokenKind::String => {
                    if !explicit {
                        let literal = &source[token.span.clone()];
                        let package =
                            crate::syntax::decode_import_path(literal).and_then(|mut path| {
                                self.workspace
                                    .sdk_import_identity(self.view.file, &mut path);
                                self.workspace.declared_package_name(&path)
                            });
                        if let Some(package) = package
                            && let Some(offset) = literal.rfind(package)
                        {
                            names.push(Name {
                                span: token.span.start + offset
                                    ..token.span.start + offset + package.len(),
                                role: Role::Namespace,
                            });
                        }
                    }
                    explicit = false;
                    if !grouped {
                        importing = false;
                    }
                }
                _ => {}
            }
        }
        names
    }
}

impl<View> Context<'_, View> {
    pub(in crate::analysis::query) fn binding_type_in(
        &self,
        file: &std::path::Path,
        source: &str,
        binding: &Binding,
    ) -> Option<String> {
        self.binding_type_formatted(file, source, binding, Qualifier::RelativeTo(file))
    }

    fn binding_type_formatted(
        &self,
        file: &std::path::Path,
        source: &str,
        binding: &Binding,
        qualifier: Qualifier<'_>,
    ) -> Option<String> {
        let s0 = u32::try_from(binding.selection.start).ok()?;
        let s1 = u32::try_from(binding.selection.end).ok()?;
        self.with_sema(|sema| {
            let (region, arena) = sema.region_and_arena_at(file, source, s0, None)?;
            let (id, _) = super::signatures::resolve_local_binding_id(
                region,
                [s0, s1],
                binding.scope_start(),
            )?;
            let op = region.local_type(id)?;
            let rendered = arena.format_type(op.ty, qualifier).or_else(|| {
                arena
                    .has_local_declaration(op.ty)
                    .filter(|has| *has)
                    .and_then(|_| arena.format_type(op.ty, Qualifier::Presentation(file)))
            })?;
            super::signatures::label::function_types(&rendered).or(Some(rendered))
        })
    }
}
