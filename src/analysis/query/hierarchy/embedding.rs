//! Explicit named-embedding extension, separate from structural implementation.
use super::{Context, SymbolTarget};
use crate::analysis::resolve::NameIndex;
use crate::index::IndexedSymbol as Symbol;
use std::collections::BTreeMap;
use std::path::Path;

impl<'a, View> Context<'a, View> {
    pub(crate) fn embedding_relations(
        &self,
        file: &'a Path,
        target: &'a Symbol,
        supers: bool,
    ) -> Result<Vec<SymbolTarget<'a>>, (i32, String)> {
        let mut results = BTreeMap::new();
        if supers {
            if let Some(info) = &target.type_info {
                for name in &info.embedded {
                    if let Some((path, symbol)) = self.workspace.embedded_target(file, name) {
                        results.insert(
                            (path.as_path(), symbol.start),
                            SymbolTarget { file: path, symbol },
                        );
                        if results.len() > 1024 {
                            return Err((
                                crate::error::REQUEST_FAILED,
                                "type hierarchy exceeds 1024 results".into(),
                            ));
                        }
                    }
                }
            }
        } else {
            let index = NameIndex::new(self.workspace, &target.name);
            let candidates = self.workspace.reference_candidates(file, target);
            self.workspace
                .trace_candidates("subtypes", candidates.len());
            for (path, stub) in candidates {
                for symbol in &stub.symbols {
                    let Some(info) = &symbol.type_info else {
                        continue;
                    };
                    if info.embedded.iter().any(|name| {
                        let name = crate::analysis::scope::named_root(name);
                        let resolved = if let Some((alias, name)) = name.split_once('.') {
                            (name == index.name())
                                .then(|| index.imported(path, alias))
                                .flatten()
                        } else {
                            (name == index.name())
                                .then(|| index.unqualified(path))
                                .flatten()
                        };
                        resolved.is_some_and(|(parent, base)| {
                            parent == file && base.start == target.start
                        })
                    }) {
                        results.insert(
                            (path.as_path(), symbol.start),
                            SymbolTarget { file: path, symbol },
                        );
                    }
                    if results.len() > 1024 {
                        return Err((
                            crate::error::REQUEST_FAILED,
                            "type hierarchy exceeds 1024 results".into(),
                        ));
                    }
                }
            }
        }
        Ok(results.into_values().collect())
    }
}
