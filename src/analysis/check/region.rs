//! Compact per-region semantic snapshot aligned with `go/types::Info`.

use super::diagnostics::Diagnostic;
use crate::analysis::engine::obj::{DeclId, LocalId, ObjectRef};
use crate::analysis::engine::scope::tree::ScopeTree;
use crate::analysis::engine::types::{Operand, Selection, TypeId};

pub(super) const MAX_REGION_DIAGNOSTICS: usize = 128;
const MAX_REGION_DIAGNOSTIC_BYTES: usize = 16 * 1024;

/// Opt-in bitmask controlling which semantic tables [`RegionChecker`] populates
/// in [`RegionInfo`], mirroring `go/types.Info` nil-map skipping.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RecordMask(u16);

impl RecordMask {
    pub(crate) const DEFS: Self = Self(1 << 0);
    pub(crate) const USES: Self = Self(1 << 1);
    pub(crate) const IMPLICITS: Self = Self(1 << 2);
    pub(crate) const TYPES: Self = Self(1 << 3);
    pub(crate) const SELECTIONS: Self = Self(1 << 4);
    pub(crate) const INSTANCES: Self = Self(1 << 5);
    pub(crate) const WRITES: Self = Self(1 << 6);
    pub(crate) const EXPECTED: Self = Self(1 << 7);
    pub(crate) const LITERALS: Self = Self(1 << 8);
    pub(crate) const CALLS: Self = Self(1 << 9);
    pub(crate) const DIAGNOSTICS: Self = Self(1 << 10);
    pub(crate) const ALL: Self = Self((1 << 11) - 1);

    pub(crate) const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    pub(crate) const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub(crate) const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl std::ops::BitOr for RecordMask {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self {
        self.union(rhs)
    }
}

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

/// Classification of statement/expression write sites recorded under [`RecordMask::WRITES`].
/// Kept separate from write-only [`RegionInfo::writes`] so `UnusedVar` diagnostics
/// preserve exact read-vs-write semantics (`x++` and `&x` count as uses; `x = 1` does not).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WriteKind {
    /// Write-only assignment target (`x = 1`, `for x = range s`, short-var re-assignment).
    Assign,
    /// Read-modify-write or channel send target (`x++`, `x--`, `ch <- v`, `(*p)++`).
    Modify,
    /// Address-of operand (`&x`, `&s.f`).
    AddrOf,
}

#[derive(Clone, Copy, Debug)]
struct SpanBounds {
    max_len: u32,
    max_end: u32,
}

impl SpanBounds {
    const UNBOUNDED: Self = Self {
        max_len: u32::MAX,
        max_end: u32::MAX,
    };

    fn of<T>(entries: &[([u32; 2], T)]) -> Self {
        let mut max_len = 0u32;
        let mut max_end = 0u32;
        for &(span, _) in entries {
            max_len = max_len.max(span[1].saturating_sub(span[0]));
            max_end = max_end.max(span[1]);
        }
        Self { max_len, max_end }
    }
}

/// Semantic facts for a single function/method body or top-level declaration spec.
/// Stored as flat sorted tables indexed by `[u32; 2]` file byte offsets so peak
/// memory stays minimal and tables drop immediately after a request/stream step.
pub(crate) struct RegionInfo<'a> {
    pub(crate) owner: Option<DeclId<'a>>,
    pub(crate) span: [u32; 2],
    pub(crate) mask: RecordMask,
    pub(crate) scopes: ScopeTree,
    pub(crate) local_types: Vec<Option<Operand>>,
    pub(crate) local_inits: Vec<Option<[u32; 2]>>,
    pub(crate) expr_types: Vec<([u32; 2], Operand)>,
    expr_bounds: SpanBounds,
    pub(crate) defs: Vec<([u32; 2], ObjectRef<'a>)>,
    def_bounds: SpanBounds,
    pub(crate) uses: Vec<([u32; 2], ObjectRef<'a>)>,
    use_bounds: SpanBounds,
    /// Lightweight local-variable use tracking for `UnusedVar` diagnostics when
    /// `RecordMask::DIAGNOSTICS` is requested without `RecordMask::USES`.
    pub(super) local_var_uses: Vec<([u32; 2], LocalId)>,
    /// Bare identifiers assigned without reading their previous value.
    pub(crate) writes: Vec<[u32; 2]>,
    /// Classified write/mutation sites gated solely by [`RecordMask::WRITES`].
    pub(crate) write_sites: Vec<([u32; 2], WriteKind)>,
    /// False when recovery, cursor filtering, or budgets can omit reads.
    /// An unknown operand type alone does not affect this.
    pub(super) uses_complete: bool,
    pub(crate) implicits: Vec<([u32; 2], ObjectRef<'a>)>,
    pub(crate) selections: Vec<([u32; 2], Selection<'a>)>,
    pub(crate) instances: Vec<([u32; 2], InstanceInfo<'a>)>,
    pub(crate) expected_types: Vec<([u32; 2], TypeId)>,
    expected_bounds: SpanBounds,
    expected_overflow: bool,
    literal_types: Vec<([u32; 2], TypeId)>,
    literal_overflow: bool,
    pub(crate) calls: Vec<CallSiteInfo<'a>>,
    pub(crate) diagnostics: Vec<Diagnostic>,
    diagnostic_bytes: usize,
}

impl<'a> RegionInfo<'a> {
    pub(crate) fn new(owner: Option<DeclId<'a>>, span: [u32; 2], scopes: ScopeTree) -> Self {
        Self::with_mask(owner, span, scopes, RecordMask::ALL)
    }

    pub(crate) fn with_mask(
        owner: Option<DeclId<'a>>,
        span: [u32; 2],
        scopes: ScopeTree,
        mask: RecordMask,
    ) -> Self {
        let binding_count = scopes.bindings.len();
        Self {
            owner,
            span,
            mask,
            scopes,
            local_types: vec![None; binding_count],
            local_inits: vec![None; binding_count],
            expr_types: Vec::new(),
            expr_bounds: SpanBounds::UNBOUNDED,
            defs: Vec::new(),
            def_bounds: SpanBounds::UNBOUNDED,
            uses: Vec::new(),
            use_bounds: SpanBounds::UNBOUNDED,
            local_var_uses: Vec::new(),
            writes: Vec::new(),
            write_sites: Vec::new(),
            uses_complete: true,
            implicits: Vec::new(),
            selections: Vec::new(),
            instances: Vec::new(),
            expected_types: Vec::new(),
            expected_bounds: SpanBounds::UNBOUNDED,
            expected_overflow: false,
            literal_types: Vec::new(),
            literal_overflow: false,
            calls: Vec::new(),
            diagnostics: Vec::new(),
            diagnostic_bytes: 0,
        }
    }

    pub(crate) fn push_def(&mut self, span: [u32; 2], obj: ObjectRef<'a>) {
        if self.mask.contains(RecordMask::DEFS) {
            self.defs.push((span, obj));
        }
    }

    pub(crate) fn finalize(&mut self) {
        self.local_var_uses = Vec::new();
        if !self.mask.contains(RecordMask::CALLS) {
            self.calls.clear();
        }
        if self.mask.is_empty() {
            return;
        }
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
        sort_and_dedup_last(&mut self.write_sites);
        if self.writes.len() > 1 {
            self.writes.sort_unstable();
            self.writes.dedup();
        }
        self.expr_bounds = SpanBounds::of(&self.expr_types);
        self.def_bounds = SpanBounds::of(&self.defs);
        self.use_bounds = SpanBounds::of(&self.uses);
        self.expected_bounds = SpanBounds::of(&self.expected_types);
        self.calls
            .sort_by_key(|call| (call.span[0], std::cmp::Reverse(call.span[1])));
    }

    pub(crate) fn local_type(&self, id: LocalId) -> Option<Operand> {
        self.local_types.get(id.0 as usize).copied().flatten()
    }

    pub(crate) fn local_binding_at(
        &self,
        selection: [u32; 2],
        scope_start: u32,
    ) -> Option<(LocalId, u32)> {
        if let Some(
            ObjectRef::Var {
                origin: crate::analysis::engine::obj::SymbolOrigin::Local { id, .. },
                ..
            }
            | ObjectRef::Const(crate::analysis::engine::obj::SymbolOrigin::Local { id, .. }),
        ) = self.def_by_span(selection)
        {
            return Some((id, selection[0]));
        }
        self.implicits.iter().find_map(|&(cspan, obj)| {
            if cspan[0] <= scope_start
                && scope_start < cspan[1]
                && let ObjectRef::Var {
                    origin: crate::analysis::engine::obj::SymbolOrigin::Local { id, span, .. },
                    ..
                } = obj
                && span == selection
            {
                Some((id, scope_start))
            } else {
                None
            }
        })
    }

    pub(super) fn record_expected(&mut self, span: [u32; 2], ty: Option<TypeId>) {
        if !self.mask.contains(RecordMask::EXPECTED) {
            return;
        }
        record_context(
            &mut self.expected_types,
            &mut self.expected_overflow,
            span,
            ty.unwrap_or(TypeId::INVALID),
        );
    }

    pub(super) fn record_literal(&mut self, span: [u32; 2], ty: TypeId) {
        if !self.mask.contains(RecordMask::LITERALS) {
            return;
        }
        record_context(
            &mut self.literal_types,
            &mut self.literal_overflow,
            span,
            ty,
        );
    }

    pub(crate) fn expected_type_at(&self, offset: u32) -> Option<TypeId> {
        let (_, &ty) = exact_or_smallest_token(&self.expected_types, self.expected_bounds, offset)?;
        (ty != TypeId::INVALID).then_some(ty)
    }

    pub(crate) fn literal_type(&self, span: [u32; 2]) -> Option<TypeId> {
        lookup_exact(&self.literal_types, span).copied()
    }

    pub(super) fn push_diagnostic(&mut self, diagnostic: Diagnostic) {
        if !self.mask.contains(RecordMask::DIAGNOSTICS) {
            return;
        }
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
        smallest_containing(&self.expr_types, self.expr_bounds, offset)
            .map(|(span, op)| (span, *op))
    }

    pub(crate) fn def_at(&self, offset: u32) -> Option<([u32; 2], ObjectRef<'a>)> {
        exact_or_smallest_token(&self.defs, self.def_bounds, offset).map(|(span, obj)| (span, *obj))
    }

    pub(crate) fn use_at(&self, offset: u32) -> Option<([u32; 2], ObjectRef<'a>)> {
        exact_or_smallest_token(&self.uses, self.use_bounds, offset).map(|(span, obj)| (span, *obj))
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

    #[allow(dead_code)]
    pub(crate) fn write_kind_at(&self, span: [u32; 2]) -> Option<WriteKind> {
        lookup_exact(&self.write_sites, span).copied()
    }

    #[allow(dead_code)]
    pub(crate) fn is_write(&self, span: [u32; 2]) -> bool {
        matches!(
            self.write_kind_at(span),
            Some(WriteKind::Assign | WriteKind::Modify)
        ) || self.writes.binary_search(&span).is_ok()
    }

    pub(crate) fn call_by_callee(&self, callee_span: [u32; 2]) -> Option<&CallSiteInfo<'a>> {
        self.calls
            .iter()
            .find(|call| call.callee_span == callee_span)
    }
}

fn sort_and_dedup_last<T>(entries: &mut Vec<([u32; 2], T)>) {
    if entries.len() <= 1 {
        return;
    }
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

fn smallest_containing<T>(
    entries: &[([u32; 2], T)],
    bounds: SpanBounds,
    offset: u32,
) -> Option<([u32; 2], &T)> {
    if entries.is_empty() || offset > bounds.max_end {
        return None;
    }
    let limit = entries.partition_point(|(span, _)| span[0] <= offset);
    let mut best: Option<(usize, u32)> = None;
    for index in (0..limit).rev() {
        let span = entries[index].0;
        let dist = offset - span[0];
        if dist > bounds.max_len {
            break;
        }
        if let Some((_, best_len)) = best
            && dist >= best_len
        {
            break;
        }
        if offset <= span[1] {
            let len = span[1] - span[0];
            if best.is_none_or(|(_, best_len)| len < best_len) {
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
    if entries.len() >= crate::analysis::engine::budget::MAX_NODES {
        entries.clear();
        *overflow = true;
        return;
    }
    entries.push((span, ty));
}

fn exact_or_smallest_token<T>(
    entries: &[([u32; 2], T)],
    bounds: SpanBounds,
    offset: u32,
) -> Option<([u32; 2], &T)> {
    if entries.is_empty() || offset > bounds.max_end {
        return None;
    }
    let limit = entries.partition_point(|(span, _)| span[0] <= offset);
    let mut best: Option<(usize, u32)> = None;
    for index in (0..limit).rev() {
        let span = entries[index].0;
        let dist = offset - span[0];
        if dist > bounds.max_len {
            break;
        }
        if let Some((_, best_len)) = best
            && dist > best_len
        {
            break;
        }
        if offset <= span[1] {
            let len = span[1] - span[0];
            if best.is_none_or(|(_, best_len)| len <= best_len) {
                best = Some((index, len));
            }
        }
    }
    best.map(|(index, _)| (entries[index].0, &entries[index].1))
}

#[cfg(test)]
mod tests {
    use super::{
        RecordMask, RegionInfo, SpanBounds, WriteKind, exact_or_smallest_token,
        smallest_containing, sort_and_dedup_last,
    };
    use crate::analysis::engine::scope::tree::ScopeTree;
    use crate::analysis::engine::types::{BasicKind, Mode, Operand, TypeId};

    fn naive_smallest_containing<T>(
        entries: &[([u32; 2], T)],
        offset: u32,
    ) -> Option<([u32; 2], &T)> {
        let mut best: Option<(usize, u32)> = None;
        for (index, (span, _)) in entries.iter().enumerate() {
            if span[0] <= offset && offset <= span[1] {
                let len = span[1].saturating_sub(span[0]);
                if best.is_none_or(|(_, best_len)| len <= best_len) {
                    best = Some((index, len));
                }
            }
        }
        best.map(|(index, _)| (entries[index].0, &entries[index].1))
    }

    fn naive_exact_or_smallest<T>(
        entries: &[([u32; 2], T)],
        offset: u32,
    ) -> Option<([u32; 2], &T)> {
        let mut best: Option<(usize, u32)> = None;
        for (index, (span, _)) in entries.iter().enumerate() {
            if span[0] <= offset && offset <= span[1] {
                let len = span[1].saturating_sub(span[0]);
                if best.is_none_or(|(_, best_len)| len < best_len) {
                    best = Some((index, len));
                }
            }
        }
        best.map(|(index, _)| (entries[index].0, &entries[index].1))
    }

    #[test]
    fn record_mask_operations() {
        let empty = RecordMask(0);
        assert!(empty.is_empty());
        assert!(!RecordMask::DIAGNOSTICS.is_empty());
        let nav = RecordMask::DEFS | RecordMask::USES | RecordMask::IMPLICITS;
        assert!(nav.contains(RecordMask::DEFS));
        assert!(nav.contains(RecordMask::USES));
        assert!(nav.contains(RecordMask::IMPLICITS));
        assert!(!nav.contains(RecordMask::TYPES));
        assert!(!nav.contains(RecordMask::DIAGNOSTICS));
        assert!(RecordMask::ALL.contains(nav.union(RecordMask::DIAGNOSTICS)));
        assert!(RecordMask::ALL.contains(
            RecordMask::SELECTIONS
                | RecordMask::INSTANCES
                | RecordMask::WRITES
                | RecordMask::EXPECTED
                | RecordMask::LITERALS
                | RecordMask::CALLS
        ));
    }

    #[test]
    fn range_lookups_match_linear_reference_and_tie_breaking() {
        let mut entries: Vec<([u32; 2], u32)> = vec![
            ([0, 0], 1),
            ([0, 10], 2),
            ([10, 20], 3),
            ([10, 50], 4),
            ([12, 18], 5),
            ([0, 1000], 6),
            ([10, 500], 7),
            ([20, 40], 8),
            ([25, 30], 9),
            ([200, 210], 10),
            ([210, 220], 11),
            ([900, 905], 12),
            // Duplicate span: latest value must win after sort_and_dedup_last.
            ([25, 30], 99),
        ];
        sort_and_dedup_last(&mut entries);
        let bounds = SpanBounds::of(&entries);

        // Shared boundary between equal-length adjacent spans [200, 210] and [210, 220]:
        // smallest_containing picks later index ([210, 220]); exact_or_smallest_token picks earlier ([200, 210]).
        assert_eq!(
            smallest_containing(&entries, bounds, 210).map(|(s, &v)| (s, v)),
            Some(([210, 220], 11))
        );
        assert_eq!(
            exact_or_smallest_token(&entries, bounds, 210).map(|(s, &v)| (s, v)),
            Some(([200, 210], 10))
        );
        // Latest entry won for duplicate [25, 30].
        assert_eq!(
            smallest_containing(&entries, bounds, 27).map(|(s, &v)| (s, v)),
            Some(([25, 30], 99))
        );

        // Exhaustively verify every offset including misses before, inside gaps, and beyond max_end.
        for offset in 0..=1050 {
            let expected_sc = naive_smallest_containing(&entries, offset).map(|(s, &v)| (s, v));
            let actual_sc = smallest_containing(&entries, bounds, offset).map(|(s, &v)| (s, v));
            assert_eq!(
                actual_sc, expected_sc,
                "smallest_containing mismatch at {offset}"
            );

            let expected_tok = naive_exact_or_smallest(&entries, offset).map(|(s, &v)| (s, v));
            let actual_tok =
                exact_or_smallest_token(&entries, bounds, offset).map(|(s, &v)| (s, v));
            assert_eq!(
                actual_tok, expected_tok,
                "exact_or_smallest_token mismatch at {offset}"
            );
        }

        // Empty table returns None even at offset 0.
        let empty: Vec<([u32; 2], u32)> = Vec::new();
        let empty_bounds = SpanBounds::of(&empty);
        assert!(smallest_containing(&empty, empty_bounds, 0).is_none());
        assert!(exact_or_smallest_token(&empty, empty_bounds, 0).is_none());
    }

    #[test]
    fn region_info_expected_literal_and_write_lookups() {
        let mut region = RegionInfo::with_mask(
            None,
            [0, 200],
            ScopeTree::empty([0, 200]),
            RecordMask::TYPES | RecordMask::EXPECTED | RecordMask::LITERALS | RecordMask::WRITES,
        );
        let t1 = TypeId::basic(BasicKind::Int);
        let t2 = TypeId::basic(BasicKind::String);

        region.record_expected([10, 50], Some(t1));
        region.record_expected([20, 30], None); // INVALID barrier
        region.record_expected([22, 26], Some(t2));
        region.record_literal([40, 60], t1);
        region.record_literal([5, 8], t2);
        region.expr_types.push((
            [10, 20],
            Operand {
                mode: Mode::Value,
                ty: t1,
                value: None,
            },
        ));
        region.writes.push([70, 75]);
        region.writes.push([30, 35]);
        region.writes.push([30, 35]);
        region.write_sites.push(([30, 35], WriteKind::Assign));
        region.write_sites.push(([80, 84], WriteKind::Modify));
        region.write_sites.push(([90, 94], WriteKind::AddrOf));
        region.finalize();

        assert_eq!(region.expected_type_at(15), Some(t1));
        assert_eq!(region.expected_type_at(21), None);
        assert_eq!(region.expected_type_at(24), Some(t2));
        assert_eq!(region.expected_type_at(199), None);

        assert_eq!(region.literal_type([5, 8]), Some(t2));
        assert_eq!(region.literal_type([40, 60]), Some(t1));
        assert_eq!(region.literal_type([1, 2]), None);

        assert_eq!(region.write_kind_at([30, 35]), Some(WriteKind::Assign));
        assert_eq!(region.write_kind_at([80, 84]), Some(WriteKind::Modify));
        assert_eq!(region.write_kind_at([90, 94]), Some(WriteKind::AddrOf));
        assert!(region.is_write([30, 35]));
        assert!(region.is_write([70, 75]));
        assert!(region.is_write([80, 84]));
        assert!(!region.is_write([90, 94]));
        assert!(!region.is_write([100, 104]));
    }
}
