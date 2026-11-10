//! Interfaces merge embedded method sets before struct promotion depth applies.
use crate::analysis::member::MemberDeclaration;
use crate::analysis::resolve;
use crate::index::IndexedSymbol as Symbol;
use crate::syntax::MemberKind;
use crate::workspace::Workspace;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

impl Workspace {
    pub(super) fn interface_members<'a>(
        &'a self,
        consumer: Option<&Path>,
        file: &'a PathBuf,
        owner: &'a Symbol,
    ) -> Option<Vec<MemberDeclaration<'a>>> {
        let mut pending = vec![(file, owner)];
        let mut seen = HashSet::new();
        let mut methods = BTreeMap::new();
        while let Some((file, owner)) = pending.pop() {
            if !seen.insert((file, owner.start)) {
                continue;
            }
            if seen.len() > 4096 {
                return None;
            }
            let info = owner
                .type_info
                .as_ref()
                .filter(|info| info.is_interface())?;
            let same = consumer.is_none_or(|consumer| {
                file.parent() == consumer.parent()
                    && self.files[file].package == self.files[consumer].package
            });
            for name in &info.members {
                if same || resolve::exported(name) {
                    methods
                        .entry(self.member_id(file, name))
                        .or_insert(MemberDeclaration {
                            file,
                            owner,
                            name,
                            kind: MemberKind::Method,
                            embedded: false,
                        });
                }
            }
            // Source-order depth first: complete the first embedded interface's
            // method set before merging the next, independent of struct depth.
            for embedded in info.embedded.iter().rev() {
                if let Some((file, owner)) = self.underlying_target(file, embedded)
                    && owner
                        .type_info
                        .as_ref()
                        .is_some_and(|info| info.is_interface())
                {
                    pending.push((file, owner));
                    if pending.len() > 4096 {
                        return None;
                    }
                }
            }
        }
        Some(methods.into_values().collect())
    }
}
