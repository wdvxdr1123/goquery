//! Compiler-grade file diagnostics computed from one admitted source snapshot.
//!
//! The gopls baseline is the go/types and go/parser diagnostic set with source
//! `compiler` or `syntax`. Every check here is conservative: a name is only
//! reported when its failure is provable from the request-local source and
//! workspace metadata, so valid files never produce false positives. Unsupported
//! constructs stay unreported; the remaining gaps are listed in the design doc.
use super::Context;
use crate::analysis::sema::MAX_DIAGNOSTICS;
pub(crate) use crate::analysis::sema::{Code, Diagnostic, Kind};
use crate::syntax::{self, view};
use crate::workspace::Workspace;
use std::collections::HashSet;
use std::ops::Range;

/// One provably unused import, with the deletion gopls applies for
/// `source.organizeImports` when exactly one import must be removed.
pub(crate) struct UnusedImport {
    pub(crate) span: Range<usize>,
    pub(crate) message: String,
    pub(crate) removal: Option<Range<usize>>,
}

struct ImportSpec {
    /// Decoded import path.
    path: String,
    /// Explicit alias token, including `_` and `.`.
    alias: Option<Range<usize>>,
    /// Span of the quoted import path literal.
    span: Range<usize>,
    declaration_start: usize,
    /// Byte offset just after the group's opening parenthesis.
    group_start: Option<usize>,
    /// End of the previous spec's path literal in the same declaration.
    previous_end: Option<usize>,
}

impl ImportSpec {
    fn alias<'a>(&self, source: &'a str) -> Option<&'a str> {
        self.alias.as_ref().map(|span| &source[span.clone()])
    }

    fn diagnostic_span(&self) -> Range<usize> {
        self.alias.as_ref().unwrap_or(&self.span).clone()
    }
}

/// All diagnostics for the admitted file, or an empty list when the workspace
/// has no metadata for it.
pub(crate) fn diagnostics(context: &mut Context<'_>) -> Vec<Diagnostic> {
    let workspace = context.workspace();
    let file = context.file();
    if !workspace.files.contains_key(file) {
        return Vec::new();
    }
    let mut rows = {
        let source = context.source();
        match validate_imports(source) {
            Ok(specs) => unused_imports_of(workspace, source, &specs)
                .into_iter()
                .map(|import| Diagnostic {
                    span: import.span,
                    kind: Kind::Compiler(Code::UnusedImport),
                    message: import.message,
                    unnecessary: true,
                    related: Vec::new(),
                })
                .collect(),
            Err(error) => return vec![error],
        }
    };
    let (doc, sema) = context.split();
    sema.file_diagnostics(doc.file(), doc.source(), &mut rows);
    rows.truncate(MAX_DIAGNOSTICS);
    rows
}

/// Provably unused imports of the admitted file, for organize-import actions.
pub(crate) fn unused_imports(context: &Context<'_>) -> Vec<UnusedImport> {
    if !context.workspace().files.contains_key(context.file()) {
        return Vec::new();
    }
    let source = context.source();
    let Ok(specs) = validate_imports(source) else {
        return Vec::new();
    };
    unused_imports_of(context.workspace(), source, &specs)
}

fn validate_imports(source: &str) -> Result<Vec<ImportSpec>, Diagnostic> {
    const MAX_IMPORT_SPECS: usize = 4096;
    let mut specs = Vec::new();
    let mut complete = true;
    let errors = syntax::validate_file_with_imports(
        source,
        |spec, declaration_start, group_start, previous_end| {
            if !complete || specs.len() == MAX_IMPORT_SPECS {
                complete = false;
                return;
            }
            let Some(path) = syntax::decode_import_path(&source[spec.path.clone()]) else {
                complete = false;
                return;
            };
            specs.push(ImportSpec {
                path,
                alias: spec.alias,
                span: spec.path,
                declaration_start,
                group_start,
                previous_end,
            });
        },
    );
    // gopls publishes the first position-sorted parser error in each file.
    if let Some(error) = errors.into_iter().min_by_key(|error| error.span.start) {
        return Err(Diagnostic {
            span: error.span.start..error.span.start,
            kind: Kind::Syntax,
            message: error.display_message(source),
            unnecessary: false,
            related: Vec::new(),
        });
    }
    Ok(if complete { specs } else { Vec::new() })
}

fn unused_imports_of(
    workspace: &Workspace,
    source: &str,
    specs: &[ImportSpec],
) -> Vec<UnusedImport> {
    if specs.is_empty() {
        return Vec::new();
    }
    // A `C` import turns the file into cgo input whose names are resolved by the
    // cgo tool, and a dot import contributes names we cannot enumerate. In both
    // cases a "not used" claim is not provable from this source alone.
    if specs
        .iter()
        .any(|spec| spec.path == "C" || spec.alias(source) == Some("."))
    {
        return Vec::new();
    }
    let named_specs: Vec<_> = specs
        .iter()
        .filter_map(|spec| {
            let name = match spec.alias(source) {
                Some("_") => return None,
                Some(alias) => alias,
                None => workspace.declared_package_name(&spec.path)?,
            };
            Some((name, spec))
        })
        .collect();
    let mut unreferenced: HashSet<_> = named_specs.iter().map(|(name, _)| *name).collect();
    if !unreferenced.is_empty() {
        for name in syntax::qualified_prefixes(source) {
            unreferenced.remove(name);
            if unreferenced.is_empty() {
                return Vec::new();
            }
        }
    }

    let mut candidates = Vec::new();
    for (name, spec) in named_specs {
        if !unreferenced.contains(name) {
            continue;
        }
        let path = syntax::quote_string(&spec.path);
        let message = if spec.path.rsplit('/').next() == Some(name) {
            format!("{path} imported and not used")
        } else {
            format!("{path} imported as {name} and not used")
        };
        candidates.push(UnusedImport {
            span: spec.diagnostic_span(),
            message,
            removal: None,
        });
    }
    if candidates.len() == 1 {
        let spec = specs
            .iter()
            .find(|spec| candidates[0].span == spec.diagnostic_span())
            .expect("candidate came from specs");
        candidates[0].removal = removal_span(source, spec);
    }
    // Several removals need gopls' minimized multi-edit; diagnostics stay, but
    // this module declines to guess the edit shape.
    candidates
}

/// Deletes the import spec the way gopls' formatted diff does when it is the
/// only removal: a later group member loses the newline before it, a first
/// group member loses the newline after the opening parenthesis, and a lone
/// import declaration loses its whole line plus one preceding blank line.
fn removal_span(source: &str, spec: &ImportSpec) -> Option<Range<usize>> {
    match (spec.group_start, spec.previous_end) {
        (Some(_), Some(previous_end)) if previous_end < spec.span.start => {
            Some(previous_end..spec.span.end)
        }
        (Some(group_start), None) => Some(group_start..spec.span.end),
        (Some(_), _) => None,
        (None, _) => {
            let start_of_line = line_start(source, spec.declaration_start);
            let end_of_line = next_line_start(source, spec.span.end)?;
            let start = if start_of_line == 0 {
                0
            } else {
                let previous_line = line_start(source, start_of_line - 1);
                if source
                    .get(previous_line..start_of_line - 1)
                    .is_some_and(|line| line.trim().is_empty())
                {
                    previous_line
                } else {
                    start_of_line
                }
            };
            Some(start..end_of_line)
        }
    }
}

pub(crate) fn variable_removal(context: &Context<'_>, span: &Range<usize>) -> Option<Range<usize>> {
    view::variable_removal(context.source(), span)
}

fn line_start(source: &str, offset: usize) -> usize {
    source
        .get(..offset)
        .and_then(|prefix| prefix.rfind('\n').map(|newline| newline + 1))
        .unwrap_or(0)
}

fn next_line_start(source: &str, offset: usize) -> Option<usize> {
    source
        .get(offset..)
        .and_then(|rest| rest.find('\n').map(|newline| offset + newline + 1))
}
