//! Bounded query-local package value facts; pending/failed entries stay unknown.
use super::{Operand, TypeArena, TypeId};
use crate::analysis::query::budget;
use std::mem;
use std::path::Path;

pub(super) struct ValueFact<'a> {
    file: &'a Path,
    selection: usize,
    operand: Option<Operand>,
}

impl TypeArena<'_> {
    pub(crate) fn cached_value(&self, file: &Path, selection: usize) -> Option<Option<Operand>> {
        self.value_facts
            .iter()
            .find(|fact| fact.file == file && fact.selection == selection)
            .map(|fact| fact.operand)
    }

    pub(crate) fn start_value(&mut self, file: &Path, selection: usize) -> Option<usize> {
        self.step()?;
        let limit =
            (budget::MEMO_BYTES / mem::size_of::<ValueFact<'_>>()).min(budget::MEMO_ENTRIES);
        if self.value_facts.len() >= limit {
            return None;
        }
        let file = self.workspace.files.get_key_value(file)?.0.as_path();
        if self.value_facts.len() == self.value_facts.capacity() {
            let capacity = (self.value_facts.capacity().max(1) * 2).min(limit);
            self.value_facts
                .reserve_exact(capacity - self.value_facts.len());
        }
        let index = self.value_facts.len();
        self.value_facts.push(ValueFact {
            file,
            selection,
            operand: None,
        });
        Some(index)
    }

    pub(crate) fn finish_value(&mut self, index: usize, operand: Option<Operand>) {
        self.value_facts[index].operand = operand;
    }

    pub(crate) fn assign_value(
        &mut self,
        source: Operand,
        target: TypeId,
        int_bits: u32,
    ) -> Option<bool> {
        self.assignable(source, target, int_bits)
    }
}
