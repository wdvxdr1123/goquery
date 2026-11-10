//! Interface unions have zero promotion depth; struct fields add one level.
use super::{OwnerKind, TypeContext};
use crate::analysis::type_resolution::Resolution;
use crate::index::IndexedSymbol as Symbol;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};

impl<'a> TypeContext<'a> {
    /// Member lookup already selected this declaration. Recover its unique
    /// shallowest environment without flattening names across package scopes.
    pub(in crate::analysis::type_resolution) fn owner_environment(
        &mut self,
        input: usize,
        owner_file: &Path,
        owner: &Symbol,
        kind: OwnerKind,
    ) -> Option<(&'a PathBuf, Option<usize>)> {
        struct Step<'a> {
            node: usize,
            parent: Option<usize>,
            identity: Option<(&'a PathBuf, usize)>,
            interface_group: Option<usize>,
        }
        let mut steps = vec![Step {
            node: input,
            parent: None,
            identity: None,
            interface_group: None,
        }];
        let mut level = VecDeque::from([0]);
        for _ in 0..128 {
            let mut next = Vec::new();
            let mut pending = Vec::new();
            let mut found = None;
            while let Some(id) = level.pop_front() {
                if matches!(kind, OwnerKind::Receiver)
                    && let Some(resolved) = self.resolve(steps[id].node, Resolution::Aliases)
                    && resolved.file == owner_file
                    && resolved.symbol.start == owner.start
                {
                    let environment = self.environment(&resolved)?;
                    if found.replace((resolved.file, environment, None)).is_some() {
                        return None;
                    }
                    continue;
                }
                let resolved = match self.resolve(steps[id].node, Resolution::Underlying) {
                    Some(resolved) => resolved,
                    None => continue,
                };
                let identity = (resolved.file, resolved.symbol.start);
                let mut ancestor = steps[id].parent;
                let mut cycle = false;
                while let Some(parent) = ancestor {
                    if steps[parent].identity == Some(identity) {
                        cycle = true;
                        break;
                    }
                    ancestor = steps[parent].parent;
                }
                if cycle {
                    continue;
                }
                steps[id].identity = Some(identity);
                if matches!(kind, OwnerKind::Aggregate)
                    && resolved.file == owner_file
                    && resolved.symbol.start == owner.start
                {
                    let environment = self.environment(&resolved)?;
                    let group = steps[id].interface_group.or_else(|| {
                        resolved
                            .symbol
                            .type_info
                            .as_ref()
                            .is_some_and(|info| info.is_interface())
                            .then_some(id)
                    });
                    if let Some((_, _, previous)) = found {
                        // A selected interface method may be inherited repeatedly
                        // within one interface union. Separate struct field paths
                        // remain ambiguous, even if their methods look identical.
                        if group.is_none() || group != previous {
                            return None;
                        }
                    } else {
                        found = Some((resolved.file, environment, group));
                    }
                    continue;
                }
                if let Some(info) = resolved
                    .symbol
                    .type_info
                    .as_ref()
                    .filter(|info| info.is_interface())
                {
                    let environment = self.environment(&resolved)?;
                    let group = steps[id].interface_group.or(Some(id));
                    for embedded in info.embedded.iter().rev() {
                        let node = self.push(resolved.file, embedded, environment)?;
                        level.push_front(steps.len());
                        steps.push(Step {
                            node,
                            parent: Some(id),
                            identity: None,
                            interface_group: group,
                        });
                        if steps.len() > 4096 {
                            return None;
                        }
                    }
                } else {
                    pending.push((id, resolved));
                }
            }
            if let Some((origin, environment, _)) = found {
                return Some((origin, environment));
            }
            for (id, resolved) in pending {
                let environment = self.environment(&resolved)?;
                if let Some(info) = &resolved.symbol.type_info {
                    for embedded in &info.embedded {
                        let node = self.push(resolved.file, embedded, environment)?;
                        next.push(steps.len());
                        steps.push(Step {
                            node,
                            parent: Some(id),
                            identity: None,
                            interface_group: None,
                        });
                        if steps.len() > 4096 {
                            return None;
                        }
                    }
                }
            }
            if next.is_empty() {
                return None;
            }
            level = next.into();
        }
        None
    }
}
