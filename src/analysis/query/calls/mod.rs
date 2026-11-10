//! Bounded semantic call edges; no protocol items or retained call graph.
mod incoming;
mod outgoing;
use super::{Context, SourceRange};
use crate::index::IndexedSymbol as Symbol;
use std::collections::BTreeMap;
use std::path::Path;

pub(crate) struct CallGroup<'a> {
    pub(crate) file: &'a Path,
    pub(crate) symbol: &'a Symbol,
    pub(crate) sites: Vec<SourceRange>,
}
type Groups<'a> = BTreeMap<(&'a Path, usize), CallGroup<'a>>;

impl<'a, View> Context<'a, View> {
    pub(crate) fn call_target(
        &self,
        file: &Path,
        start: usize,
        name: Option<&str>,
    ) -> Result<(&'a Path, &'a Symbol), (i32, String)> {
        self.workspace
            .files
            .get_key_value(file)
            .and_then(|(file, stub)| {
                stub.symbols
                    .iter()
                    .find(|symbol| {
                        symbol.start == start
                            && name == Some(symbol.name.as_str())
                            && !symbol.is_type()
                    })
                    .map(|symbol| (file.as_path(), symbol))
            })
            .ok_or((-32801, "call hierarchy item is stale".into()))
    }
}
fn record_call<'a>(
    groups: &mut Groups<'a>,
    file: &'a Path,
    symbol: &'a Symbol,
    range: SourceRange,
    count: &mut usize,
) -> Result<(), (i32, String)> {
    if *count == super::budget::MAX_CALL_SITES {
        return Err((
            crate::error::REQUEST_FAILED,
            "call hierarchy result exceeds 4096 call sites".into(),
        ));
    }
    *count += 1;
    groups
        .entry((file, symbol.start))
        .or_insert_with(|| CallGroup {
            file,
            symbol,
            sites: Vec::new(),
        })
        .sites
        .push(range);
    Ok(())
}
fn finish(groups: Groups<'_>) -> Vec<CallGroup<'_>> {
    groups
        .into_values()
        .map(|mut group| {
            group
                .sites
                .sort_by_key(|range| (range.start.line, range.start.utf16_column));
            group.sites.dedup();
            group
        })
        .collect()
}
