//! Incoming edges follow symbol references, including callback values and closures.
use super::{CallGroup, Groups, finish, record_call};
use crate::analysis::query::{Context, SourceRange};
use crate::analysis::sema::{
    MethodOrigin, SemanticObject, SymbolOrigin, lexical_name_spans, lexical_qualified_name_spans,
};
use crate::bloom::Fingerprint;
use crate::index::IndexedSymbol as Symbol;
use crate::text::SourceMap;
use std::path::Path;

impl<'a, View> Context<'a, View> {
    pub(crate) fn incoming_calls(
        &mut self,
        file: &'a Path,
        target: &'a Symbol,
    ) -> Result<Vec<CallGroup<'a>>, (i32, String)> {
        if target.name == "_" {
            return Ok(Vec::new());
        }
        let mut groups = Groups::new();
        let mut count = 0;
        let mut candidates = self.resolver().reference_candidates(file, target);
        let scope = self.workspace.reference_scope(file);
        candidates.retain(|(candidate, _)| scope.contains(candidate, file));
        crate::analysis::resolve::trace_candidates(
            self.workspace,
            "incomingCalls",
            candidates.len(),
        );
        let package_global_only = target.receiver.is_none();
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
                .filter_map(|import| self.resolver().import_name(candidate, import))
                .collect();
            let spans = if package_global_only && !same_package && !qualifiers.contains(&".") {
                lexical_qualified_name_spans(&source, stub, &target.name, &qualifiers)
            } else {
                lexical_name_spans(&source, stub, &target.name)
            };
            for span in spans {
                if candidate == file && span.start == target.start {
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
                if !decl.is_some_and(|d| d.file == file && d.symbol.start == target.start) {
                    continue;
                }
                let Some(caller) = stub
                    .symbols
                    .iter()
                    .find(|symbol| symbol.body().is_some_and(|body| body.contains(&span.start)))
                    .or_else(|| {
                        stub.symbols.iter().find(|symbol| {
                            symbol.is_value()
                                && map
                                    .offset(symbol.range.start)
                                    .zip(map.offset(symbol.range.end))
                                    .is_some_and(|(start, end)| (start..end).contains(&span.start))
                        })
                    })
                else {
                    continue;
                };
                record_call(
                    &mut groups,
                    candidate,
                    caller,
                    SourceRange::new(&map, span),
                    &mut count,
                )?;
            }
        }
        Ok(finish(groups))
    }
}
