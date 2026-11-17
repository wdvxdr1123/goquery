//! File diagnostics consume one streaming region at a time.
use super::Semantics;
use crate::analysis::check::diagnostics::{self, Diagnostic, MAX_DIAGNOSTICS};
use std::path::Path;

impl Semantics<'_> {
    pub(crate) fn file_diagnostics(
        &mut self,
        file: &Path,
        source: &str,
        rows: &mut Vec<Diagnostic>,
    ) {
        let Some(file) = self.intern_file(file) else {
            return;
        };
        let Some(stub) = self.workspace.files.get(file) else {
            return;
        };
        diagnostics::file_diagnostics(file, stub, source, rows);
        if rows.len() >= MAX_DIAGNOSTICS {
            return;
        }
        self.stream_file_regions(file, source, None, |region, _, _| {
            let remaining = MAX_DIAGNOSTICS.saturating_sub(rows.len());
            rows.extend(region.diagnostics.iter().take(remaining).cloned());
        });
    }
}
