//! Ad-hoc expression queries use the same checker state as enclosing regions.
use super::Semantics;
use crate::analysis::budget::MAX_BYTES;
use crate::analysis::check::{RegionChecker, region::RegionInfo};
use crate::analysis::scope::tree::ScopeTree;
use crate::analysis::types::TypeArena;
use crate::syntax::{self, BlockTree};
use std::path::Path;

impl<'a> Semantics<'a> {
    pub(crate) fn with_expression<R>(
        &mut self,
        file: &Path,
        text: &str,
        visit: impl FnOnce(&RegionInfo<'a>, &mut TypeArena<'a>, [u32; 2]) -> Option<R>,
    ) -> Option<R> {
        if text.len() > MAX_BYTES {
            return None;
        }
        let file = self.intern_file(file)?;
        let parsed = syntax::parse_expression(text);
        if !parsed.diagnostics().is_empty() {
            return None;
        }
        let root = parsed.root();
        let root_span = parsed.node(root)?.span.clone();
        let tree = BlockTree::from_expression(parsed);
        let scopes = ScopeTree::from_function(text, 0, None, 0, &tree, None);
        let span = [0, u32::try_from(text.len()).ok()?];
        self.prepare_for_new_region();
        let mut region = RegionInfo::new(None, span, scopes);
        RegionChecker::new(
            &mut self.arena,
            &mut self.decls,
            file,
            text,
            0,
            &tree,
            &mut region,
            None,
            None,
        )
        .check_expr_outcome(root, None);
        region.finalize();
        visit(
            &region,
            &mut self.arena,
            [root_span.start as u32, root_span.end as u32],
        )
    }
}
