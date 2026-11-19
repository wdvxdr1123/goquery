//! Compact per-region semantic snapshot aligned with `go/types::Info`.

use super::diagnostics::Diagnostic;
use crate::analysis::obj::{DeclId, LocalId, ObjectRef};
use crate::analysis::scope::tree::ScopeTree;
use crate::analysis::types::{Operand, Selection, TypeId};

pub(super) const MAX_REGION_DIAGNOSTICS: usize = 128;
const MAX_REGION_DIAGNOSTIC_BYTES: usize = 16 * 1024;

#[derive(Clone, Debug)]
pub(crate) struct CallSiteInfo<'a> {
    pub(crate) span: [u32; 2],
    pub(crate) callee_span: [u32; 2],
    pub(crate) callee_ident_span: Option<[u32; 2]>,
    pub(crate) open_paren: u32,
    pub(crate) arg_spans: Vec<[u32; 2]>,
    pub(crate) callee_obj: Option<ObjectRef<'a>>,
    pub(crate) signature: Option<TypeId>,
    pub(crate) is_conversion: bool,
    pub(crate) is_const_builtin: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct InstanceInfo<'a> {
    pub(crate) target: ObjectRef<'a>,
    /// Preserve the complete Info.Instances fact independently of presentation.
    #[allow(dead_code)]
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
    pub(crate) expr_types: Vec<([u32; 2], Operand)>,
    pub(crate) defs: Vec<([u32; 2], ObjectRef<'a>)>,
    pub(crate) uses: Vec<([u32; 2], ObjectRef<'a>)>,
    /// Bare identifiers assigned without reading their previous value.
    pub(super) writes: Vec<[u32; 2]>,
    /// False when recovery, cursor filtering, or budgets can omit reads.
    /// An unknown operand type alone does not affect this.
    pub(super) uses_complete: bool,
    pub(crate) implicits: Vec<([u32; 2], ObjectRef<'a>)>,
    pub(crate) selections: Vec<([u32; 2], Selection<'a>)>,
    pub(crate) instances: Vec<([u32; 2], InstanceInfo<'a>)>,
    pub(crate) expected_types: Vec<([u32; 2], TypeId)>,
    expected_overflow: bool,
    literal_types: Vec<([u32; 2], TypeId)>,
    literal_overflow: bool,
    pub(crate) calls: Vec<CallSiteInfo<'a>>,
    pub(crate) diagnostics: Vec<Diagnostic>,
    diagnostic_bytes: usize,
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
            expr_types: Vec::new(),
            defs: Vec::new(),
            uses: Vec::new(),
            writes: Vec::new(),
            uses_complete: true,
            implicits: Vec::new(),
            selections: Vec::new(),
            instances: Vec::new(),
            expected_types: Vec::new(),
            expected_overflow: false,
            literal_types: Vec::new(),
            literal_overflow: false,
            calls: Vec::new(),
            diagnostics: Vec::new(),
            diagnostic_bytes: 0,
        }
    }

    pub(crate) fn finalize(&mut self) {
        self.diagnostics
            .sort_by_key(|diagnostic| diagnostic.span.start);
        sort_and_dedup_last(&mut self.expr_types);
        sort_and_dedup_last(&mut self.defs);
        sort_and_dedup_last(&mut self.uses);
        sort_and_dedup_last(&mut self.implicits);
        sort_and_dedup_last(&mut self.selections);
        sort_and_dedup_last(&mut self.instances);
        sort_and_dedup_last(&mut self.expected_types);
        sort_and_dedup_last(&mut self.literal_types);
        self.calls
            .sort_by_key(|call| (call.span[0], std::cmp::Reverse(call.span[1])));
    }

    pub(crate) fn local_type(&self, id: LocalId) -> Option<Operand> {
        self.local_types.get(id.0 as usize).copied().flatten()
    }

    pub(super) fn record_expected(&mut self, span: [u32; 2], ty: Option<TypeId>) {
        record_context(
            &mut self.expected_types,
            &mut self.expected_overflow,
            span,
            ty.unwrap_or(TypeId::INVALID),
        );
    }

    pub(super) fn record_literal(&mut self, span: [u32; 2], ty: TypeId) {
        record_context(
            &mut self.literal_types,
            &mut self.literal_overflow,
            span,
            ty,
        );
    }

    pub(crate) fn expected_type_at(&self, offset: u32) -> Option<TypeId> {
        let (_, ty) = self
            .expected_types
            .iter()
            .filter(|(span, _)| span[0] <= offset && offset <= span[1])
            .min_by_key(|(span, _)| span[1].saturating_sub(span[0]))?;
        (*ty != TypeId::INVALID).then_some(*ty)
    }

    pub(crate) fn literal_type(&self, span: [u32; 2]) -> Option<TypeId> {
        self.literal_types
            .iter()
            .find(|(found, _)| *found == span)
            .map(|(_, ty)| *ty)
    }

    pub(super) fn push_diagnostic(&mut self, diagnostic: Diagnostic) {
        if self.diagnostics.iter().any(|existing| {
            existing.span == diagnostic.span
                && existing.kind == diagnostic.kind
                && existing.message == diagnostic.message
        }) {
            return;
        }
        let bytes = diagnostic
            .related
            .iter()
            .fold(diagnostic.message.len(), |bytes, related| {
                bytes.saturating_add(related.message.len())
            });
        if self.diagnostics.len() >= MAX_REGION_DIAGNOSTICS
            || self.diagnostic_bytes.saturating_add(bytes) > MAX_REGION_DIAGNOSTIC_BYTES
        {
            return;
        }
        self.diagnostic_bytes += bytes;
        self.diagnostics.push(diagnostic);
    }

    pub(crate) fn local_init(&self, id: LocalId) -> Option<[u32; 2]> {
        self.local_inits.get(id.0 as usize).copied().flatten()
    }

    pub(crate) fn set_local_type(&mut self, id: LocalId, operand: Operand) {
        if let Some(slot) = self.local_types.get_mut(id.0 as usize) {
            *slot = Some(operand);
        }
    }

    pub(super) fn extend_binding_storage(&mut self) {
        let length = self.scopes.bindings.len();
        self.local_types.resize(length, None);
        self.local_inits.resize(length, None);
    }

    pub(crate) fn set_local_init(&mut self, id: LocalId, span: [u32; 2]) {
        if let Some(slot) = self.local_inits.get_mut(id.0 as usize) {
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

fn record_context(
    entries: &mut Vec<([u32; 2], TypeId)>,
    overflow: &mut bool,
    span: [u32; 2],
    ty: TypeId,
) {
    if *overflow {
        return;
    }
    if let Some((last, previous)) = entries.last_mut()
        && *last == span
    {
        *previous = ty;
        return;
    }
    if entries.len() >= crate::analysis::budget::MAX_NODES {
        entries.clear();
        *overflow = true;
        return;
    }
    entries.push((span, ty));
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
