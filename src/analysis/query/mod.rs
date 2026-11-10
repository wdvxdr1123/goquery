//! A request-local immutable metadata/source view, with a lazy native type arena.
pub(crate) mod budget;
mod calls;
pub(crate) mod completion;
pub(crate) mod diagnostics;
pub(crate) mod folding;
pub(crate) mod hierarchy;
pub(crate) mod highlight;
pub(crate) mod hover;
pub(crate) mod implementation;
pub(crate) mod links;
mod locations;
mod navigation;
mod references;
pub(crate) mod rename;
mod semantics;
mod signatures;
pub(crate) mod symbols;
pub(crate) use locations::{SourceLocation, SourceRange};
pub(crate) use signatures::SignatureTrigger;
mod source;
use crate::analysis::scope::{CursorScope, Locals};
use crate::analysis::types::TypeArena;
use crate::workspace::Workspace;
pub(crate) use navigation::ReferenceTarget;
use source::SourceView;
use std::borrow::Cow;
use std::ops::Range;
use std::path::Path;

pub(crate) struct Document<'a> {
    file: &'a Path,
    source: SourceView<'a>,
}

pub(crate) struct Context<'a, View = Document<'a>> {
    workspace: &'a Workspace,
    view: View,
    arena: Option<TypeArena<'a>>,
}
impl<'a, View> Context<'a, View> {
    pub(crate) fn workspace(&self) -> &'a Workspace {
        self.workspace
    }
}
impl<'a> Context<'a, ()> {
    pub(crate) fn metadata(workspace: &'a Workspace) -> Self {
        Self {
            workspace,
            view: (),
            arena: None,
        }
    }
}
impl<'a> Context<'a> {
    pub(crate) fn new(workspace: &'a mut Workspace, file: &Path) -> Result<Self, (i32, String)> {
        let closed = workspace.prepare_document(file)?;
        let workspace: &'a Workspace = workspace;
        // An open buffer can be admitted for source-only answering without ever
        // entering the package index (for example a cgo file in a cgo-disabled
        // build context). Semantic fields stay unavailable for it.
        let Some(file) = workspace
            .files
            .get_key_value(file)
            .map(|(path, _)| path.as_path())
        else {
            return Err((
                crate::error::INVALID_PARAMS,
                "no package metadata for file".into(),
            ));
        };
        Ok(Self {
            workspace,
            view: Document {
                file,
                source: SourceView::admitted(workspace, file, closed),
            },
            arena: None,
        })
    }
    pub(crate) fn borrowed(workspace: &'a Workspace, file: &'a Path, source: &'a str) -> Self {
        Self {
            workspace,
            view: Document {
                file,
                source: SourceView::borrowed(source),
            },
            arena: None,
        }
    }

    /// Source-only view for syntactic features on open documents that the build
    /// or discovery scope excludes. Semantic fields stay unavailable: callers
    /// must not touch package metadata through this view.
    pub(crate) fn source_only(
        workspace: &'a Workspace,
        file: &'a Path,
    ) -> Result<Self, (i32, String)> {
        let view = match workspace.open_source(file) {
            Some(source) => Document {
                file,
                source: SourceView::borrowed(source),
            },
            None => {
                let source = workspace.source(file).ok_or((
                    crate::error::INVALID_PARAMS,
                    "file unavailable for source-only queries".into(),
                ))?;
                Document {
                    file,
                    source: SourceView::admitted(workspace, file, Some(source)),
                }
            }
        };
        Ok(Self {
            workspace,
            view,
            arena: None,
        })
    }
    pub(crate) fn file(&self) -> &'a Path {
        self.view.file
    }
    pub(crate) fn source(&self) -> &str {
        self.view.source.text()
    }
    /// printf-style format literals and directive ranges in the current source.
    pub(crate) fn format_directives(&self) -> Vec<highlight::FormatDirective> {
        highlight::format_directives(self.workspace, self.view.file, self.view.source.text())
    }
    pub(crate) fn into_source(self) -> Cow<'a, str> {
        self.view.source.into_text()
    }
    pub(crate) fn callable_type(&mut self, annotation: &str) -> Option<bool> {
        let arena = self
            .arena
            .get_or_insert_with(|| TypeArena::new(self.workspace));
        let ty = arena.parse(self.view.file, annotation)?;
        arena.is_callable(ty)
    }
}

pub(crate) fn cursor_function(
    workspace: &Workspace,
    file: &Path,
    source: &str,
    offset: usize,
) -> Option<(usize, Range<usize>)> {
    workspace
        .files
        .get(file)?
        .symbols
        .iter()
        .find_map(|symbol| {
            let body = symbol.body.as_ref()?;
            (body.contains(&offset) || offset == source.len() && body.end == offset)
                .then(|| symbol.declaration().map(|span| (span.start, body.clone())))
                .flatten()
        })
}
pub(crate) fn cursor_scope(
    workspace: &Workspace,
    file: &Path,
    source: &str,
    offset: usize,
) -> Result<CursorScope, (i32, String)> {
    Locals::scope_at_cursor(
        source,
        offset,
        cursor_function(workspace, file, source, offset),
    )
}
