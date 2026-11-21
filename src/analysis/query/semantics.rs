//! Read-only semantic operations over the admitted request source and metadata.
use super::locations::NavigationLocation;
use super::{Context, Location, SourceRange};
use crate::analysis::sema::{CursorBody, RegionMode, TypeDeclaration};
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
        let Some(mut view) = sema.region_at(doc.file(), doc.source(), off, mode) else {
            return Ok(Vec::new());
        };
        view.type_declarations_at(off, selected)
    }
}
