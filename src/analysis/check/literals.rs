//! Function-literal and composite-literal checking for [`RegionChecker`].

use std::collections::BTreeSet;
use std::ops::Range;

use crate::analysis::engine::check::{ExprOutcome, RegionChecker};
use crate::analysis::engine::types::{Constant, Mode, Operand, PromotionIndices, TypeId};
use crate::syntax::{self, ExprId, ExprKind};

impl<'a, 'b> RegionChecker<'a, 'b> {
    pub(super) fn check_func_literal(
        &mut self,
        signature: &Range<usize>,
        body: &Range<usize>,
    ) -> ExprOutcome<'a, 'b> {
        let abs_sig = self.abs_range(signature);
        let abs_body = self.abs_range(body);
        let sig_slice = self.source.get(abs_sig.clone()).unwrap_or("");
        let parsed_sig = syntax::parse_literal_signature(sig_slice);

        let refs_valid = self.sig_all_refs_valid(sig_slice, abs_sig.start, &parsed_sig);
        let (_, env) = self.visible_type_env(abs_sig.start);
        let env_pairs: Vec<(&str, TypeId)> = env.iter().map(|(n, t)| (n.as_str(), *t)).collect();
        let sig_ty = if refs_valid {
            self.arena
                .signature(
                    self.file,
                    sig_slice,
                    Some(self.file_offset(abs_sig.start)),
                    &parsed_sig,
                    &env_pairs,
                    0,
                )
                .filter(|&ty| self.arena.validate_type(ty).is_some())
        } else {
            None
        };

        if self.closure_scope_present(abs_sig.start as u32, abs_body.end as u32) {
            let body_slice = self.source.get(abs_body.clone()).unwrap_or("");
            let child_tree = syntax::parse_block(body_slice);
            let child_env = self.env.clone();
            let mut child = RegionChecker::with_env(
                self.arena,
                self.decls,
                self.file,
                self.source,
                abs_body.start,
                &child_tree,
                self.region,
                self.iota,
                self.cursor_offset,
                child_env,
            );
            child.populate_closure_signature(sig_slice, abs_sig.start, &parsed_sig);
            child.check_body();
        } else {
            self.region.uses_complete = false;
        }

        match sig_ty {
            Some(ty) => ExprOutcome::Value(Operand {
                mode: Mode::Value,
                ty,
                value: None,
            }),
            None => ExprOutcome::invalid(),
        }
    }

    pub(super) fn check_composite(
        &mut self,
        span: &Range<usize>,
        literal_type: Option<ExprId>,
        elements: &[ExprId],
        expected: Option<TypeId>,
    ) -> ExprOutcome<'a, 'b> {
        let Some(int_bits) = self.int_bits() else {
            return ExprOutcome::invalid();
        };

        if elements.len() > crate::analysis::engine::budget::MAX_NODES {
            self.region.uses_complete = false;
            return ExprOutcome::invalid();
        }
        let mut inferred_array_elem: Option<TypeId> = None;

        let container_ty = if let Some(lt_id) = literal_type {
            let lt_span = self
                .body
                .expression(lt_id)
                .map(|e| e.span.clone())
                .unwrap_or(0..0);
            let lt_text = self.text(&lt_span).trim();
            if let Some(after_dots) = lt_text.strip_prefix("[...]").map(str::trim_start) {
                let elem_offset =
                    self.body_base + lt_span.start + (lt_text.len() - after_dots.len());
                inferred_array_elem = self.resolve_type_text_in(after_dots, elem_offset);
                None
            } else {
                match self.check_expr_outcome(lt_id, None) {
                    ExprOutcome::Value(op) if op.mode == Mode::TypeExpr => Some(op.ty),
                    _ => None,
                }
            }
        } else {
            expected.and_then(|exp_ty| {
                self.arena
                    .elided_composite_element_type(exp_ty)
                    .or(Some(exp_ty))
            })
        };

        if let Some(elem_ty) = inferred_array_elem {
            return self.check_inferred_array_literal(elem_ty, elements, int_bits);
        }

        let Some(target_ty) = container_ty else {
            self.check_unknown_composite_elements(elements);
            return ExprOutcome::invalid();
        };
        self.region.record_literal(self.abs_span(span), target_ty);

        if let Some((key_ty_opt, elem_ty)) = self.arena.composite_container_types(target_ty) {
            return self
                .check_container_literal(target_ty, key_ty_opt, elem_ty, elements, int_bits);
        }

        if self.arena.is_struct(target_ty) == Some(true) {
            return self.check_struct_literal(target_ty, elements, int_bits);
        }

        self.check_unknown_composite_elements(elements);
        ExprOutcome::invalid()
    }

    fn check_inferred_array_literal(
        &mut self,
        elem_ty: TypeId,
        elements: &[ExprId],
        int_bits: u32,
    ) -> ExprOutcome<'a, 'b> {
        let mut next_idx: u64 = 0;
        let mut max_len: u64 = 0;
        let mut seen_indices = BTreeSet::new();
        let mut ok = true;

        for &el_id in elements {
            self.arena.reset_steps();
            let (key_opt, val_id) = match self.body.expression(el_id).map(|e| &e.kind) {
                Some(ExprKind::KeyValue { key, value }) => (Some(*key), *value),
                _ => (None, el_id),
            };
            if let Some(key_id) = key_opt {
                if let Some(key_op) = self.check_expr(key_id, None) {
                    match self.arena.index_value(key_op, int_bits) {
                        Some(Some(idx_val)) => {
                            if !seen_indices.insert(idx_val) {
                                ok = false;
                            }
                            next_idx = idx_val + 1;
                            if next_idx > max_len {
                                max_len = next_idx;
                            }
                        }
                        _ => ok = false,
                    }
                } else {
                    ok = false;
                }
            } else {
                if !seen_indices.insert(next_idx) {
                    ok = false;
                }
                next_idx += 1;
                if next_idx > max_len {
                    max_len = next_idx;
                }
            }

            let elem_expected = self
                .arena
                .elided_composite_element_type(elem_ty)
                .map(|_| elem_ty);
            if let Some(val_op) = self.check_expr(val_id, elem_expected.or(Some(elem_ty))) {
                if !self
                    .arena
                    .assignable(val_op, elem_ty, int_bits)
                    .unwrap_or(false)
                {
                    ok = false;
                }
            } else {
                ok = false;
            }
        }

        if !ok {
            return ExprOutcome::invalid();
        }
        self.arena
            .array(elem_ty, max_len)
            .map(|ty| {
                ExprOutcome::Value(Operand {
                    mode: Mode::Value,
                    ty,
                    value: None,
                })
            })
            .unwrap_or(ExprOutcome::invalid())
    }

    fn check_container_literal(
        &mut self,
        target_ty: TypeId,
        key_ty_opt: Option<TypeId>,
        elem_ty: TypeId,
        elements: &[ExprId],
        int_bits: u32,
    ) -> ExprOutcome<'a, 'b> {
        let mut ok = true;
        if let Some(key_ty) = key_ty_opt {
            if self.arena.constructor_map_keys(target_ty).is_none() {
                ok = false;
            }
            let mut seen_int_keys = BTreeSet::new();
            for &el_id in elements {
                self.arena.reset_steps();
                let (key_opt, val_id) = match self.body.expression(el_id).map(|e| &e.kind) {
                    Some(ExprKind::KeyValue { key, value }) => (Some(*key), *value),
                    _ => (None, el_id),
                };
                let Some(key_id) = key_opt else {
                    let _ = self.check_expr_outcome(el_id, Some(elem_ty));
                    ok = false;
                    continue;
                };
                if let Some(key_op) = self.check_expr(key_id, Some(key_ty)) {
                    if !self
                        .arena
                        .assignable(key_op, key_ty, int_bits)
                        .unwrap_or(false)
                    {
                        ok = false;
                    } else {
                        self.update_expr_type(key_id, key_ty, int_bits);
                    }
                    if let Some(Constant::Int(iv)) = key_op.value
                        && !seen_int_keys.insert(iv)
                    {
                        ok = false;
                    }
                } else {
                    ok = false;
                }
                if let Some(val_op) = self.check_expr(val_id, Some(elem_ty)) {
                    if !self
                        .arena
                        .assignable(val_op, elem_ty, int_bits)
                        .unwrap_or(false)
                    {
                        ok = false;
                    } else {
                        self.update_expr_type(val_id, elem_ty, int_bits);
                    }
                } else {
                    ok = false;
                }
            }
        } else {
            let array_len = if self.arena.is_array(target_ty) == Some(true) {
                self.arena
                    .builtin_length(
                        Operand {
                            mode: Mode::Value,
                            ty: target_ty,
                            value: None,
                        },
                        false,
                        false,
                        int_bits,
                    )
                    .and_then(|op| match op.value {
                        Some(Constant::Int(v)) if v >= 0 => u64::try_from(v).ok(),
                        _ => None,
                    })
            } else {
                None
            };
            let mut next_idx: u64 = 0;
            let mut seen_indices = BTreeSet::new();
            for &el_id in elements {
                self.arena.reset_steps();
                let (key_opt, val_id) = match self.body.expression(el_id).map(|e| &e.kind) {
                    Some(ExprKind::KeyValue { key, value }) => (Some(*key), *value),
                    _ => (None, el_id),
                };
                if let Some(key_id) = key_opt {
                    if let Some(key_op) = self.check_expr(key_id, None) {
                        match self.arena.index_value(key_op, int_bits) {
                            Some(Some(idx_val)) => {
                                if !seen_indices.insert(idx_val) {
                                    ok = false;
                                }
                                if array_len.is_some_and(|len| idx_val >= len) {
                                    ok = false;
                                }
                                next_idx = idx_val + 1;
                            }
                            _ => ok = false,
                        }
                    } else {
                        ok = false;
                    }
                } else {
                    if !seen_indices.insert(next_idx) {
                        ok = false;
                    }
                    if array_len.is_some_and(|len| next_idx >= len) {
                        ok = false;
                    }
                    next_idx += 1;
                }
                if let Some(val_op) = self.check_expr(val_id, Some(elem_ty)) {
                    if !self
                        .arena
                        .assignable(val_op, elem_ty, int_bits)
                        .unwrap_or(false)
                    {
                        ok = false;
                    } else {
                        self.update_expr_type(val_id, elem_ty, int_bits);
                    }
                } else {
                    ok = false;
                }
            }
        }

        if !ok {
            return ExprOutcome::invalid();
        }
        ExprOutcome::Value(Operand {
            mode: Mode::Value,
            ty: target_ty,
            value: None,
        })
    }

    fn check_struct_literal(
        &mut self,
        target_ty: TypeId,
        elements: &[ExprId],
        int_bits: u32,
    ) -> ExprOutcome<'a, 'b> {
        let mut pos_fields: Option<Vec<TypeId>> = None;
        let mut pos_fields_loaded = false;
        let mut has_keyed = false;
        let mut has_unkeyed = false;
        let mut seen_fields = BTreeSet::new();
        let mut seen_paths: Vec<PromotionIndices> = Vec::new();
        let mut ok = true;

        for (idx, &el_id) in elements.iter().enumerate() {
            self.arena.reset_steps();
            let (key_opt, val_id) = match self.body.expression(el_id).map(|e| &e.kind) {
                Some(ExprKind::KeyValue { key, value }) => (Some(*key), *value),
                _ => (None, el_id),
            };
            if let Some(key_id) = key_opt {
                has_keyed = true;
                let key_span = self
                    .body
                    .expression(key_id)
                    .filter(|e| matches!(e.kind, ExprKind::Name))
                    .map(|e| e.span.clone());
                let Some(key_span) = key_span else {
                    let _ = self.check_expr_outcome(key_id, None);
                    let _ = self.check_expr_outcome(val_id, None);
                    ok = false;
                    continue;
                };
                let fname = self.text(&key_span);
                if !seen_fields.insert(fname.to_string()) {
                    ok = false;
                }
                if let Some(selection) = self.arena.literal_field(target_ty, self.file, fname) {
                    let abs_key = self.abs_span(&key_span);
                    let fty = selection.ty;
                    if seen_paths
                        .iter()
                        .any(|path| path.conflicts(&selection.indices))
                    {
                        ok = false;
                    }
                    seen_paths.push(selection.indices.clone());
                    self.record_use(abs_key, selection.obj);
                    self.record_selection(abs_key, selection);
                    if let Some(val_op) = self.check_expr(val_id, Some(fty)) {
                        if !self
                            .arena
                            .assignable(val_op, fty, int_bits)
                            .unwrap_or(false)
                        {
                            ok = false;
                        } else {
                            self.update_expr_type(val_id, fty, int_bits);
                        }
                    } else {
                        ok = false;
                    }
                } else {
                    let _ = self.check_expr_outcome(val_id, None);
                    ok = false;
                }
            } else {
                has_unkeyed = true;
                if !pos_fields_loaded {
                    pos_fields = self.arena.struct_positional_fields(target_ty, self.file);
                    pos_fields_loaded = true;
                }
                let expected_field = pos_fields.as_ref().and_then(|pf| pf.get(idx).copied());
                if expected_field.is_none() {
                    ok = false;
                }
                if let Some(val_op) = self.check_expr(val_id, expected_field) {
                    if let Some(fty) = expected_field {
                        if !self
                            .arena
                            .assignable(val_op, fty, int_bits)
                            .unwrap_or(false)
                        {
                            ok = false;
                        } else {
                            self.update_expr_type(val_id, fty, int_bits);
                        }
                    }
                } else {
                    ok = false;
                }
            }
        }
        if has_unkeyed
            && (has_keyed
                || pos_fields
                    .as_ref()
                    .is_none_or(|pf| elements.len() != pf.len()))
        {
            ok = false;
        }
        if !ok {
            return ExprOutcome::invalid();
        }
        ExprOutcome::Value(Operand {
            mode: Mode::Value,
            ty: target_ty,
            value: None,
        })
    }

    fn check_unknown_composite_elements(&mut self, elements: &[ExprId]) {
        for &el_id in elements {
            self.arena.reset_steps();
            let target = match self.body.expression(el_id).map(|e| &e.kind) {
                Some(ExprKind::KeyValue { key, value }) => {
                    if let Some(key_expr) = self.body.expression(*key) {
                        if matches!(key_expr.kind, ExprKind::Name) {
                            // Without the container type a bare key could be a
                            // struct field or a read of a local map key.
                            self.region.uses_complete &= self
                                .region
                                .scopes
                                .resolve_at(
                                    self.source,
                                    self.abs_span(&key_expr.span)[0],
                                    self.text(&key_expr.span),
                                )
                                .is_none();
                        } else {
                            let _ = self.check_expr_outcome(*key, None);
                        }
                    }
                    *value
                }
                _ => el_id,
            };
            let _ = self.check_expr_outcome(target, None);
        }
    }
}
