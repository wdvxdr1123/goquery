//! Unified semantic fact recording and bounded `untyped` delayed-typing table.

use std::collections::HashMap;

use crate::analysis::engine::budget::{MAX_DEPTH, MAX_NODES};
use crate::analysis::engine::check::region::{CallSiteInfo, InstanceInfo, RecordMask, WriteKind};
use crate::analysis::engine::check::{ExprOutcome, RegionChecker};
use crate::analysis::engine::obj::{ObjectRef, SymbolOrigin};
use crate::analysis::engine::scope::tree::LocalBindingKind;
use crate::analysis::engine::types::{BasicKind, Mode, Operand, Selection, TypeId};
use crate::syntax::{ExprId, ExprKind};

/// Entry in [`UntypedTable`] mirroring `go/types.exprInfo`.
#[derive(Clone, Copy)]
pub(super) struct UntypedEntry {
    pub(super) span: [u32; 2],
    pub(super) is_lhs_shift: bool,
    pub(super) op: Operand,
}

/// Bounded per-region table of untyped constant/nil/shift expressions awaiting context typing.
pub(super) struct UntypedTable {
    entries: HashMap<usize, UntypedEntry>,
}

impl UntypedTable {
    pub(super) fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    pub(super) fn insert(&mut self, id: ExprId, span: [u32; 2], op: Operand) -> bool {
        let key = id.index();
        if self.entries.len() < MAX_NODES || self.entries.contains_key(&key) {
            let is_lhs_shift = self.entries.get(&key).is_some_and(|e| e.is_lhs_shift);
            self.entries.insert(
                key,
                UntypedEntry {
                    span,
                    is_lhs_shift,
                    op,
                },
            );
            true
        } else {
            false
        }
    }

    pub(super) fn mark_lhs_shift(&mut self, id: ExprId) {
        if let Some(entry) = self.entries.get_mut(&id.index()) {
            entry.is_lhs_shift = true;
        }
    }

    pub(super) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

enum UntypedConversion {
    Converted(Operand),
    Unknown,
    Invalid,
}

impl<'a, 'b> RegionChecker<'a, 'b> {
    pub(super) fn record_def(&mut self, span: [u32; 2], obj: ObjectRef<'a>) {
        if self.record_mask.contains(RecordMask::DEFS) {
            self.region.defs.push((span, obj));
        }
    }

    pub(super) fn record_use(&mut self, span: [u32; 2], obj: ObjectRef<'a>) {
        if self.record_mask.contains(RecordMask::USES) {
            self.region.uses.push((span, obj));
        } else if self.record_mask.contains(RecordMask::DIAGNOSTICS)
            && self.region.uses_complete
            && let ObjectRef::Var {
                origin: SymbolOrigin::Local { id, .. },
                ..
            } = obj
            && self
                .region
                .scopes
                .bindings
                .get(id.0 as usize)
                .is_some_and(|binding| {
                    matches!(
                        binding.kind,
                        LocalBindingKind::Var
                            | LocalBindingKind::ShortVar
                            | LocalBindingKind::RangeKey
                            | LocalBindingKind::RangeVal
                            | LocalBindingKind::TypeSwitchVar
                    )
                })
        {
            self.region.local_var_uses.push((span, id));
        }
    }

    pub(super) fn record_implicit(&mut self, span: [u32; 2], obj: ObjectRef<'a>) {
        if self.record_mask.contains(RecordMask::IMPLICITS) {
            self.region.implicits.push((span, obj));
        }
    }

    pub(super) fn record_selection(&mut self, span: [u32; 2], selection: Selection<'a>) {
        if self.record_mask.contains(RecordMask::SELECTIONS) {
            self.region.selections.push((span, selection));
        }
    }

    pub(super) fn record_instance(&mut self, span: [u32; 2], info: InstanceInfo<'a>) {
        if self.record_mask.contains(RecordMask::INSTANCES) {
            self.region.instances.push((span, info));
        }
    }

    pub(super) fn record_call(&mut self, info: CallSiteInfo<'a>) {
        if self.record_mask.contains(RecordMask::CALLS) {
            self.region.calls.push(info);
        }
    }

    pub(super) fn record_write(&mut self, span: [u32; 2]) {
        if self.record_mask.contains(RecordMask::WRITES) {
            self.region.writes.push(span);
            self.region.write_sites.push((span, WriteKind::Assign));
        } else if self.record_mask.contains(RecordMask::DIAGNOSTICS) && self.region.uses_complete {
            self.region.writes.push(span);
        }
    }

    pub(super) fn record_write_site(&mut self, span: [u32; 2], kind: WriteKind) {
        if self.record_mask.contains(RecordMask::WRITES) {
            self.region.write_sites.push((span, kind));
        }
    }

    /// Record a write/mutation target (for `x++`, `x--`, `ch <- v`, `&x`, `(*p)++`, `s.f++`)
    /// without marking it write-only in `region.writes`, preserving exact `UnusedVar` semantics.
    pub(super) fn record_write_target(&mut self, mut id: ExprId, kind: WriteKind) {
        if !self.record_mask.contains(RecordMask::WRITES) {
            return;
        }
        for _ in 0..MAX_DEPTH {
            let Some(expr) = self.body.expression(id) else {
                return;
            };
            match &expr.kind {
                ExprKind::Name => {
                    let span = self.abs_span(&expr.span);
                    self.record_write_site(span, kind);
                    return;
                }
                ExprKind::Selector { field, .. } => {
                    let span = self.abs_span(field);
                    self.record_write_site(span, kind);
                    return;
                }
                ExprKind::Unary { operator, operand } if self.text(operator) == "*" => {
                    id = *operand;
                }
                ExprKind::Parenthesized { expression } => {
                    id = *expression;
                }
                _ => return,
            }
        }
    }

    pub(super) fn record_type_and_value(&mut self, span: [u32; 2], op: Operand) {
        if self.record_mask.contains(RecordMask::TYPES) {
            push_expr_type(&mut self.region.expr_types, span, op);
        }
    }

    pub(super) fn record_expr_outcome(&mut self, id: ExprId, outcome: &ExprOutcome<'a, 'b>) {
        if !self.record_mask.contains(RecordMask::TYPES) {
            return;
        }
        if let ExprOutcome::Value(op) = outcome
            && let Some(expr) = self.body.expression(id)
        {
            let span = self.abs_span(&expr.span);
            if op.mode.is_value() && self.arena.basic_kind(op.ty).is_some_and(BasicKind::untyped) {
                if !self.untyped.insert(id, span, *op) {
                    self.record_type_and_value(span, *op);
                }
            } else {
                self.record_type_and_value(span, *op);
            }
        }
    }

    /// Update an untyped expression and its homogeneous untyped subexpressions
    /// once a surrounding assignment, declaration, return, shift, or call fixes `target_ty`.
    pub(super) fn update_expr_type(&mut self, id: ExprId, target_ty: TypeId, int_bits: u32) {
        if !self.record_mask.contains(RecordMask::TYPES) {
            return;
        }
        if self
            .arena
            .basic_kind(target_ty)
            .is_some_and(BasicKind::untyped)
        {
            return;
        }
        let _ = self.update_expr_type_inner(id, target_ty, int_bits, 0);
    }

    fn update_expr_type_inner(
        &mut self,
        id: ExprId,
        target_ty: TypeId,
        int_bits: u32,
        depth: usize,
    ) -> bool {
        if depth >= MAX_DEPTH {
            return true;
        }
        let Some(entry) = self.untyped.entries.remove(&id.index()) else {
            return true;
        };
        let Some(expr) = self.body.expression(id) else {
            return true;
        };
        let is_constant = entry.op.mode == Mode::Constant && entry.op.value.is_some();
        let child_ok = match &expr.kind {
            ExprKind::Parenthesized { expression } => {
                let inner = *expression;
                self.update_expr_type_inner(inner, target_ty, int_bits, depth + 1)
            }
            ExprKind::Unary { operator, operand } if !is_constant => {
                let op_text = self.text(operator);
                let inner = *operand;
                if matches!(op_text, "+" | "-" | "^" | "!") {
                    self.update_expr_type_inner(inner, target_ty, int_bits, depth + 1)
                } else {
                    true
                }
            }
            ExprKind::Binary {
                left,
                operator,
                right,
            } if !is_constant => {
                let op_text = self.text(operator);
                let (l, r) = (*left, *right);
                if matches!(op_text, "<<" | ">>") {
                    // Shift result type depends only on LHS; RHS was checked independently.
                    self.update_expr_type_inner(l, target_ty, int_bits, depth + 1)
                } else if matches!(
                    op_text,
                    "+" | "-" | "*" | "/" | "%" | "&" | "|" | "^" | "&^" | "&&" | "||"
                ) {
                    let ok_l = self.update_expr_type_inner(l, target_ty, int_bits, depth + 1);
                    let ok_r = self.update_expr_type_inner(r, target_ty, int_bits, depth + 1);
                    ok_l && ok_r
                } else {
                    true
                }
            }
            _ => true,
        };
        if !child_ok {
            return false;
        }
        match self.convert_untyped_entry(entry, target_ty, int_bits) {
            UntypedConversion::Converted(converted) => {
                self.record_type_and_value(entry.span, converted);
                true
            }
            UntypedConversion::Unknown => {
                self.record_type_and_value(entry.span, entry.op);
                true
            }
            UntypedConversion::Invalid => false,
        }
    }

    fn convert_untyped_entry(
        &mut self,
        entry: UntypedEntry,
        target_ty: TypeId,
        int_bits: u32,
    ) -> UntypedConversion {
        let Some(source_kind) = self.arena.basic_kind(entry.op.ty) else {
            return UntypedConversion::Unknown;
        };
        if !source_kind.untyped() {
            return UntypedConversion::Converted(entry.op);
        }
        if source_kind == BasicKind::UntypedNil {
            return if entry.is_lhs_shift {
                UntypedConversion::Invalid
            } else {
                // Preserve UntypedNil in Info.Types, matching go/types (go.dev/issue/13061).
                UntypedConversion::Converted(entry.op)
            };
        }
        let effective_target = if self.arena.is_interface(target_ty) == Some(true) {
            let defaulted = if entry.is_lhs_shift {
                self.arena.basic(BasicKind::Int)
            } else {
                self.arena.default_operand(entry.op, int_bits)
            };
            let Some(ty) = defaulted else {
                return UntypedConversion::Unknown;
            };
            ty
        } else {
            target_ty
        };
        let Some(target_kind) = self.arena.basic_kind(effective_target) else {
            // Non-basic target (e.g., type parameter or unresolved): preserve
            // three-valued Unknown discipline without inventing or dropping types.
            return UntypedConversion::Unknown;
        };
        if entry.is_lhs_shift && !target_kind.integer() {
            return UntypedConversion::Invalid;
        }
        if entry.op.mode != Mode::Constant {
            let compatible = match source_kind {
                BasicKind::UntypedBool => target_kind.boolean(),
                BasicKind::UntypedInt
                | BasicKind::UntypedRune
                | BasicKind::UntypedFloat
                | BasicKind::UntypedComplex => target_kind.numeric_rank().is_some(),
                BasicKind::UntypedString => target_kind.string(),
                _ => false,
            };
            if !compatible {
                return UntypedConversion::Invalid;
            }
        }
        match self
            .arena
            .assign_basic(entry.op, effective_target, int_bits)
        {
            Some(converted) => UntypedConversion::Converted(converted),
            None => {
                if entry.op.mode == Mode::Constant && entry.op.value.is_none() {
                    UntypedConversion::Unknown
                } else {
                    UntypedConversion::Invalid
                }
            }
        }
    }

    pub(super) fn flush_untyped(&mut self) {
        if !self.record_mask.contains(RecordMask::TYPES) {
            self.untyped.entries.clear();
            return;
        }
        let expr_types = &mut self.region.expr_types;
        for (_, entry) in self.untyped.entries.drain() {
            push_expr_type(expr_types, entry.span, entry.op);
        }
    }
}

fn push_expr_type(expr_types: &mut Vec<([u32; 2], Operand)>, span: [u32; 2], op: Operand) {
    if let Some((last_span, last_op)) = expr_types.last_mut()
        && *last_span == span
    {
        *last_op = op;
    } else {
        expr_types.push((span, op));
    }
}

impl Drop for RegionChecker<'_, '_> {
    fn drop(&mut self) {
        if !self.untyped.is_empty() {
            self.flush_untyped();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use serde_json::json;

    use crate::analysis::engine::check::RegionChecker;
    use crate::analysis::engine::check::decls::DeclTable;
    use crate::analysis::engine::check::diagnostics::{Code, Kind};
    use crate::analysis::engine::check::region::{RecordMask, RegionInfo, WriteKind};
    use crate::analysis::engine::scope::tree::ScopeTree;
    use crate::analysis::engine::types::{BasicKind, Constant, Mode, Operand, TypeArena};
    use crate::syntax::{self, ExprKind};
    use crate::workspace::{self, Workspace};

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    struct TempMod {
        root: PathBuf,
    }

    impl TempMod {
        fn new(name: &str, files: &[(&str, &str)]) -> Self {
            let id = COUNTER.fetch_add(1, Ordering::Relaxed);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root = std::env::temp_dir().join(format!(
                "goquery_record_{name}_{}_{id}_{nanos}",
                std::process::id()
            ));
            std::fs::create_dir_all(&root).unwrap();
            for (rel, content) in files {
                let path = root.join(rel);
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).unwrap();
                }
                std::fs::write(&path, content).unwrap();
            }
            Self { root }
        }

        fn workspace(&self) -> Workspace {
            let mut ws = Workspace::default();
            ws.initialize(&json!({
                "rootUri": workspace::uri(&self.root),
                "initializationOptions": {
                    "goroot": "",
                    "fileWatch": false,
                }
            }))
            .unwrap();
            ws
        }
    }

    impl Drop for TempMod {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn check_func_region<'a>(
        ws: &'a Workspace,
        arena: &mut TypeArena<'a>,
        decls: &mut DeclTable<'a>,
        file: &'a Path,
        source: &str,
        func_name: &str,
        mask: RecordMask,
    ) -> RegionInfo<'a> {
        let stub = ws.files.get(file).unwrap();
        let sym = stub
            .symbols
            .iter()
            .find(|s| s.is_function() && s.name == func_name)
            .unwrap();
        let decl = sym.declaration().unwrap();
        let body_range = sym.body().unwrap();
        let header_slice = &source[decl.start..body_range.start];
        let body_slice = &source[body_range.clone()];
        let sig = syntax::parse_signature(header_slice);
        let tree = syntax::parse_block(body_slice);
        let span = [decl.start as u32, decl.end as u32];
        let scopes = ScopeTree::from_function(
            source,
            decl.start,
            Some(&sig),
            body_range.start,
            &tree,
            None,
        );
        let mut region = RegionInfo::with_mask(None, span, scopes, mask);
        {
            let mut checker = RegionChecker::new(
                arena,
                decls,
                file,
                source,
                body_range.start,
                &tree,
                &mut region,
                None,
                None,
            );
            checker.populate_signature(header_slice, decl.start, &sig);
            checker.check_body();
        }
        if mask.contains(RecordMask::DIAGNOSTICS) {
            region.finish_local_diagnostics(source);
        }
        region.finalize();
        region
    }

    fn span_of(source: &str, needle: &str) -> [u32; 2] {
        let start = source.find(needle).unwrap();
        [start as u32, (start + needle.len()) as u32]
    }

    fn span_of_sub(source: &str, outer: &str, inner: &str) -> [u32; 2] {
        let base = source.find(outer).unwrap();
        let rel = outer.find(inner).unwrap();
        let start = base + rel;
        [start as u32, (start + inner.len()) as u32]
    }

    #[test]
    fn untyped_shift_context_propagation_and_flush() {
        let src = concat!(
            "package p\n",
            "func f(s uint) {\n",
            "\tvar a uint64 = 1 << s\n",
            "\tvar b uint64 = (1 << 2) << s\n",
            "\tvar c uint64 = (1.0) << s\n",
            "\td := 3 << s\n",
            "\tvar e float64 = 4 << s\n",
            "\tvar g uint8 = 256 << s\n",
            "\tvar h uint8 = 1000 - 999\n",
            "\tvar iface any = 42\n",
            "\tvar ptr *int = nil\n",
            "\tvar u uint64 = 0\n",
            "\t_ = u + 7\n",
            "\tconst k = 10 + 20\n",
            "\t_, _, _, _, _, _, _, _, _, _, _ = a, b, c, d, e, g, h, iface, ptr, u, k\n",
            "}\n",
        );
        let dir = TempMod::new(
            "untyped_shift",
            &[
                ("go.mod", "module example.com/p\n\ngo 1.24\n"),
                ("p.go", src),
            ],
        );
        let ws = dir.workspace();
        let file = ws
            .files
            .keys()
            .find(|p| p.ends_with("p.go"))
            .unwrap()
            .as_path();
        let mut arena = TypeArena::with_shared_budget(&ws, ws.build_view(None));
        let mut decls = DeclTable::new();
        let region =
            check_func_region(&ws, &mut arena, &mut decls, file, src, "f", RecordMask::ALL);

        let kind_name =
            |arena: &mut TypeArena<'_>, op: Operand| arena.basic_kind(op.ty).map(BasicKind::name);

        // `var a uint64 = 1 << s` -> both `1 << s` and `1` get uint64.
        let a_shift = region.expr_type(span_of(src, "1 << s")).unwrap();
        assert_eq!(kind_name(&mut arena, a_shift), Some("uint64"));
        let a_lhs = region
            .expr_type(span_of_sub(src, "var a uint64 = 1 << s", "1"))
            .unwrap();
        assert_eq!(kind_name(&mut arena, a_lhs), Some("uint64"));
        assert_eq!(a_lhs.mode, Mode::Constant);
        assert!(matches!(a_lhs.value, Some(Constant::Int(1))));

        // `var b uint64 = (1 << 2) << s` -> `(1 << 2)` and `1 << 2` get uint64 (value 4),
        // while inner constant suboperands `1` and `2` remain untyped int via `flush_untyped`.
        let b_paren = region.expr_type(span_of(src, "(1 << 2)")).unwrap();
        assert_eq!(kind_name(&mut arena, b_paren), Some("uint64"));
        let b_inner_shift = region.expr_type(span_of(src, "1 << 2")).unwrap();
        assert_eq!(kind_name(&mut arena, b_inner_shift), Some("uint64"));
        assert!(matches!(b_inner_shift.value, Some(Constant::Int(4))));
        let b_one = region.expr_type(span_of_sub(src, "(1 << 2)", "1")).unwrap();
        assert_eq!(kind_name(&mut arena, b_one), Some("untyped int"));
        let b_two = region.expr_type(span_of_sub(src, "(1 << 2)", "2")).unwrap();
        assert_eq!(kind_name(&mut arena, b_two), Some("untyped int"));

        // `var c uint64 = (1.0) << s` -> `1.0` converts to uint64 with integer value 1.
        let c_lhs = region.expr_type(span_of(src, "1.0")).unwrap();
        assert_eq!(kind_name(&mut arena, c_lhs), Some("uint64"));
        assert!(matches!(c_lhs.value, Some(Constant::Int(1))));

        // `d := 3 << s` -> defaults to int for both shift and LHS.
        let d_shift = region.expr_type(span_of(src, "3 << s")).unwrap();
        assert_eq!(kind_name(&mut arena, d_shift), Some("int"));
        let d_lhs = region.expr_type(span_of_sub(src, "3 << s", "3")).unwrap();
        assert_eq!(kind_name(&mut arena, d_lhs), Some("int"));

        // `var e float64 = 4 << s` -> invalid non-integer target; `4` must not become float64.
        let e_lhs = region.expr_type(span_of_sub(src, "4 << s", "4")).unwrap();
        assert_ne!(kind_name(&mut arena, e_lhs), Some("float64"));

        // `var g uint8 = 256 << s` -> overflow; `256` must not become uint8.
        let g_lhs = region
            .expr_type(span_of_sub(src, "256 << s", "256"))
            .unwrap();
        assert_ne!(kind_name(&mut arena, g_lhs), Some("uint8"));

        // `var h uint8 = 1000 - 999` -> outer expr gets uint8 (1), inner constants stay untyped int.
        let h_expr = region.expr_type(span_of(src, "1000 - 999")).unwrap();
        assert_eq!(kind_name(&mut arena, h_expr), Some("uint8"));
        let h_left = region.expr_type(span_of(src, "1000")).unwrap();
        assert_eq!(kind_name(&mut arena, h_left), Some("untyped int"));
        let h_right = region.expr_type(span_of(src, "999")).unwrap();
        assert_eq!(kind_name(&mut arena, h_right), Some("untyped int"));

        // `var iface any = 42` -> materializes to default type int.
        let iface_lit = region.expr_type(span_of(src, "42")).unwrap();
        assert_eq!(kind_name(&mut arena, iface_lit), Some("int"));

        // `var ptr *int = nil` -> preserves untyped nil.
        let nil_op = region.expr_type(span_of(src, "nil")).unwrap();
        assert_eq!(kind_name(&mut arena, nil_op), Some("untyped nil"));

        // `_ = u + 7` -> untyped peer updates to u's uint64 type.
        let seven = region.expr_type(span_of_sub(src, "u + 7", "7")).unwrap();
        assert_eq!(kind_name(&mut arena, seven), Some("uint64"));

        // `const k = 10 + 20` -> flushed as untyped int.
        let k_expr = region.expr_type(span_of(src, "10 + 20")).unwrap();
        assert_eq!(kind_name(&mut arena, k_expr), Some("untyped int"));
        let k_left = region.expr_type(span_of_sub(src, "10 + 20", "10")).unwrap();
        assert_eq!(kind_name(&mut arena, k_left), Some("untyped int"));

        // No false-positive diagnostics emitted under three-valued Unknown discipline.
        assert!(region.diagnostics.is_empty());
    }

    #[test]
    fn direct_untyped_shift_update_enforces_integer_and_representability() {
        let dir = TempMod::new(
            "direct_shift",
            &[
                ("go.mod", "module example.com/p\n\ngo 1.24\n"),
                ("p.go", "package p\n"),
            ],
        );
        let ws = dir.workspace();
        let file = ws
            .files
            .keys()
            .find(|p| p.ends_with("p.go"))
            .unwrap()
            .as_path();

        let run_case = |expr_src: &str,
                        target_kind: BasicKind|
         -> (Option<&'static str>, Option<&'static str>) {
            let parsed = syntax::parse_expression(expr_src);
            let root = parsed.root();
            let ExprKind::Binary { left, .. } = parsed.node(root).unwrap().kind else {
                panic!("expected binary shift");
            };
            let shift_span = {
                let s = &parsed.node(root).unwrap().span;
                [s.start as u32, s.end as u32]
            };
            let lhs_span = {
                let s = &parsed.node(left).unwrap().span;
                [s.start as u32, s.end as u32]
            };
            let tree = syntax::BlockTree::from_expression(parsed);
            let scopes = ScopeTree::empty([0, expr_src.len() as u32]);
            let mut arena = TypeArena::with_shared_budget(&ws, ws.build_view(None));
            let mut decls = DeclTable::new();
            let mut region =
                RegionInfo::with_mask(None, [0, expr_src.len() as u32], scopes, RecordMask::TYPES);
            let target_ty = arena.basic(target_kind).unwrap();
            let untyped_int = arena.basic(BasicKind::UntypedInt).unwrap();
            let lhs_val: i128 = expr_src[lhs_span[0] as usize..lhs_span[1] as usize]
                .parse()
                .unwrap();
            {
                let mut checker = RegionChecker::new(
                    &mut arena,
                    &mut decls,
                    file,
                    expr_src,
                    0,
                    &tree,
                    &mut region,
                    None,
                    None,
                );
                checker.untyped.insert(
                    left,
                    lhs_span,
                    Operand {
                        mode: Mode::Constant,
                        ty: untyped_int,
                        value: Some(Constant::Int(lhs_val)),
                    },
                );
                checker.untyped.mark_lhs_shift(left);
                checker.untyped.insert(
                    root,
                    shift_span,
                    Operand {
                        mode: Mode::Value,
                        ty: untyped_int,
                        value: None,
                    },
                );
                checker.update_expr_type(root, target_ty, 64);
                checker.flush_untyped();
            }
            region.finalize();
            (
                region
                    .expr_type(shift_span)
                    .and_then(|o| arena.basic_kind(o.ty).map(BasicKind::name)),
                region
                    .expr_type(lhs_span)
                    .and_then(|o| arena.basic_kind(o.ty).map(BasicKind::name)),
            )
        };

        // Concrete integer target propagates through shift to LHS.
        assert_eq!(
            run_case("1 << s", BasicKind::Uint64),
            (Some("uint64"), Some("uint64"))
        );
        // Non-integer target is rejected by is_lhs_shift on LHS and aborts shift update.
        assert_eq!(run_case("1 << s", BasicKind::Float64), (None, None));
        // Overflowing constant LHS is rejected by representability.
        assert_eq!(run_case("256 << s", BasicKind::Uint8), (None, None));
    }

    #[test]
    fn writes_recording_preserves_unused_var_and_classifies_mutations() {
        let src = concat!(
            "package p\n",
            "type box struct { n int }\n",
            "func f() {\n",
            "\tinc := 1\n",
            "\tinc++\n",
            "\taddr := 1\n",
            "\t_ = &addr\n",
            "\tch := make(chan int)\n",
            "\tch <- 1\n",
            "\tvar ptr *int\n",
            "\t(*ptr)++\n",
            "\tvar st box\n",
            "\tst.n++\n",
            "\tonlyWritten := 1\n",
            "\tonlyWritten = 2\n",
            "}\n",
        );
        let dir = TempMod::new(
            "writes_unused",
            &[
                ("go.mod", "module example.com/p\n\ngo 1.24\n"),
                ("p.go", src),
            ],
        );
        let ws = dir.workspace();
        let file = ws
            .files
            .keys()
            .find(|p| p.ends_with("p.go"))
            .unwrap()
            .as_path();

        let mut arena = TypeArena::with_shared_budget(&ws, ws.build_view(None));
        let mut decls = DeclTable::new();
        let region =
            check_func_region(&ws, &mut arena, &mut decls, file, src, "f", RecordMask::ALL);

        // Only `onlyWritten` should be flagged as UnusedVar.
        let unused: Vec<&str> = region
            .diagnostics
            .iter()
            .filter(|d| d.kind == Kind::Compiler(Code::UnusedVar))
            .map(|d| d.message.as_str())
            .collect();
        assert_eq!(unused, vec!["declared and not used: onlyWritten"]);

        // Classified write sites distinguish Assign, Modify, and AddrOf.
        let inc_span = span_of_sub(src, "inc++", "inc");
        let addr_span = span_of_sub(src, "&addr", "addr");
        let ch_span = span_of_sub(src, "ch <- 1", "ch");
        let ptr_span = span_of_sub(src, "(*ptr)++", "ptr");
        let field_span = span_of_sub(src, "st.n++", "n");
        let assign_span = span_of_sub(src, "onlyWritten = 2", "onlyWritten");

        assert_eq!(region.write_kind_at(inc_span), Some(WriteKind::Modify));
        assert!(region.is_write(inc_span));
        assert_eq!(region.write_kind_at(addr_span), Some(WriteKind::AddrOf));
        assert!(!region.is_write(addr_span));
        assert_eq!(region.write_kind_at(ch_span), Some(WriteKind::Modify));
        assert!(region.is_write(ch_span));
        assert_eq!(region.write_kind_at(ptr_span), Some(WriteKind::Modify));
        assert!(region.is_write(ptr_span));
        assert_eq!(region.write_kind_at(field_span), Some(WriteKind::Modify));
        assert!(region.is_write(field_span));
        assert_eq!(region.write_kind_at(assign_span), Some(WriteKind::Assign));
        assert!(region.is_write(assign_span));

        // DIAGNOSTICS-only skips write_sites allocation while keeping exact UnusedVar diagnostics.
        arena.reset_local();
        let diag_region = check_func_region(
            &ws,
            &mut arena,
            &mut decls,
            file,
            src,
            "f",
            RecordMask::DIAGNOSTICS,
        );
        assert!(diag_region.write_sites.is_empty());
        assert!(diag_region.writes.is_empty());
        let diag_unused: Vec<&str> = diag_region
            .diagnostics
            .iter()
            .filter(|d| d.kind == Kind::Compiler(Code::UnusedVar))
            .map(|d| d.message.as_str())
            .collect();
        assert_eq!(diag_unused, vec!["declared and not used: onlyWritten"]);
    }

    #[test]
    fn record_type_and_value_overwrites_matching_tail_in_place() {
        let dir = TempMod::new(
            "tail_dedup",
            &[
                ("go.mod", "module example.com/p\n\ngo 1.24\n"),
                ("p.go", "package p\n"),
            ],
        );
        let ws = dir.workspace();
        let file = ws
            .files
            .keys()
            .find(|p| p.ends_with("p.go"))
            .unwrap()
            .as_path();
        let mut arena = TypeArena::with_shared_budget(&ws, ws.build_view(None));
        let mut decls = DeclTable::new();
        let tree = syntax::parse_block("{}");
        let mut region =
            RegionInfo::with_mask(None, [0, 2], ScopeTree::empty([0, 2]), RecordMask::TYPES);
        let t_untyped = arena.basic(BasicKind::UntypedInt).unwrap();
        let t_uint64 = arena.basic(BasicKind::Uint64).unwrap();
        {
            let mut checker = RegionChecker::new(
                &mut arena,
                &mut decls,
                file,
                "{}",
                0,
                &tree,
                &mut region,
                None,
                None,
            );
            checker.record_type_and_value(
                [0, 1],
                Operand {
                    mode: Mode::Constant,
                    ty: t_untyped,
                    value: Some(Constant::Int(1)),
                },
            );
            checker.record_type_and_value(
                [0, 1],
                Operand {
                    mode: Mode::Constant,
                    ty: t_uint64,
                    value: Some(Constant::Int(1)),
                },
            );
            assert_eq!(checker.region.expr_types.len(), 1);
            assert_eq!(checker.region.expr_types[0].1.ty, t_uint64);
        }
    }
}
