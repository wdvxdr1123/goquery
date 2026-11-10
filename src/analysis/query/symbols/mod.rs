// Copyright 2021 The Go Authors. All rights reserved.
// Matching policy adapted from gopls v0.24.0 internal/golang/workspace_symbol.go.
// BSD-3-Clause license: LICENSE-GO at the repository root.

//! Bounded workspace symbol ranking over compact declaration metadata.
mod document;
mod positions;
mod query;
use super::{Context, SourceLocation, SourceRange};
pub(crate) use document::{DocumentSymbol, DocumentSymbolKind, document_symbols};

use crate::index::IndexedSymbol as Symbol;
use crate::syntax::MemberKind;
use query::Query;
use std::cmp::Ordering;
use std::collections::HashSet;
use std::path::Path;

const MAX_RESULTS: usize = 100;
const MAX_NAME_BYTES: usize = 16 * 1024;

struct Candidate<'a> {
    name: String,
    container: String,
    file: &'a Path,
    symbol: &'a Symbol,
    member: Option<(&'a str, MemberKind)>,
    score: f64,
}

pub(crate) enum SearchKind {
    Declaration,
    Method,
    Interface,
    Field,
    Struct,
}
pub(crate) struct SearchSymbol<'a> {
    pub(crate) name: String,
    pub(crate) container: String,
    pub(crate) location: SourceLocation<'a>,
    pub(crate) kind: SearchKind,
    pub(crate) symbol: &'a Symbol,
}

impl<'a, View> Context<'a, View> {
    pub(crate) fn workspace_symbols(
        &self,
        text: &str,
    ) -> Result<Option<Vec<SearchSymbol<'a>>>, (i32, String)> {
        let query = Query::new(text)?;
        if query.is_empty() {
            return Ok(None);
        }
        let mut best: Vec<Candidate<'_>> = Vec::with_capacity(MAX_RESULTS);
        let mut core = String::new();
        let mut qualified = String::new();
        let mut lowercase = String::new();
        // A real package may itself end in `_test`. Only test files belonging
        // to an external test package get a synthetic import-path suffix.
        // Borrow the exceptional regular-package identities for this request;
        // rebuilding handles unsaved package clauses and file invalidation.
        let regular_test_names: HashSet<_> = self
            .workspace
            .files
            .iter()
            .filter(|(file, stub)| stub.package.ends_with("_test") && !test_file(file))
            .map(|(file, stub)| (file.parent(), stub.package.as_str()))
            .collect();
        for (file, stub) in &self.workspace.files {
            if self.workspace.is_detached(file) {
                continue;
            }
            let import = self.workspace.import_path(file);
            let has_module = import.is_some();
            let mut import = import.unwrap_or_else(|| stub.package.clone());
            if has_module
                && test_file(file)
                && stub.package.ends_with("_test")
                && !regular_test_names.contains(&(file.parent(), stub.package.as_str()))
            {
                import.push_str("_test");
            }
            if import.len() + stub.package.len() > MAX_NAME_BYTES {
                continue;
            }
            for symbol in &stub.symbols {
                let members = symbol.type_info.iter().flat_map(|info| {
                    let kind = if info.is_interface() {
                        MemberKind::Method
                    } else {
                        MemberKind::Field
                    };
                    info.members
                        .iter()
                        .map(move |name| (name.as_str(), kind))
                        .chain(
                            info.search_members
                                .iter()
                                .map(|member| (member.name.as_str(), member.kind)),
                        )
                });
                for member in std::iter::once(None).chain(members.map(Some)) {
                    core.clear();
                    let receiver = symbol.receiver.as_deref().unwrap_or("");
                    if symbol.name.len()
                        + receiver.len()
                        + member.map_or(0, |(name, _)| name.len())
                        + import.len()
                        + stub.package.len()
                        + 3
                        > MAX_NAME_BYTES
                    {
                        continue;
                    }
                    if !receiver.is_empty() {
                        core.push_str(receiver);
                        core.push('.');
                    }
                    core.push_str(&symbol.name);
                    if let Some((member, _)) = member {
                        core.push('.');
                        core.push_str(member);
                    }
                    let Some((name, mut score)) = dynamic_match(
                        &query,
                        &core,
                        &stub.package,
                        &import,
                        &mut qualified,
                        &mut lowercase,
                    ) else {
                        continue;
                    };
                    let depth = core.bytes().filter(|byte| *byte == b'.').count().min(3);
                    score *= 1.0 - 0.01 * depth as f64;
                    let position = best.partition_point(|other| {
                        compare(
                            (other.score, &other.name, other.file, other.symbol.start),
                            (score, name, file, symbol.start),
                        ) != Ordering::Greater
                    });
                    if position == MAX_RESULTS {
                        continue;
                    }
                    if best.len() == MAX_RESULTS {
                        best.pop();
                    }
                    best.insert(
                        position,
                        Candidate {
                            name: name.to_owned(),
                            container: import.clone(),
                            file,
                            symbol,
                            member,
                            score,
                        },
                    );
                }
            }
        }
        if best.is_empty() {
            return Ok(None);
        }
        Ok(self.symbol_positions(best))
    }
}

fn test_file(file: &Path) -> bool {
    file.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with("_test.go"))
}

// Match the shortest qualification that explains the query, following gopls'
// default DynamicSymbols style. Buffers are reused across all declarations.
fn dynamic_match<'a>(
    query: &Query,
    core: &'a str,
    package: &str,
    import: &str,
    qualified: &'a mut String,
    lowercase: &mut String,
) -> Option<(&'a str, f64)> {
    let path_ends_package = import.ends_with(package);
    if !path_ends_package {
        qualified.clear();
        qualified.push_str(package);
        qualified.push('.');
        qualified.push_str(core);
        if let Some(hit) = query.score(qualified, lowercase) {
            return Some(if hit.start > package.len() {
                (core, hit.score)
            } else {
                (qualified.as_str(), hit.score * 0.8)
            });
        }
    }
    qualified.clear();
    qualified.push_str(import);
    qualified.push('.');
    qualified.push_str(core);
    let hit = query.score(qualified, lowercase)?;
    Some(if hit.start > import.len() {
        (core, hit.score)
    } else if path_ends_package && hit.start >= import.len() - package.len() {
        (&qualified[import.len() - package.len()..], hit.score)
    } else {
        (qualified.as_str(), hit.score * 0.6)
    })
}

fn compare(
    (a_score, a, a_path, a_start): (f64, &str, &Path, usize),
    (b_score, b, b_path, b_start): (f64, &str, &Path, usize),
) -> Ordering {
    b_score
        .total_cmp(&a_score)
        .then_with(|| a.len().cmp(&b.len()))
        .then_with(|| a.cmp(b))
        .then_with(|| a_path.cmp(b_path))
        .then_with(|| a_start.cmp(&b_start))
}
