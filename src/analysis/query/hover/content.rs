//! Protocol-independent hover facts. Markdown/wire encoding belongs to features.
use super::SourceRange;

/// One hover result: a primary code block, optional documentation and footer.
pub(crate) struct HoverContent {
    pub(crate) range: SourceRange,
    /// Primary fenced block body, rendered exactly like gopls' signature/type
    /// declaration text (without the fence).
    pub(crate) code: Option<String>,
    /// Raw Go doc comment text after comment markers are removed. The feature
    /// layer converts it to Markdown and resolves default doc-link URLs.
    pub(crate) doc: Option<String>,
    /// Doc-link bracket text (as written, without brackets) to URI.
    pub(crate) doc_links: Vec<(String, String)>,
    /// Additional fenced blocks, in gopls order: promoted fields, methods.
    pub(crate) extra: Vec<String>,
    pub(crate) footer: Option<String>,
    pub(crate) link: Option<HoverLink>,
}

pub(crate) struct HoverLink {
    /// Text inside the pkg.go.dev link, e.g. `bytes.IndexByte`.
    pub(crate) symbol: String,
    /// Import path (module version suffix included for dependencies).
    pub(crate) path: String,
    /// Anchor, e.g. `IndexByte`.
    pub(crate) anchor: String,
}

impl HoverContent {
    pub(super) fn new(range: SourceRange) -> Self {
        Self {
            range,
            code: None,
            doc: None,
            doc_links: Vec::new(),
            extra: Vec::new(),
            footer: None,
            link: None,
        }
    }

    /// No renderable section: the feature layer answers null instead of an empty
    /// markdown string.
    pub(crate) fn is_empty(&self) -> bool {
        self.code.is_none()
            && self.doc.is_none()
            && self.extra.is_empty()
            && self.footer.is_none()
            && self.link.is_none()
    }
}
