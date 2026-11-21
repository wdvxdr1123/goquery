//! Local diagnostics consume canonical ScopeTree and RegionInfo facts.
use super::{Code, Diagnostic, Kind, Related};
use crate::analysis::engine::check::region::{MAX_REGION_DIAGNOSTICS, RecordMask, RegionInfo};
use crate::analysis::engine::obj::{ObjectRef, SymbolOrigin};
use crate::analysis::engine::scope::tree::LocalBindingKind;
use std::collections::HashMap;

impl RegionInfo<'_> {
    /// Local errors are proved by this region's native identities. Incomplete
    /// checking can establish uses, but cannot prove that a binding is unused.
    pub(crate) fn finish_local_diagnostics(&mut self, source: &str) {
        if !self.mask.contains(RecordMask::DIAGNOSTICS) {
            return;
        }
        if !self.scopes.complete {
            self.local_var_uses = Vec::new();
            if !self.mask.contains(RecordMask::WRITES) {
                self.writes = Vec::new();
            }
            return;
        }
        let mut duplicates = vec![false; self.scopes.bindings.len()];
        for node_index in 0..self.scopes.nodes.len() {
            let node = self.scopes.nodes[node_index];
            let mut declarations = HashMap::new();
            let mut current = node.last_binding;
            while let Some(id) = current {
                let binding = self.scopes.bindings[id.0 as usize];
                current = binding.prev_in_scope;
                let span = binding.name_span;
                let Some(name) = source.get(span[0] as usize..span[1] as usize) else {
                    return;
                };
                if name == "_" {
                    continue;
                }
                if let Some((later, later_span)) = declarations.insert(name, (id, span)) {
                    duplicates[id.0 as usize] = true;
                    duplicates[later.0 as usize] = true;
                    if self.diagnostics.len() + 2 <= MAX_REGION_DIAGNOSTICS && name.len() <= 1024 {
                        self.push_diagnostic(Diagnostic {
                            span: later_span[0] as usize..later_span[1] as usize,
                            kind: Kind::Compiler(Code::DuplicateDecl),
                            message: format!("{name} redeclared in this block"),
                            unnecessary: false,
                            related: vec![Related {
                                span: span[0] as usize..span[1] as usize,
                                message: format!("other declaration of {name}"),
                            }],
                        });
                        self.push_diagnostic(Diagnostic {
                            span: span[0] as usize..span[1] as usize,
                            kind: Kind::Compiler(Code::DuplicateDecl),
                            message: format!("{name} redeclared in this block (this error: other declaration of {name})"),
                            unnecessary: false,
                            related: vec![Related {
                                span: later_span[0] as usize..later_span[1] as usize,
                                message: String::new(),
                            }],
                        });
                    }
                }
            }
        }
        if self.uses_complete {
            self.writes.sort_unstable();
            self.writes.dedup();
            let mut used = vec![false; self.scopes.bindings.len()];
            if self.mask.contains(RecordMask::USES) {
                for (span, object) in &self.uses {
                    if let ObjectRef::Var {
                        origin: SymbolOrigin::Local { id, .. },
                        ..
                    } = object
                        && self.writes.binary_search(span).is_err()
                        && let Some(slot) = used.get_mut(id.0 as usize)
                    {
                        *slot = true;
                    }
                }
            } else {
                for &(span, id) in &self.local_var_uses {
                    if self.writes.binary_search(&span).is_err()
                        && let Some(slot) = used.get_mut(id.0 as usize)
                    {
                        *slot = true;
                    }
                }
            }
            // All implicit case bindings share the guard's source selection.
            // Reading the variable in any case satisfies the declaration.
            let mut switches = HashMap::new();
            for (index, binding) in self.scopes.bindings.iter().enumerate() {
                if binding.kind == LocalBindingKind::TypeSwitchVar {
                    *switches.entry(binding.name_span).or_insert(false) |= used[index];
                }
            }
            for index in 0..self.scopes.bindings.len() {
                let binding = self.scopes.bindings[index];
                let switch_case = binding.kind == LocalBindingKind::TypeSwitchVar;
                if used[index]
                    || duplicates[index]
                    || !matches!(
                        binding.kind,
                        LocalBindingKind::Var
                            | LocalBindingKind::ShortVar
                            | LocalBindingKind::RangeKey
                            | LocalBindingKind::RangeVal
                            | LocalBindingKind::TypeSwitchVar
                    )
                    || switch_case && switches.remove(&binding.name_span) != Some(false)
                {
                    continue;
                }
                let span = binding.name_span;
                let Some(name) = source.get(span[0] as usize..span[1] as usize) else {
                    continue;
                };
                if self.diagnostics.len() >= MAX_REGION_DIAGNOSTICS {
                    break;
                }
                if name.len() > 1024 {
                    continue;
                }
                self.push_diagnostic(Diagnostic {
                    span: span[0] as usize..span[1] as usize,
                    kind: Kind::Compiler(Code::UnusedVar),
                    message: if switch_case {
                        format!("{name} declared and not used")
                    } else {
                        format!("declared and not used: {name}")
                    },
                    unnecessary: true,
                    related: Vec::new(),
                });
            }
        }
        self.local_var_uses = Vec::new();
        if !self.mask.contains(RecordMask::WRITES) {
            self.writes = Vec::new();
        }
    }
}
