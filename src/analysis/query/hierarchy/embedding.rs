//! Explicit named-embedding extension, separate from structural implementation.
use super::{Context, SymbolTarget};
use crate::analysis::sema::DeclId;
use crate::index::IndexedSymbol as Symbol;
use std::collections::BTreeMap;
use std::path::Path;

impl<'a, View> Context<'a, View> {
    pub(crate) fn embedding_relations(
        &mut self,
        file: &'a Path,
        target: &'a Symbol,
        supers: bool,
    ) -> Result<Vec<SymbolTarget<'a>>, (i32, String)> {
        let mut results = BTreeMap::new();
        if supers {
            let declarations = self.with_sema(|sema| {
                sema.relations().direct_embedding_declarations(DeclId {
                    file,
                    symbol: target,
                })
            });
            for declaration in declarations.unwrap_or_default() {
                results.insert(
                    (declaration.file, declaration.symbol.start),
                    SymbolTarget {
                        file: declaration.file,
                        symbol: declaration.symbol,
                    },
                );
            }
        } else {
            let candidates = self.resolver().dependent_candidates(file);
            crate::analysis::resolve::trace_candidates(
                self.workspace,
                "subtypes",
                candidates.len(),
            );
            for (path, stub) in candidates {
                for symbol in &stub.symbols {
                    let Some(info) = &symbol.type_info else {
                        continue;
                    };
                    if info.embedded.is_empty() {
                        continue;
                    }
                    let declarations = self.with_sema(|sema| {
                        sema.relations()
                            .direct_embedding_declarations(DeclId { file: path, symbol })
                    });
                    if declarations.is_some_and(|declarations| {
                        declarations.contains(&DeclId {
                            file,
                            symbol: target,
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
