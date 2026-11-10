//! Call resolution shares the bounded query-local receiver inference context.
use crate::analysis::check::Inference;
use crate::analysis::scope::Locals;
use crate::index::IndexedSymbol as Symbol;
use crate::syntax::{Expr, ExprId};
use crate::workspace::Workspace;
use std::path::{Path, PathBuf};

impl Workspace {
    pub(crate) fn resolve_call(
        &self,
        file: &Path,
        expressions: &[Expr],
        function: ExprId,
        source: &str,
        body_offset: usize,
        locals: &Locals,
    ) -> Option<(&PathBuf, &Symbol)> {
        Inference::new(self, file, source, locals, expressions, body_offset).callee(function)
    }
}
