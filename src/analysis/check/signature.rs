//! Function and closure signature population into `CheckEnv` and `RegionInfo`.

use std::ops::Range;

use crate::analysis::engine::check::RegionChecker;
use crate::analysis::engine::obj::{LocalId, ObjectRef, VarKind};
use crate::analysis::engine::types::{LocalFormal, LocalTypeDefinition, Mode, Operand, TypeId};
use crate::syntax::{self, Signature};

impl<'a, 'b> RegionChecker<'a, 'b> {
    pub(crate) fn populate_signature(&mut self, sig_slice: &str, sig_base: usize, sig: &Signature) {
        let sig_text = |r: &Range<usize>| sig_slice.get(r.start..r.end).unwrap_or("");
        let abs_span = |r: &Range<usize>| [(sig_base + r.start) as u32, (sig_base + r.end) as u32];

        let recv_ann = sig
            .receiver
            .first()
            .map(|p| sig_text(&p.annotation))
            .filter(|s| !s.is_empty());
        self.env.recv_annotation = recv_ann.map(ToString::to_string);

        let mut formals_owned: Vec<(String, usize, Option<String>)> =
            Vec::with_capacity(sig.type_parameters.len());
        let mut implicit_recv_tps: Vec<(usize, Range<usize>)> = Vec::new();
        for tp in &sig.type_parameters {
            let Some(name_range) = &tp.name else {
                continue;
            };
            let name = sig_text(name_range);
            if name.is_empty() || name == "_" {
                continue;
            }
            let bound = (!tp.annotation.is_empty()).then(|| sig_text(&tp.annotation).to_string());
            formals_owned.push((name.to_string(), sig_base + name_range.start, bound));
        }
        if sig.type_parameters.is_empty()
            && let Some(recv) = sig.receiver.first()
        {
            let recv_params =
                crate::analysis::engine::scope::receiver_parameters(sig_slice, &recv.annotation);
            for (idx, r) in recv_params.into_iter().enumerate() {
                let name = sig_text(&r);
                if name.is_empty()
                    || name == "_"
                    || formals_owned
                        .iter()
                        .any(|(existing, _, _)| existing == name)
                {
                    continue;
                }
                formals_owned.push((name.to_string(), sig_base + r.start, None));
                implicit_recv_tps.push((idx, r));
            }
        }
        self.env.type_formals = formals_owned.clone();

        let formals: Vec<LocalFormal<'_>> = formals_owned
            .iter()
            .map(|(name, selection, bound)| LocalFormal {
                name: name.as_str(),
                selection: *selection,
                bound: bound.as_deref(),
            })
            .collect();

        let mut all_constraints_valid = sig.diagnostics.is_empty();
        for tp in &sig.type_parameters {
            let is_blank = tp.name.as_ref().is_none_or(|nr| {
                let n = sig_text(nr);
                n.is_empty() || n == "_"
            });
            if is_blank {
                continue;
            }
            if tp.annotation.is_empty()
                || !self.sig_constraint_refs_valid(sig_slice, sig_base, &tp.annotation)
            {
                all_constraints_valid = false;
            }
        }

        for (idx, tp) in sig.type_parameters.iter().enumerate() {
            let Some(name_range) = &tp.name else {
                continue;
            };
            let span = abs_span(name_range);
            let name = sig_text(name_range);
            if name == "_" || name.is_empty() {
                continue;
            }
            let tp_ty = if !all_constraints_valid {
                None
            } else {
                self.arena
                    .parse_parameterized(self.file, name, None, &formals, None)
            };
            if let Some(local_id) = self.region.scopes.definition_at(span[0]) {
                self.mark_active(local_id);
                let obj = ObjectRef::TypeParam {
                    owner: self.env.type_param_owner,
                    index: idx as u16,
                    span,
                };
                self.record_def(span, obj);
                if let Some(ty) = tp_ty {
                    self.region.set_local_type(
                        local_id,
                        Operand {
                            mode: Mode::TypeExpr,
                            ty,
                            value: None,
                        },
                    );
                }
            }
            if let Some(ty) = tp_ty {
                self.env.type_param_env.push((name.to_string(), ty));
            }
        }

        let recv_refs_valid = recv_ann.is_some()
            && sig.receiver.first().is_some_and(|recv| {
                self.sig_type_refs_valid(sig_slice, sig_base, &recv.annotation)
            });

        for (idx, r) in implicit_recv_tps {
            let span = abs_span(&r);
            let name = sig_text(&r);
            let tp_ty = if !sig.diagnostics.is_empty() || !recv_refs_valid {
                None
            } else {
                self.arena
                    .parse_parameterized(self.file, name, None, &formals, recv_ann)
            };
            if let Some(local_id) = self.region.scopes.definition_at(span[0]) {
                self.mark_active(local_id);
                let obj = ObjectRef::TypeParam {
                    owner: self.env.type_param_owner,
                    index: idx as u16,
                    span,
                };
                self.record_def(span, obj);
                if let Some(ty) = tp_ty {
                    self.region.set_local_type(
                        local_id,
                        Operand {
                            mode: Mode::TypeExpr,
                            ty,
                            value: None,
                        },
                    );
                }
            }
            if let Some(ty) = tp_ty
                && !self
                    .env
                    .type_param_env
                    .iter()
                    .any(|(existing, _)| existing == name)
            {
                self.env.type_param_env.push((name.to_string(), ty));
            }
        }

        for tr in &sig.type_references {
            if tr.qualifier.is_some() {
                continue;
            }
            let Some(name) = sig_slice.get(tr.name.clone()) else {
                continue;
            };
            let span = abs_span(&tr.name);
            let tp_obj = self
                .region
                .scopes
                .resolve_at(self.source, span[0], name)
                .and_then(|local_id| {
                    let binding = self.region.scopes.bindings.get(local_id.0 as usize)?;
                    (binding.kind
                        == crate::analysis::engine::scope::tree::LocalBindingKind::TypeParam)
                        .then_some(binding.name_span)
                        .and_then(|def_span| {
                            self.region.defs.iter().rev().find_map(|&(dspan, obj)| {
                                (dspan == def_span && matches!(obj, ObjectRef::TypeParam { .. }))
                                    .then_some(obj)
                            })
                        })
                })
                .or_else(|| {
                    self.region.defs.iter().rev().find_map(|&(dspan, obj)| {
                        (matches!(obj, ObjectRef::TypeParam { .. })
                            && self.source.get(dspan[0] as usize..dspan[1] as usize) == Some(name))
                        .then_some(obj)
                    })
                });
            if let Some(obj @ ObjectRef::TypeParam { span: def_span, .. }) = tp_obj
                && def_span != span
            {
                self.record_use(span, obj);
            }
        }

        let mut pending_active: Vec<LocalId> = Vec::new();
        for recv in &sig.receiver {
            let recv_ty = if let Some(rt) = recv_ann {
                if !recv_refs_valid {
                    None
                } else {
                    self.arena
                        .parse_parameterized(
                            self.file,
                            rt,
                            Some(self.file_offset(sig_base + recv.annotation.start)),
                            &formals,
                            Some(rt),
                        )
                        .filter(|&ty| self.arena.validate_type(ty).is_some())
                }
            } else {
                None
            };
            if recv_ty.is_some()
                && let Some(rt) = recv_ann
            {
                self.collect_recv_type_params(rt, &formals);
            }
            if let Some(name_range) = &recv.name {
                let name = sig_text(name_range);
                let span = abs_span(name_range);
                if name != "_"
                    && let Some(local_id) = self.region.scopes.definition_at(span[0])
                {
                    self.record_local_var_entry(local_id, span, VarKind::Receiver, recv_ty);
                    pending_active.push(local_id);
                }
            }
        }

        for param in &sig.parameters {
            let ann = sig_text(&param.annotation);
            let p_ty = if ann.is_empty()
                || !self.sig_type_refs_valid(sig_slice, sig_base, &param.annotation)
            {
                None
            } else if param.variadic {
                let elem_text = ann.trim_start_matches("...").trim();
                self.arena
                    .parse_parameterized(
                        self.file,
                        elem_text,
                        Some(self.file_offset(sig_base + param.annotation.end - elem_text.len())),
                        &formals,
                        recv_ann,
                    )
                    .filter(|&ty| self.arena.validate_type(ty).is_some())
                    .and_then(|elem| self.arena.slice_type(elem))
            } else {
                self.arena
                    .parse_parameterized(
                        self.file,
                        ann,
                        Some(self.file_offset(sig_base + param.annotation.start)),
                        &formals,
                        recv_ann,
                    )
                    .filter(|&ty| self.arena.validate_type(ty).is_some())
            };
            if let Some(name_range) = &param.name {
                let name = sig_text(name_range);
                let span = abs_span(name_range);
                if name != "_"
                    && let Some(local_id) = self.region.scopes.definition_at(span[0])
                {
                    self.record_local_var_entry(local_id, span, VarKind::Param, p_ty);
                    pending_active.push(local_id);
                }
            }
        }

        self.env.result_types.clear();
        self.env.named_results = sig
            .results
            .first()
            .is_some_and(|result| result.name.is_some());
        for res in &sig.results {
            let ann = sig_text(&res.annotation);
            let r_ty = if ann.is_empty()
                || !self.sig_type_refs_valid(sig_slice, sig_base, &res.annotation)
            {
                None
            } else {
                self.arena
                    .parse_parameterized(
                        self.file,
                        ann,
                        Some(self.file_offset(sig_base + res.annotation.start)),
                        &formals,
                        recv_ann,
                    )
                    .filter(|&ty| self.arena.validate_type(ty).is_some())
            };
            self.env.result_types.push(r_ty);
            if let Some(name_range) = &res.name {
                let name = sig_text(name_range);
                let span = abs_span(name_range);
                if name != "_"
                    && let Some(local_id) = self.region.scopes.definition_at(span[0])
                {
                    self.record_local_var_entry(local_id, span, VarKind::ResultVar, r_ty);
                    pending_active.push(local_id);
                }
            }
        }

        for id in pending_active {
            self.mark_active(id);
        }
    }

    pub(crate) fn populate_closure_signature(
        &mut self,
        sig_slice: &str,
        sig_base: usize,
        sig: &Signature,
    ) {
        let sig_text = |r: &Range<usize>| sig_slice.get(r.start..r.end).unwrap_or("");
        let abs_span = |r: &Range<usize>| [(sig_base + r.start) as u32, (sig_base + r.end) as u32];

        let param_valid: Vec<bool> = sig
            .parameters
            .iter()
            .map(|param| {
                !sig_text(&param.annotation).is_empty()
                    && self.sig_type_refs_valid(sig_slice, sig_base, &param.annotation)
            })
            .collect();
        let result_valid: Vec<bool> = sig
            .results
            .iter()
            .map(|res| {
                !sig_text(&res.annotation).is_empty()
                    && self.sig_type_refs_valid(sig_slice, sig_base, &res.annotation)
            })
            .collect();

        let (visible_defs, extra_env) = self.visible_type_env(sig_base);
        let defs: Vec<LocalTypeDefinition<'_>> = visible_defs
            .iter()
            .map(|entry| LocalTypeDefinition {
                name: entry.name.as_str(),
                selection: self.file_offset(entry.selection.start)
                    ..self.file_offset(entry.selection.end),
                annotation: entry.annotation.as_str(),
                annotation_start: self.file_offset(entry.annotation_start),
                alias: entry.alias,
            })
            .collect();
        let env_pairs: Vec<(&str, TypeId)> =
            extra_env.iter().map(|(n, ty)| (n.as_str(), *ty)).collect();

        let mut pending_active: Vec<LocalId> = Vec::new();
        for (param, refs_ok) in sig.parameters.iter().zip(param_valid) {
            let ann = sig_text(&param.annotation);
            let p_ty = if !refs_ok {
                None
            } else if param.variadic {
                let elem_text = ann.trim_start_matches("...").trim();
                self.arena
                    .declared_scope_type_in(
                        self.file,
                        elem_text,
                        Some(self.file_offset(sig_base + param.annotation.end - elem_text.len())),
                        &defs,
                        &env_pairs,
                    )
                    .filter(|&ty| self.arena.validate_type(ty).is_some())
                    .and_then(|elem| self.arena.slice_type(elem))
            } else {
                self.arena
                    .declared_scope_type_in(
                        self.file,
                        ann,
                        Some(self.file_offset(sig_base + param.annotation.start)),
                        &defs,
                        &env_pairs,
                    )
                    .filter(|&ty| self.arena.validate_type(ty).is_some())
            };
            if let Some(name_range) = &param.name {
                let name = sig_text(name_range);
                let span = abs_span(name_range);
                if name != "_"
                    && let Some(local_id) = self.region.scopes.definition_at(span[0])
                {
                    self.record_local_var_entry(local_id, span, VarKind::Param, p_ty);
                    pending_active.push(local_id);
                }
            }
        }

        self.env.result_types.clear();
        self.env.named_results = sig
            .results
            .first()
            .is_some_and(|result| result.name.is_some());
        for (res, refs_ok) in sig.results.iter().zip(result_valid) {
            let ann = sig_text(&res.annotation);
            let r_ty = if !refs_ok {
                None
            } else {
                self.arena
                    .declared_scope_type_in(
                        self.file,
                        ann,
                        Some(self.file_offset(sig_base + res.annotation.start)),
                        &defs,
                        &env_pairs,
                    )
                    .filter(|&ty| self.arena.validate_type(ty).is_some())
            };
            self.env.result_types.push(r_ty);
            if let Some(name_range) = &res.name {
                let name = sig_text(name_range);
                let span = abs_span(name_range);
                if name != "_"
                    && let Some(local_id) = self.region.scopes.definition_at(span[0])
                {
                    self.record_local_var_entry(local_id, span, VarKind::ResultVar, r_ty);
                    pending_active.push(local_id);
                }
            }
        }

        for id in pending_active {
            self.mark_active(id);
        }
    }

    fn collect_recv_type_params(&mut self, recv_text: &str, formals: &[LocalFormal<'_>]) {
        let parsed_recv = syntax::parse_type(recv_text);
        for tr in &parsed_recv.references {
            if tr.qualifier.is_some() {
                continue;
            }
            let Some(name) = recv_text.get(tr.name.clone()) else {
                continue;
            };
            if let Some(tp_ty) =
                self.arena
                    .parse_parameterized(self.file, name, None, formals, Some(recv_text))
                && self.arena.format_parameter(tp_ty).is_some()
                && !self
                    .env
                    .type_param_env
                    .iter()
                    .any(|(existing, _)| existing == name)
            {
                self.env.type_param_env.push((name.to_string(), tp_ty));
            }
        }
    }
}
