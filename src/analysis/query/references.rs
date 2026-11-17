//! Cross-file reference checking keeps only bounded semantic location rows.
use super::locations;
use super::navigation::anonymous;
use super::{Context, SourceLocation, SourceRange};
use crate::analysis::obj::MemberDeclId;
use crate::analysis::resolve::{self, NameProbe};
use crate::analysis::scope::occurrences::{Occurrences, Role};
use crate::analysis::sema::{Identity, IdentityResolver};
use crate::bloom::Fingerprint;
use crate::index::IndexedSymbol as Symbol;
use crate::text::SourceMap;
use std::path::Path;
use std::time::{Duration, Instant};

impl<View> Context<'_, View> {
    /// Anonymous fields keep declaration-site identity across copies and files.
    pub(crate) fn inline_field_references<'query>(
        &'query self,
        target_file: &Path,
        selection: &std::ops::Range<usize>,
        name: &str,
        origin: &Path,
        include_declaration: bool,
    ) -> Result<Vec<SourceLocation<'query>>, (i32, String)> {
        let workspace = self.workspace();
        let Some(target_stub) = workspace.files.get(target_file) else {
            return Ok(Vec::new());
        };
        let mut candidates = if resolve::exported(name) {
            self.resolver().dependent_candidates(target_file)
        } else {
            workspace
                .files
                .iter()
                .filter(|(file, stub)| {
                    file.parent() == target_file.parent() && stub.package == target_stub.package
                })
                .collect()
        };
        let scope = workspace.reference_scope(origin);
        candidates.retain(|(file, _)| scope.contains(file, target_file));
        let fingerprint = Fingerprint::new(name.as_bytes());
        let mut locations = Vec::new();
        for (file, stub) in candidates {
            if !stub.identifiers.may_contain_fingerprint(fingerprint) {
                continue;
            }
            let source = workspace.source(file).ok_or_else(|| {
                (
                    crate::error::REQUEST_FAILED,
                    "unable to read anonymous-field reference candidate".to_owned(),
                )
            })?;
            let analysis = Occurrences::for_name(&source, stub, name)?;
            let map = SourceMap::new(&source);
            for occurrence in &analysis.names {
                if source[occurrence.span.clone()] != *name {
                    continue;
                }
                if !include_declaration && file == target_file && occurrence.span == *selection {
                    continue;
                }
                let field = anonymous::occurrence_field(self, file, &source, &analysis, occurrence);
                if field
                    .is_some_and(|field| field.file == target_file && field.selection == *selection)
                {
                    locations::push(&mut locations, file, &map, occurrence.span.clone())?;
                }
            }
        }
        Ok(locations)
    }

    pub(crate) fn global_references<'query>(
        &'query self,
        file: &Path,
        index: &NameProbe,
        target_file: &Path,
        target: &Symbol,
        include_declaration: bool,
        query_started: Instant,
    ) -> Result<Vec<SourceLocation<'query>>, (i32, String)> {
        let mut source_time = Duration::ZERO;
        let mut syntax_time = Duration::ZERO;
        let mut scope_time = Duration::ZERO;
        let mut resolve_time = Duration::ZERO;
        let initial_time = query_started.elapsed();
        let mut candidates = self.resolver().reference_candidates(target_file, target);
        let scope = self.workspace.reference_scope(file);
        candidates.retain(|(candidate, _)| scope.contains(candidate, target_file));
        crate::analysis::resolve::trace_candidates(self.workspace, "references", candidates.len());
        let candidate_time = query_started.elapsed() - initial_time;
        let candidate_count = candidates.len();
        let target_import = self.workspace.import_path(target_file);
        let mut lexical_candidates = 0;
        let mut locations = Vec::new();
        let fingerprint = Fingerprint::new(index.name().as_bytes());
        for (candidate, stub) in candidates {
            if !stub.identifiers.may_contain_fingerprint(fingerprint) {
                continue;
            }
            lexical_candidates += 1;
            let phase = Instant::now();
            let Some(source) = self.workspace.source(candidate) else {
                continue;
            };
            if !source.contains(&target.name) {
                source_time += phase.elapsed();
                continue;
            }
            source_time += phase.elapsed();
            let phase = Instant::now();
            let map = SourceMap::new(&source);
            let same_package = candidate.parent() == target_file.parent()
                && stub.package == self.workspace.files[target_file].package;
            let qualifiers: Vec<_> = stub
                .imports
                .iter()
                .filter(|import| Some(import.path.as_str()) == target_import.as_deref())
                .filter_map(|import| self.resolver().import_name(candidate, import))
                .collect();
            let analysis =
                if target.receiver.is_none() && !same_package && !qualifiers.contains(&".") {
                    Occurrences::for_qualified_name(&source, stub, &target.name, &qualifiers)?
                } else {
                    Occurrences::for_name(&source, stub, &target.name)?
                };
            syntax_time += phase.elapsed();
            if analysis.names.is_empty() {
                continue;
            }
            let phase = Instant::now();
            let mut resolver =
                IdentityResolver::for_reference(self.workspace, candidate, index, target);
            scope_time += phase.elapsed();
            let phase = Instant::now();
            for name in &analysis.names {
                if source[name.span.clone()] == target.name
                    && (include_declaration
                        || candidate != target_file
                        || name.span.start != target.start)
                    && matches!(self.resolve_identity(&mut resolver, &analysis,&source,name), Some(Identity::Global(path,symbol)) if path == target_file && symbol.start == target.start)
                {
                    locations::push(&mut locations, candidate, &map, name.span.clone())?;
                }
            }
            resolve_time += phase.elapsed();
        }
        if self.workspace.trace.queries {
            eprintln!("references identifiers: {lexical_candidates}/{candidate_count} candidates");
            eprintln!(
                "references profile: initial={initial_time:?} candidates={candidate_time:?} source={source_time:?} syntax={syntax_time:?} scope={scope_time:?} resolve={resolve_time:?} total={:?}",
                query_started.elapsed()
            );
        }
        Ok(locations)
    }
    pub(crate) fn member_references<'query>(
        &'query self,
        target: MemberDeclId<'query>,
        index: &NameProbe,
        include_declaration: bool,
        origin: &Path,
    ) -> Result<Vec<SourceLocation<'query>>, (i32, String)> {
        let identity = target;
        let Some(target) = target.describe(self.workspace) else {
            return Ok(Vec::new());
        };
        let mut candidates = if resolve::exported(target.name) {
            self.resolver().dependent_candidates(target.file)
        } else {
            self.workspace
                .files
                .iter()
                .filter(|(path, stub)| {
                    path.parent() == target.file.parent()
                        && stub.package == self.workspace.files[target.file].package
                })
                .collect()
        };
        let scope = self.workspace.reference_scope(origin);
        candidates.retain(|(file, _)| scope.contains(file, target.file));
        crate::analysis::resolve::trace_candidates(self.workspace, "references", candidates.len());
        let mut locations = Vec::new();
        if include_declaration
            && target.embedded
            && let Some(data) = target.data(self.workspace)
        {
            let range = data.selection;
            locations.push(SourceLocation {
                file: target.file,
                range: SourceRange {
                    start: super::locations::SourcePosition {
                        line: range.start.line,
                        utf16_column: range.start.character,
                    },
                    end: super::locations::SourcePosition {
                        line: range.end.line,
                        utf16_column: range.end.character,
                    },
                },
            });
        }
        let fingerprint = Fingerprint::new(target.name.as_bytes());
        for (file, stub) in candidates {
            if !stub.identifiers.may_contain_fingerprint(fingerprint) {
                continue;
            }
            let Some(source) = self.workspace.source(file) else {
                continue;
            };
            if !source.contains(target.name) {
                continue;
            }
            let analysis = Occurrences::for_name(&source, stub, target.name)?;
            let map = SourceMap::new(&source);
            let mut resolver = IdentityResolver::for_name(self.workspace, file, index);
            for name in &analysis.names {
                if (!include_declaration && matches!(name.role, Role::FieldDeclaration(_)))
                    || &source[name.span.clone()] != target.name
                {
                    continue;
                }
                if matches!(self.resolve_identity(&mut resolver, &analysis, &source, name), Some(Identity::Member(member)) if member == identity)
                {
                    locations::push(&mut locations, file, &map, name.span.clone())?;
                }
            }
        }
        Ok(locations)
    }
}
