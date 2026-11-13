//! Incoming edges follow symbol references, including callback values and closures.
use super::{CallGroup, Groups, finish, record_call};
use crate::analysis::query::{Context, SourceRange};
use crate::analysis::resolve::NameIndex;
use crate::analysis::scope::{Identity, Resolver};
use crate::analysis::source::SourceAnalysis;
use crate::bloom::Fingerprint;
use crate::index::IndexedSymbol as Symbol;
use crate::text::SourceMap;
use std::path::Path;

impl<'a, View> Context<'a, View> {
    pub(crate) fn incoming_calls(
        &self,
        file: &'a Path,
        target: &'a Symbol,
    ) -> Result<Vec<CallGroup<'a>>, (i32, String)> {
        let mut groups = Groups::new();
        let mut count = 0;
        let index = NameIndex::new(self.workspace, &target.name);
        let mut candidates = self.workspace.reference_candidates(file, target);
        let scope = self.workspace.reference_scope(file);
        candidates.retain(|(candidate, _)| scope.contains(candidate, file));
        self.workspace
            .trace_candidates("incomingCalls", candidates.len());
        let fingerprint = Fingerprint::new(target.name.as_bytes());
        for (candidate, stub) in candidates {
            if !stub.identifiers.may_contain_fingerprint(fingerprint) {
                continue;
            }
            let Some(source) = self.workspace.source(candidate) else {
                continue;
            };
            if !source.contains(&target.name) {
                continue;
            }
            let map = SourceMap::new(&source);
            let same_package = candidate.parent() == file.parent()
                && stub.package == self.workspace.files[file].package;
            let import_path = self.workspace.import_path(file);
            let qualifiers: Vec<_> = stub
                .imports
                .iter()
                .filter(|import| Some(import.path.as_str()) == import_path.as_deref())
                .filter_map(|import| self.workspace.import_name(import))
                .collect();
            let analysis =
                if target.receiver.is_none() && !same_package && !qualifiers.contains(&".") {
                    SourceAnalysis::for_qualified_name(&source, stub, &target.name, &qualifiers)?
                } else {
                    SourceAnalysis::for_name(&source, stub, &target.name)?
                };
            let mut resolver = Resolver::for_reference(self.workspace, candidate, &index, target);
            for name in &analysis.names {
                if candidate == file && name.span.start == target.start {
                    continue;
                }
                if !matches!(resolver.resolve(&analysis, &source, name), Some(Identity::Global(path, symbol)) if path == file && symbol.start == target.start)
                {
                    continue;
                }
                let Some(caller) = stub
                    .symbols
                    .iter()
                    .find(|symbol| {
                        symbol
                            .body()
                            .is_some_and(|body| body.contains(&name.span.start))
                    })
                    .or_else(|| {
                        stub.symbols.iter().find(|symbol| {
                            symbol.is_value()
                                && map
                                    .offset(symbol.range.start)
                                    .zip(map.offset(symbol.range.end))
                                    .is_some_and(|(start, end)| {
                                        (start..end).contains(&name.span.start)
                                    })
                        })
                    })
                else {
                    continue;
                };
                record_call(
                    &mut groups,
                    candidate,
                    caller,
                    SourceRange::new(&map, name.span.clone()),
                    &mut count,
                )?;
            }
        }
        Ok(finish(groups))
    }
}
