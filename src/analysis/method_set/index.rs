//! Bounded query-local receiver index; values borrow declaration stubs only.
use crate::index::IndexedSymbol as Symbol;
use crate::workspace::Workspace;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

type Owner<'a> = (Option<&'a Path>, &'a str, &'a str);
type Method<'a> = (&'a PathBuf, &'a Symbol);

pub(crate) struct MethodIndex<'a> {
    methods: HashMap<Owner<'a>, Vec<Method<'a>>>,
}

impl<'a> MethodIndex<'a> {
    pub(crate) fn new(workspace: &'a Workspace) -> Option<Self> {
        let mut methods: HashMap<Owner<'_>, Vec<Method<'_>>> = HashMap::new();
        let mut count = 0;
        // Expansion may use declarations outside the query's candidate view.
        // Index the complete metadata graph, then narrow result candidates.
        for (file, stub) in &workspace.files {
            for symbol in &stub.symbols {
                if let Some(receiver) = &symbol.receiver {
                    count += 1;
                    if count > 65_536 {
                        return None;
                    }
                    methods
                        .entry((file.parent(), &stub.package, receiver))
                        .or_default()
                        .push((file, symbol));
                    if methods.len() > 16_384 {
                        return None;
                    }
                }
            }
        }
        Some(Self { methods })
    }

    pub(crate) fn get(
        &self,
        workspace: &'a Workspace,
        file: &'a Path,
        name: &'a str,
    ) -> &[Method<'a>] {
        self.methods
            .get(&(file.parent(), &workspace.files[file].package, name))
            .map_or(&[], Vec::as_slice)
    }
}
