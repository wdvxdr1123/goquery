//! Cross-file reference checking keeps only bounded semantic location rows.
use super::locations;
use super::{Context, SourceLocation, SourceRange};
use crate::analysis::resolve::{self, NameProbe};
use crate::analysis::sema::{
    MemberDeclId, MethodOrigin, SemanticObject, SymbolOrigin, lexical_name_spans,
    lexical_qualified_name_spans,
};
use crate::bloom::Fingerprint;
use crate::index::IndexedSymbol as Symbol;
use crate::text::SourceMap;
use std::path::Path;
use std::time::{Duration, Instant};

impl<'a, View> Context<'a, View> {
    /// Anonymous fields keep declaration-site identity across copies and files.
    pub(crate) fn inline_field_references(
        &mut self,
        target_file: &Path,
        selection: &std::ops::Range<usize>,
        name: &str,
        origin: &Path,
        include_declaration: bool,
    ) -> Result<Vec<SourceLocation<'a>>, (i32, String)> {
        if name == "_" {
            return Ok(Vec::new());
        }
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
        let target_sel = [
            u32::try_from(selection.start).unwrap_or(u32::MAX),
            u32::try_from(selection.end).unwrap_or(u32::MAX),
        ];
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
            if !source.contains(name) {
                continue;
            }
            let spans = lexical_name_spans(&source, stub, name);
            if spans.is_empty() {
                continue;
            }
            let map = SourceMap::new(&source);
            for span in spans {
                if !include_declaration && file == target_file && span == *selection {
                    continue;
                }
                let key = [span.start as u32, span.end as u32];
                let Some(SemanticObject::Field(member)) =
                    self.with_sema(|sema| sema.resolve_offset(file, &source, key))
                else {
                    continue;
                };
                if !member.builtin && member.file == target_file && member.selection == target_sel {
                    locations::push(&mut locations, file, &map, span)?;
                }
            }
        }
        Ok(locations)
    }

    pub(crate) fn global_references(
        &mut self,
        file: &Path,
        index: &NameProbe,
        target_file: &Path,
        target: &Symbol,
        include_declaration: bool,
        query_started: Instant,
    ) -> Result<Vec<SourceLocation<'a>>, (i32, String)> {
        if target.name == "_" {
            return Ok(Vec::new());
        }
        let mut source_time = Duration::ZERO;
        let mut syntax_time = Duration::ZERO;
        let scope_time = Duration::ZERO;
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
        let package_global_only = target.receiver.is_none();
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
            let spans = if package_global_only && !same_package && !qualifiers.contains(&".") {
                lexical_qualified_name_spans(&source, stub, &target.name, &qualifiers)
            } else {
                lexical_name_spans(&source, stub, &target.name)
            };
            syntax_time += phase.elapsed();
            if spans.is_empty() {
                continue;
            }
            let phase = Instant::now();
            for span in spans {
                if !(include_declaration || candidate != target_file || span.start != target.start)
                {
                    continue;
                }
                let key = [span.start as u32, span.end as u32];
                let Some(obj) = self.with_sema(|sema| sema.resolve_offset(candidate, &source, key))
                else {
                    continue;
                };
                let decl = match obj {
                    SemanticObject::Const(SymbolOrigin::Package(decl))
                    | SemanticObject::TypeName {
                        origin: SymbolOrigin::Package(decl),
                        ..
                    }
                    | SemanticObject::Var {
                        origin: SymbolOrigin::Package(decl),
                        ..
                    }
                    | SemanticObject::Func(decl) => Some(decl),
                    SemanticObject::Method(MethodOrigin::Concrete(decl))
                        if !package_global_only =>
                    {
                        Some(decl)
                    }
                    _ => None,
                };
                if let Some(decl) = decl
                    && decl.file == target_file
                    && decl.symbol.start == target.start
                {
                    locations::push(&mut locations, candidate, &map, span)?;
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

    pub(crate) fn member_references(
        &mut self,
        target: MemberDeclId<'_>,
        _index: &NameProbe,
        include_declaration: bool,
        origin: &Path,
    ) -> Result<Vec<SourceLocation<'a>>, (i32, String)> {
        let identity = target;
        let Some(target) = target.describe(self.workspace) else {
            return Ok(Vec::new());
        };
        if target.name == "_" {
            return Ok(Vec::new());
        }
        let Some((decl_file, _)) = self.workspace.files.get_key_value(target.file) else {
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
                file: decl_file.as_path(),
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
        let decl_sel = [
            identity.selection[0] as usize,
            identity.selection[1] as usize,
        ];
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
            let spans = lexical_name_spans(&source, stub, target.name);
            if spans.is_empty() {
                continue;
            }
            let map = SourceMap::new(&source);
            for span in spans {
                if !include_declaration
                    && file == decl_file.as_path()
                    && span.start == decl_sel[0]
                    && span.end == decl_sel[1]
                {
                    continue;
                }
                let key = [span.start as u32, span.end as u32];
                let Some(obj) = self.with_sema(|sema| sema.resolve_offset(file, &source, key))
                else {
                    continue;
                };
                if matches!(
                    obj,
                    SemanticObject::Field(member) | SemanticObject::Method(MethodOrigin::Interface(member))
                        if member.indexed(self.workspace) == Some(identity)
                ) {
                    locations::push(&mut locations, file, &map, span)?;
                }
            }
        }
        Ok(locations)
    }
}
