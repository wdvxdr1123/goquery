//! Read-only semantic operations over the admitted request source and metadata.
use super::locations::NavigationLocation;
use super::{Context, Location, SourceRange};
use crate::analysis::sema::{
    CursorBody, LocalDeclKind, Object, RegionMode, SymbolOrigin, TypeDeclaration,
};
use crate::syntax::{Lexer, TokenKind};
use std::borrow::Cow;

impl<'a> Context<'a> {
    pub(in crate::analysis::query) fn cursor_body(
        &mut self,
        offset: usize,
    ) -> Result<Option<CursorBody>, (i32, String)> {
        let (doc, sema) = self.split();
        let function = super::cursor_function(doc.workspace(), doc.file(), doc.source(), offset);
        sema.cursor_body(doc.file(), doc.source(), offset, function)
    }

    pub(crate) fn type_definition_locations(
        &mut self,
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

    fn type_declarations(
        &mut self,
        offset: usize,
    ) -> Result<Vec<TypeDeclaration<'a>>, (i32, String)> {
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
        let mode = RegionMode::from_cursor((offset == source.len()).then_some(offset));
        let (doc, sema) = self.split();
        let workspace = doc.workspace();
        let source = doc.source();
        let Some(mut view) = sema.region_at(doc.file(), source, off, mode) else {
            return Ok(Vec::new());
        };
        if let Some((span, obj)) = view.object_at(off) {
            return match obj {
                Object::PkgName { .. }
                | Object::Label { .. }
                | Object::Nil
                | Object::BuiltinFunction(_) => Ok(Vec::new()),
                Object::TypeParam { span: tp_span, .. } => {
                    match source.get(tp_span[0] as usize..tp_span[1] as usize) {
                        Some(name) => {
                            Err((0, format!("cannot find type name(s) from type {name}")))
                        }
                        None => Ok(Vec::new()),
                    }
                }
                Object::TypeName {
                    origin:
                        SymbolOrigin::Local {
                            file: f, span: s, ..
                        },
                    ..
                } => Ok(match workspace.files.get_key_value(f) {
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
                } => Ok(match workspace.files.get_key_value(decl.file) {
                    Some((file_buf, _)) => vec![TypeDeclaration::Named(file_buf, decl.symbol)],
                    None => Vec::new(),
                }),
                _ => {
                    let Some(ty) = view.type_of_object(obj, span) else {
                        return Ok(Vec::new());
                    };
                    view.types()
                        .projected_type_declarations(ty)
                        .unwrap_or_else(|| Ok(Vec::new()))
                }
            };
        }
        if let Some((ident_span, false)) = selected {
            let key = [ident_span.start as u32, ident_span.end as u32];
            let guard_ty = view
                .local_binding_at_span(key)
                .filter(|binding| binding.is_def && binding.kind == LocalDeclKind::TypeSwitchVar)
                .and_then(|binding| view.local_init_span(binding.id))
                .and_then(|guard_span| view.expr_type(guard_span))
                .map(|tv| tv.ty);
            return guard_ty
                .and_then(|ty| view.types().projected_type_declarations(ty))
                .unwrap_or_else(|| Ok(Vec::new()));
        }
        if selected.is_none() {
            let expr_cand = view.smallest_expr_at(off);
            let call_cand = view
                .calls()
                .filter(|call| {
                    !call.is_conversion
                        && call.signature.is_some()
                        && call.span[0] <= off
                        && off <= call.span[1]
                })
                .min_by_key(|call| call.span[1].saturating_sub(call.span[0]));
            let target_ty = match (expr_cand, call_cand) {
                (Some((es, tv)), Some(call)) => {
                    let call_len = call.span[1].saturating_sub(call.span[0]);
                    let expr_len = es[1].saturating_sub(es[0]);
                    if call_len < expr_len {
                        call.signature
                    } else {
                        Some(tv.ty)
                    }
                }
                (Some((_, tv)), None) => Some(tv.ty),
                (None, Some(call)) => call.signature,
                (None, None) => None,
            };
            return target_ty
                .and_then(|ty| view.types().projected_type_declarations(ty))
                .unwrap_or_else(|| Ok(Vec::new()));
        }
        Ok(Vec::new())
    }
}
