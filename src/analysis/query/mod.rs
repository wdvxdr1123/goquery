//! A request-local immutable metadata/source view, with a lazy native type arena.
mod builtin;
mod calls;
pub(crate) mod completion;
pub(crate) mod diagnostics;
mod documentation;
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
pub(crate) mod semantic_tokens;
mod semantics;
mod signatures;
pub(crate) mod symbols;
pub(crate) use crate::analysis::types::TypeDeclaration;
pub(crate) use locations::{SourceLocation, SourceRange};
pub(crate) use navigation::{ReferenceTarget, Target as NavigationSymbolTarget};
pub(crate) use signatures::SignatureTrigger;
pub(crate) use signatures::label::CallableSignature;
mod source;
use crate::workspace::Workspace;
use source::SourceView;
use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::path::Path;

pub(crate) struct Document<'a> {
    file: &'a Path,
    source: SourceView<'a>,
}

pub(crate) struct Context<'a, View = Document<'a>> {
    workspace: &'a Workspace,
    view: View,
    build_view: Cell<Option<crate::workspace::BuildView<'a>>>,
    sema: RefCell<Option<crate::analysis::sema::Semantics<'a>>>,
}
impl<'a, View> Context<'a, View> {
    pub(crate) fn with_sema<R>(
        &self,
        f: impl FnOnce(&mut crate::analysis::sema::Semantics<'a>) -> R,
    ) -> R {
        let mut guard = self.sema.borrow_mut();
        let sema = guard.get_or_insert_with(|| {
            crate::analysis::sema::Semantics::new(
                self.workspace,
                self.build_view
                    .get()
                    .unwrap_or_else(|| self.workspace.build_view(None)),
            )
        });
        f(sema)
    }

    pub(in crate::analysis::query) fn select_query_file(
        &self,
        file: &Path,
    ) -> Result<(), (i32, String)> {
        if self.build_view.get().is_some() {
            return Ok(());
        }
        if self.sema.borrow().is_some() {
            return Err((
                crate::error::REQUEST_FAILED,
                "semantic context has no selected build view".into(),
            ));
        }
        self.build_view
            .set(Some(self.workspace.build_view(Some(file))));
        Ok(())
    }

    pub(in crate::analysis::query) fn resolver(&self) -> crate::analysis::resolve::View<'a> {
        crate::analysis::resolve::View::new(
            self.workspace,
            self.build_view
                .get()
                .unwrap_or_else(|| self.workspace.build_view(None)),
        )
    }

    pub(crate) fn hierarchy_view_origin(&self, file: &Path) -> Option<&'a Path> {
        self.build_view
            .get()
            .and_then(|view| view.origin_for(self.workspace, file))
    }

    pub(in crate::analysis::query) fn resolve_identity<'identity>(
        &self,
        resolver: &mut crate::analysis::sema::IdentityResolver<'identity>,
        analysis: &crate::analysis::scope::occurrences::Occurrences,
        source: &str,
        name: &crate::analysis::scope::occurrences::Name,
    ) -> Option<crate::analysis::sema::Identity<'identity>>
    where
        'a: 'identity,
    {
        self.with_sema(|sema| resolver.resolve(sema, analysis, source, name))
    }

    pub(in crate::analysis) fn workspace(&self) -> &'a Workspace {
        self.workspace
    }

    pub(crate) fn sdk_root_path(&self) -> Option<&'a Path> {
        self.workspace.sdk_root_path()
    }

    pub(crate) fn document_version_of(&self, file: &Path) -> Option<i32> {
        self.workspace.document_version(file)
    }

    pub(crate) fn import_path(&self, file: &Path) -> Option<String> {
        self.workspace.import_path(file)
    }

    pub(crate) fn file_symbols(&self, file: &Path) -> Option<&'a [crate::index::IndexedSymbol]> {
        self.workspace
            .files
            .get(file)
            .map(|file| file.symbols.as_slice())
    }

    pub(crate) fn trace_candidates(&self, query: &str, count: usize) {
        crate::analysis::resolve::trace_candidates(self.workspace, query, count);
    }

    pub(crate) fn read_source(&self, file: &Path) -> Option<String> {
        self.workspace.source(file)
    }
}
impl<'a> Context<'a, ()> {
    pub(crate) fn metadata(workspace: &'a Workspace) -> Self {
        Self {
            workspace,
            view: (),
            build_view: Cell::new(None),
            sema: RefCell::new(None),
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
            build_view: Cell::new(Some(workspace.build_view(Some(file)))),
            sema: RefCell::new(None),
        })
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
            build_view: Cell::new(Some(workspace.build_view(Some(file)))),
            sema: RefCell::new(None),
        })
    }
    pub(crate) fn file(&self) -> &'a Path {
        self.view.file
    }
    pub(crate) fn source(&self) -> &str {
        self.view.source.text()
    }
    pub(crate) fn is_detached(&self) -> bool {
        self.workspace.is_detached(self.view.file)
    }

    pub(crate) fn document_version(&self) -> Option<i32> {
        self.workspace.document_version(self.view.file)
    }

    pub(crate) fn into_source_and_metadata(self) -> (Cow<'a, str>, Context<'a, ()>) {
        (
            self.view.source.into_text(),
            Context {
                workspace: self.workspace,
                view: (),
                build_view: self.build_view,
                sema: self.sema,
            },
        )
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
            let body = symbol.body()?;
            (body.contains(&offset) || offset == source.len() && body.end == offset)
                .then(|| symbol.declaration().map(|span| (span.start, body.clone())))
                .flatten()
        })
}
