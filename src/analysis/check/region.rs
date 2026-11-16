//! Compact per-region semantic snapshot aligned with `go/types::Info`.

use crate::analysis::obj::{DeclId, LocalId, ObjectRef};
use crate::analysis::scope::tree::ScopeTree;
use crate::analysis::types::{Operand, Selection, TypeId};

#[derive(Clone, Debug)]
pub(crate) struct CallSiteInfo<'a> {
    pub(crate) span: [u32; 2],
    pub(crate) callee_span: [u32; 2],
    pub(crate) callee_ident_span: Option<[u32; 2]>,
    pub(crate) open_paren: u32,
    pub(crate) arg_spans: Vec<[u32; 2]>,
    pub(crate) callee_obj: Option<ObjectRef<'a>>,
    pub(crate) signature: Option<TypeId>,
    pub(crate) type_args: Box<[TypeId]>,
    pub(crate) is_conversion: bool,
    pub(crate) is_const_builtin: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct InstanceInfo<'a> {
    pub(crate) target: ObjectRef<'a>,
    pub(crate) type_args: Box<[TypeId]>,
    pub(crate) instantiated: TypeId,
}

/// Semantic facts for a single function/method body or top-level declaration spec.
/// Stored as flat sorted tables indexed by `[u32; 2]` file byte offsets so peak
/// memory stays minimal and tables drop immediately after a request/stream step.
pub(crate) struct RegionInfo<'a> {
    pub(crate) owner: Option<DeclId<'a>>,
    pub(crate) span: [u32; 2],
    pub(crate) scopes: ScopeTree,
    pub(crate) local_types: Vec<Option<Operand>>,
    pub(crate) local_inits: Vec<Option<[u32; 2]>>,
    pub(crate) local_annotations: Vec<Option<[u32; 2]>>,
    pub(crate) expr_types: Vec<([u32; 2], Operand)>,
    pub(crate) defs: Vec<([u32; 2], ObjectRef<'a>)>,
    pub(crate) uses: Vec<([u32; 2], ObjectRef<'a>)>,
    pub(crate) implicits: Vec<([u32; 2], ObjectRef<'a>)>,
    pub(crate) selections: Vec<([u32; 2], Selection<'a>)>,
    pub(crate) instances: Vec<([u32; 2], InstanceInfo<'a>)>,
    pub(crate) expected_types: Vec<([u32; 2], TypeId)>,
    pub(crate) calls: Vec<CallSiteInfo<'a>>,
}

impl<'a> RegionInfo<'a> {
    pub(crate) fn new(owner: Option<DeclId<'a>>, span: [u32; 2], scopes: ScopeTree) -> Self {
        let binding_count = scopes.bindings.len();
        Self {
            owner,
            span,
            scopes,
            local_types: vec![None; binding_count],
            local_inits: vec![None; binding_count],
            local_annotations: vec![None; binding_count],
            expr_types: Vec::new(),
            defs: Vec::new(),
            uses: Vec::new(),
            implicits: Vec::new(),
            selections: Vec::new(),
            instances: Vec::new(),
            expected_types: Vec::new(),
            calls: Vec::new(),
        }
    }

    pub(crate) fn finalize(&mut self) {
        sort_and_dedup_last(&mut self.expr_types);
        sort_and_dedup_last(&mut self.defs);
        sort_and_dedup_last(&mut self.uses);
        sort_and_dedup_last(&mut self.implicits);
        sort_and_dedup_last(&mut self.selections);
        sort_and_dedup_last(&mut self.instances);
        sort_and_dedup_last(&mut self.expected_types);
        self.calls
            .sort_by_key(|call| (call.span[0], std::cmp::Reverse(call.span[1])));
    }

    pub(crate) fn local_type(&self, id: LocalId) -> Option<Operand> {
        self.local_types.get(id.0 as usize).copied().flatten()
    }

    pub(crate) fn local_init(&self, id: LocalId) -> Option<[u32; 2]> {
        self.local_inits.get(id.0 as usize).copied().flatten()
    }

    pub(crate) fn local_annotation(&self, id: LocalId) -> Option<[u32; 2]> {
        self.local_annotations.get(id.0 as usize).copied().flatten()
    }

    pub(crate) fn set_local_type(&mut self, id: LocalId, operand: Operand) {
        if let Some(slot) = self.local_types.get_mut(id.0 as usize) {
            *slot = Some(operand);
        }
    }

    pub(crate) fn set_local_init(&mut self, id: LocalId, span: [u32; 2]) {
        if let Some(slot) = self.local_inits.get_mut(id.0 as usize) {
            *slot = Some(span);
        }
    }

    pub(crate) fn set_local_annotation(&mut self, id: LocalId, span: [u32; 2]) {
        if let Some(slot) = self.local_annotations.get_mut(id.0 as usize) {
            *slot = Some(span);
        }
    }

    pub(crate) fn expr_type(&self, span: [u32; 2]) -> Option<Operand> {
        lookup_exact(&self.expr_types, span).copied()
    }

    pub(crate) fn smallest_expr_at(&self, offset: u32) -> Option<([u32; 2], Operand)> {
        smallest_containing(&self.expr_types, offset).map(|(span, op)| (span, *op))
    }

    pub(crate) fn def_at(&self, offset: u32) -> Option<([u32; 2], ObjectRef<'a>)> {
        exact_or_smallest_token(&self.defs, offset).map(|(span, obj)| (span, *obj))
    }

    pub(crate) fn use_at(&self, offset: u32) -> Option<([u32; 2], ObjectRef<'a>)> {
        exact_or_smallest_token(&self.uses, offset).map(|(span, obj)| (span, *obj))
    }

    pub(crate) fn def_by_span(&self, span: [u32; 2]) -> Option<ObjectRef<'a>> {
        lookup_exact(&self.defs, span).copied()
    }

    pub(crate) fn use_by_span(&self, span: [u32; 2]) -> Option<ObjectRef<'a>> {
        lookup_exact(&self.uses, span).copied()
    }

    pub(crate) fn object_at(&self, offset: u32) -> Option<([u32; 2], ObjectRef<'a>)> {
        self.def_at(offset).or_else(|| self.use_at(offset))
    }

    pub(crate) fn selection_by_span(&self, span: [u32; 2]) -> Option<&Selection<'a>> {
        lookup_exact(&self.selections, span)
    }

    pub(crate) fn instance_by_span(&self, span: [u32; 2]) -> Option<&InstanceInfo<'a>> {
        lookup_exact(&self.instances, span)
    }

    pub(crate) fn call_by_callee(&self, callee_span: [u32; 2]) -> Option<&CallSiteInfo<'a>> {
        self.calls
            .iter()
            .find(|call| call.callee_span == callee_span)
    }
}

fn sort_and_dedup_last<T>(entries: &mut Vec<([u32; 2], T)>) {
    // Stable sort preserves insertion order so reversing puts the latest entry first.
    entries.reverse();
    entries.sort_by_key(|(span, _)| *span);
    entries.dedup_by_key(|(span, _)| *span);
}

fn lookup_exact<T>(entries: &[([u32; 2], T)], span: [u32; 2]) -> Option<&T> {
    let index = entries
        .binary_search_by_key(&span, |(entry, _)| *entry)
        .ok()?;
    Some(&entries[index].1)
}

fn smallest_containing<T>(entries: &[([u32; 2], T)], offset: u32) -> Option<([u32; 2], &T)> {
    let limit = entries.partition_point(|(span, _)| span[0] <= offset);
    let mut best: Option<(usize, u32)> = None;
    for (index, (span, _)) in entries[..limit].iter().enumerate() {
        if offset <= span[1] {
            let len = span[1].saturating_sub(span[0]);
            if best.is_none_or(|(_, best_len)| len <= best_len) {
                best = Some((index, len));
            }
        }
    }
    best.map(|(index, _)| (entries[index].0, &entries[index].1))
}

fn exact_or_smallest_token<T>(entries: &[([u32; 2], T)], offset: u32) -> Option<([u32; 2], &T)> {
    let limit = entries.partition_point(|(span, _)| span[0] <= offset);
    let mut best: Option<(usize, u32)> = None;
    for (index, (span, _)) in entries[..limit].iter().enumerate() {
        if offset <= span[1] {
            let len = span[1].saturating_sub(span[0]);
            if best.is_none_or(|(_, best_len)| len < best_len) {
                best = Some((index, len));
            }
        }
    }
    best.map(|(index, _)| (entries[index].0, &entries[index].1))
}
