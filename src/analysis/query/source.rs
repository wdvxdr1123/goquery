//! One immutable document view: borrow open buffers, own one bounded closed read.
use crate::workspace::Workspace;
use std::borrow::Cow;
use std::path::Path;

pub(super) struct SourceView<'a>(Cow<'a, str>);
impl<'a> SourceView<'a> {
    pub(super) fn admitted(workspace: &'a Workspace, file: &Path, closed: Option<String>) -> Self {
        Self(match closed {
            Some(source) => Cow::Owned(source),
            None => Cow::Borrowed(workspace.open_source(file).expect("admitted open buffer")),
        })
    }
    pub(super) fn borrowed(source: &'a str) -> Self {
        Self(Cow::Borrowed(source))
    }
    pub(super) fn text(&self) -> &str {
        &self.0
    }
    pub(super) fn into_text(self) -> Cow<'a, str> {
        self.0
    }
}
