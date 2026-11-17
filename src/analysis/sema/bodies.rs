//! One bounded request-local syntax body, shared by checking and candidates.
use super::Semantics;
use crate::analysis::budget::MAX_BODY_BYTES;
use crate::analysis::scope::{CursorScope, Locals};
use crate::syntax::BlockTree;
use std::ops::Range;
use std::path::Path;
use std::sync::Arc;

pub(super) struct ParsedBody<'a> {
    file: &'a Path,
    span: Range<usize>,
    tree: Arc<BlockTree>,
}

impl<'a> Semantics<'a> {
    pub(super) fn shared_body(
        &mut self,
        file: &'a Path,
        source: &str,
        span: Range<usize>,
    ) -> Option<Arc<BlockTree>> {
        if span.len() > MAX_BODY_BYTES {
            return None;
        }
        if let Some(body) = &self.parsed_body
            && body.file == file
            && body.span == span
        {
            return Some(Arc::clone(&body.tree));
        }
        self.parsed_body = None;
        let tree = Arc::new(crate::syntax::parse_block(source.get(span.clone())?));
        self.parsed_body = Some(ParsedBody {
            file,
            span,
            tree: Arc::clone(&tree),
        });
        Some(tree)
    }

    pub(crate) fn cursor_scope(
        &mut self,
        file: &Path,
        source: &str,
        offset: usize,
        function: Option<(usize, Range<usize>)>,
        native_cursor: Option<usize>,
    ) -> Result<CursorScope, (i32, String)> {
        let Some(file) = self.intern_file(file) else {
            return Ok(CursorScope {
                available: false,
                locals: Locals::empty(),
                body: None,
            });
        };
        // Finish native checking while the enclosing tree is current. Closure
        // candidate traversal can then release it before selecting an inner body.
        if function
            .as_ref()
            .is_some_and(|(_, body)| body.start <= offset && offset <= body.end)
        {
            let _ = u32::try_from(offset)
                .ok()
                .is_some_and(|offset| self.ensure_region(file, source, offset, native_cursor));
        }
        Locals::scope_at_cursor_with_parser(source, offset, function, |span| {
            self.shared_body(file, source, span)
        })
    }
}
