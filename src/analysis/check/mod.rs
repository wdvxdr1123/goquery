//! Demand-driven semantic checker (`DeclCache`, `RegionInfo`, `RegionChecker`).
//! Intra-body semantic checker for Go functions and declaration initializers.

pub(crate) mod decls;
pub(crate) mod diagnostics;
mod exprs;
pub(crate) mod region;
mod stmts;

use std::collections::BTreeSet;
use std::ops::Range;
use std::path::Path;

use crate::analysis::check::decls::DeclCache;
use crate::analysis::check::region::RegionInfo;
use crate::analysis::obj::{
    BuiltinFunction, DeclId, LocalId, MemberDeclId, MethodOrigin, ObjectRef, SymbolOrigin, VarKind,
};
use crate::analysis::scope::tree::{LocalBinding, LocalBindingKind};
use crate::analysis::types::{
    BasicKind, LocalFormal, LocalTypeDefinition, Mode, Operand, TypeArena, TypeId,
};
use crate::syntax::{self, BlockTree, ExprId, ExprKind, Signature, StmtId, SymbolKind};

/// Operands share one mode model; packages and unapplied generics retain their
/// declaration environments until they can produce a concrete operand.
pub(crate) enum ExprOutcome<'a, 'b> {
    Value(Operand),
    Package {
        alias: &'b str,
    },
    PartialGenericFunc {
        decl: DeclId<'a>,
        header: String,
        parsed_sig: Signature,
        explicit_type_args: Vec<TypeId>,
    },
    GenericType(DeclId<'a>),
}

impl ExprOutcome<'_, '_> {
    fn invalid() -> Self {
        Self::Value(Operand::non_value(Mode::Invalid))
    }

    fn no_value() -> Self {
        Self::Value(Operand::non_value(Mode::NoValue))
    }

    fn builtin(function: BuiltinFunction) -> Self {
        Self::Value(Operand::non_value(Mode::Builtin(function)))
    }
}

#[derive(Clone)]
struct LocalTypeDefEntry {
    local_id: LocalId,
    name: String,
    selection: Range<usize>,
    annotation: String,
    annotation_start: usize,
    alias: bool,
}

pub(crate) struct ClosureSnapshot<'a> {
    source_base: usize,
    active_locals: Vec<bool>,
    type_param_owner: Option<DeclId<'a>>,
    type_param_env: Vec<(String, TypeId)>,
    type_formals: Vec<(String, usize, Option<String>)>,
    recv_annotation: Option<String>,
    local_type_defs: Vec<LocalTypeDefEntry>,
}

/// Checks a single function body or declaration initializer into [`RegionInfo`].
pub(crate) struct RegionChecker<'a, 'b> {
    pub(crate) arena: &'b mut TypeArena<'a>,
    pub(crate) decls: &'b mut DeclCache<'a>,
    pub(crate) file: &'a Path,
    pub(crate) source: &'b str,
    /// File byte origin of a bounded declaration slice; region spans stay slice-local.
    source_base: usize,
    pub(crate) body_base: usize,
    pub(crate) body: &'b BlockTree,
    pub(crate) region: &'b mut RegionInfo<'a>,
    pub(crate) iota: Option<u32>,
    pub(crate) cursor_offset: Option<usize>,
    active_locals: Vec<bool>,
    declaring_type: Option<LocalId>,
    pub(in crate::analysis::check) type_param_owner: Option<DeclId<'a>>,
    pub(in crate::analysis::check) type_param_env: Vec<(String, TypeId)>,
    pub(in crate::analysis::check) type_formals: Vec<(String, usize, Option<String>)>,
    recv_annotation: Option<String>,
    local_type_defs: Vec<LocalTypeDefEntry>,
    result_types: Vec<Option<TypeId>>,
}

impl<'a, 'b> RegionChecker<'a, 'b> {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        arena: &'b mut TypeArena<'a>,
        decls: &'b mut DeclCache<'a>,
        file: &'a Path,
        source: &'b str,
        body_base: usize,
        body: &'b BlockTree,
        region: &'b mut RegionInfo<'a>,
        iota: Option<u32>,
        cursor_offset: Option<usize>,
    ) -> Self {
        let binding_count = region.scopes.bindings.len();
        region.uses_complete &= body.diagnostics().is_empty() && cursor_offset.is_none();
        Self {
            arena,
            decls,
            file,
            source,
            source_base: 0,
            body_base,
            body,
            region,
            iota,
            cursor_offset,
            active_locals: vec![false; binding_count],
            declaring_type: None,
            type_param_owner: None,
            type_param_env: Vec::new(),
            type_formals: Vec::new(),
            recv_annotation: None,
            local_type_defs: Vec::new(),
            result_types: Vec::new(),
        }
    }

    pub(super) fn set_source_base(&mut self, base: usize) {
        self.source_base = base;
    }

    fn file_offset(&self, source_offset: usize) -> usize {
        self.source_base + source_offset
    }

    pub(crate) fn set_type_param_owner(&mut self, owner: DeclId<'a>) {
        self.type_param_owner = Some(owner);
    }

    pub(crate) fn arena(&mut self) -> &mut TypeArena<'a> {
        self.arena
    }

    pub(crate) fn int_bits(&mut self) -> Option<u32> {
        let bits = self.arena.integer_bits();
        self.region.uses_complete &= bits.is_some();
        bits
    }

    pub(crate) fn abs_range(&self, r: &Range<usize>) -> Range<usize> {
        (self.body_base + r.start)..(self.body_base + r.end)
    }

    pub(crate) fn abs_span(&self, r: &Range<usize>) -> [u32; 2] {
        [
            (self.body_base + r.start) as u32,
            (self.body_base + r.end) as u32,
        ]
    }

    pub(crate) fn text(&self, r: &Range<usize>) -> &'b str {
        self.source
            .get(self.body_base + r.start..self.body_base + r.end)
            .unwrap_or("")
    }

    fn is_active(&self, id: LocalId) -> bool {
        self.active_locals
            .get(id.0 as usize)
            .copied()
            .unwrap_or(false)
    }

    fn mark_active(&mut self, id: LocalId) {
        let idx = id.0 as usize;
        if idx >= self.active_locals.len() {
            self.active_locals.resize(idx + 1, false);
        }
        self.active_locals[idx] = true;
    }

    fn binding(&self, id: LocalId) -> Option<&LocalBinding> {
        self.region.scopes.bindings.get(id.0 as usize)
    }

    fn local_object(&self, id: LocalId) -> Option<ObjectRef<'a>> {
        let b = self.binding(id)?;
        let span = b.name_span;
        let origin = SymbolOrigin::Local {
            file: self.file,
            id,
            span,
        };
        Some(match b.kind {
            LocalBindingKind::Param => ObjectRef::Var {
                origin,
                kind: VarKind::Param,
            },
            LocalBindingKind::NamedResult => ObjectRef::Var {
                origin,
                kind: VarKind::ResultVar,
            },
            LocalBindingKind::Receiver => ObjectRef::Var {
                origin,
                kind: VarKind::Receiver,
            },
            LocalBindingKind::Var
            | LocalBindingKind::ShortVar
            | LocalBindingKind::RangeKey
            | LocalBindingKind::RangeVal
            | LocalBindingKind::TypeSwitchVar => ObjectRef::Var {
                origin,
                kind: VarKind::Local,
            },
            LocalBindingKind::Const => ObjectRef::Const(origin),
            LocalBindingKind::TypeDecl { alias } => ObjectRef::TypeName {
                origin,
                is_alias: alias,
            },
            LocalBindingKind::TypeParam => {
                let index = self
                    .region
                    .scopes
                    .bindings
                    .iter()
                    .take(id.0 as usize)
                    .filter(|other| matches!(other.kind, LocalBindingKind::TypeParam))
                    .count() as u16;
                ObjectRef::TypeParam {
                    owner: self.type_param_owner,
                    index,
                    span,
                }
            }
        })
    }

    fn record_local_var_entry(
        &mut self,
        id: LocalId,
        span: [u32; 2],
        kind: VarKind,
        ty: Option<TypeId>,
    ) {
        let obj = ObjectRef::Var {
            origin: SymbolOrigin::Local {
                file: self.file,
                id,
                span,
            },
            kind,
        };
        self.region.defs.push((span, obj));
        if let Some(ty) = ty {
            self.region.set_local_type(
                id,
                Operand {
                    mode: Mode::Variable,
                    ty,
                    value: None,
                },
            );
        }
    }

    fn record_local_var_def(
        &mut self,
        id: LocalId,
        span: [u32; 2],
        kind: VarKind,
        ty: Option<TypeId>,
    ) {
        self.mark_active(id);
        self.record_local_var_entry(id, span, kind, ty);
    }

    pub(crate) fn populate_signature(&mut self, sig_slice: &str, sig_base: usize, sig: &Signature) {
        let sig_text = |r: &Range<usize>| sig_slice.get(r.start..r.end).unwrap_or("");
        let abs_span = |r: &Range<usize>| [(sig_base + r.start) as u32, (sig_base + r.end) as u32];

        let recv_ann = sig
            .receiver
            .first()
            .map(|p| sig_text(&p.annotation))
            .filter(|s| !s.is_empty());
        self.recv_annotation = recv_ann.map(ToString::to_string);

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
                crate::analysis::scope::receiver_parameters(sig_slice, &recv.annotation);
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
        self.type_formals = formals_owned.clone();

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
                    owner: self.type_param_owner,
                    index: idx as u16,
                    span,
                };
                self.region.defs.push((span, obj));
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
                self.type_param_env.push((name.to_string(), ty));
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
                    owner: self.type_param_owner,
                    index: idx as u16,
                    span,
                };
                self.region.defs.push((span, obj));
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
                    .type_param_env
                    .iter()
                    .any(|(existing, _)| existing == name)
            {
                self.type_param_env.push((name.to_string(), ty));
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

        self.result_types.clear();
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
            self.result_types.push(r_ty);
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

        self.result_types.clear();
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
            self.result_types.push(r_ty);
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
                    .type_param_env
                    .iter()
                    .any(|(existing, _)| existing == name)
            {
                self.type_param_env.push((name.to_string(), tp_ty));
            }
        }
    }

    pub(in crate::analysis::check) fn sig_constraint_refs_valid(
        &mut self,
        sig_slice: &str,
        sig_base: usize,
        ann_range: &Range<usize>,
    ) -> bool {
        let Some(text) = sig_slice.get(ann_range.clone()) else {
            return false;
        };
        if text.trim().is_empty() {
            return false;
        }
        const PREFIX: &str = "interface{";
        let wrapped = format!("{PREFIX}{text}}}");
        let parsed = syntax::parse_type(&wrapped);
        if !parsed.diagnostics.is_empty() {
            return false;
        }
        let abs_start = sig_base + ann_range.start;
        for tr in &parsed.references {
            let (Some(ns), Some(ne)) = (
                tr.name.start.checked_sub(PREFIX.len()),
                tr.name.end.checked_sub(PREFIX.len()),
            ) else {
                return false;
            };
            let name_r = ns..ne;
            let Some(name) = text.get(name_r.clone()) else {
                return false;
            };
            if let Some(qual_r) = &tr.qualifier {
                let (Some(qs), Some(qe)) = (
                    qual_r.start.checked_sub(PREFIX.len()),
                    qual_r.end.checked_sub(PREFIX.len()),
                ) else {
                    return false;
                };
                let adj_qual_r = qs..qe;
                let Some(qual) = text.get(adj_qual_r.clone()) else {
                    return false;
                };
                if self.type_param_env.iter().any(|(n, _)| n == qual)
                    || self.type_formals.iter().any(|(n, _, _)| n == qual)
                {
                    return false;
                }
                if !self.resolve_pkg_qualified_type_use(
                    qual,
                    name,
                    abs_start,
                    &adj_qual_r,
                    &name_r,
                    true,
                ) {
                    return false;
                }
            } else {
                let name_span = [
                    (abs_start + name_r.start) as u32,
                    (abs_start + name_r.end) as u32,
                ];
                if self.record_formal_type_param_use(name, name_span)
                    || self.type_param_env.iter().any(|(n, _)| n == name)
                {
                    continue;
                }
                if !self.resolve_unqualified_type_use(name, abs_start, &name_r, true, true) {
                    return false;
                }
            }
        }
        true
    }

    pub(crate) fn sig_all_refs_valid(
        &mut self,
        sig_slice: &str,
        sig_base: usize,
        sig: &Signature,
    ) -> bool {
        if !sig.diagnostics.is_empty() || !sig.type_parameters.is_empty() {
            return false;
        }
        for param in &sig.parameters {
            if param.annotation.is_empty()
                || !self.sig_type_refs_valid(sig_slice, sig_base, &param.annotation)
            {
                return false;
            }
        }
        for res in &sig.results {
            if res.annotation.is_empty()
                || !self.sig_type_refs_valid(sig_slice, sig_base, &res.annotation)
            {
                return false;
            }
        }
        true
    }

    fn sig_type_refs_valid(
        &mut self,
        sig_slice: &str,
        sig_base: usize,
        ann_range: &Range<usize>,
    ) -> bool {
        let Some(text) = sig_slice.get(ann_range.clone()) else {
            return false;
        };
        let mut parsed = syntax::parse_type(text);
        if !parsed.diagnostics.is_empty() {
            return false;
        }
        let abs_start = sig_base + ann_range.start;
        let ok = self.check_type_references(text, abs_start, &parsed);
        self.check_type_annotation_details(text, abs_start, &mut parsed);
        ok
    }

    pub(crate) fn check_body(&mut self) {
        let root = self.body.root();
        self.check_stmt(root);
    }

    pub(crate) fn resolve_type_syntax_span(&mut self, span: &Range<usize>) -> Option<TypeId> {
        let text = self.text(span);
        let abs_start = self.body_base + span.start;
        self.resolve_type_text_in(text, abs_start)
    }

    pub(crate) fn resolve_type_text_in(&mut self, text: &str, abs_offset: usize) -> Option<TypeId> {
        if text.trim().is_empty() {
            return None;
        }
        let mut parsed = syntax::parse_type(text);
        if !parsed.diagnostics.is_empty() {
            return None;
        }

        let refs_ok = self.check_type_references(text, abs_offset, &parsed);
        self.check_type_annotation_details(text, abs_offset, &mut parsed);
        if !refs_ok {
            return None;
        }

        self.resolve_type_text_unverified(text, abs_offset)
    }

    fn resolve_type_text_unverified(&mut self, text: &str, abs_offset: usize) -> Option<TypeId> {
        let (visible_defs, extra_env) = self.visible_type_env(abs_offset);
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

        let ty = self.arena.declared_scope_type_in(
            self.file,
            text,
            Some(self.file_offset(abs_offset)),
            &defs,
            &env_pairs,
        )?;
        self.arena.validate_type(ty).map(|()| ty)
    }

    pub(in crate::analysis::check) fn check_type_references(
        &mut self,
        text: &str,
        abs_offset: usize,
        parsed: &syntax::TypeSyntax,
    ) -> bool {
        let mut ok = true;
        for tr in &parsed.references {
            let Some(name) = text.get(tr.name.clone()) else {
                ok = false;
                continue;
            };
            if let Some(qual_r) = &tr.qualifier {
                let Some(qual) = text.get(qual_r.clone()) else {
                    ok = false;
                    continue;
                };
                if self.type_param_env.iter().any(|(n, _)| n == qual)
                    || self.type_formals.iter().any(|(n, _, _)| n == qual)
                {
                    ok = false;
                    continue;
                }
                if !self
                    .resolve_pkg_qualified_type_use(qual, name, abs_offset, qual_r, &tr.name, true)
                {
                    ok = false;
                }
            } else {
                let allow_comparable = parsed.interface_members.iter().any(|m| {
                    m.name.is_none()
                        && m.annotation.start <= tr.name.start
                        && tr.name.end <= m.annotation.end
                });
                if !self.resolve_unqualified_type_use(
                    name,
                    abs_offset,
                    &tr.name,
                    true,
                    allow_comparable,
                ) {
                    ok = false;
                }
            }
        }
        ok
    }

    pub(in crate::analysis::check) fn check_type_annotation_details(
        &mut self,
        text: &str,
        abs_offset: usize,
        parsed: &mut syntax::TypeSyntax,
    ) {
        let exprs = std::mem::take(&mut parsed.expressions);
        if !exprs.is_empty() {
            let mut is_child = vec![false; exprs.len()];
            let mut mark = |id: ExprId| {
                if let Some(slot) = is_child.get_mut(id.index()) {
                    *slot = true;
                }
            };
            for expr in &exprs {
                match &expr.kind {
                    ExprKind::Name
                    | ExprKind::Literal
                    | ExprKind::Missing
                    | ExprKind::Type
                    | ExprKind::FunctionLiteral { .. } => {}
                    ExprKind::Unary { operand, .. }
                    | ExprKind::Parenthesized {
                        expression: operand,
                    }
                    | ExprKind::Selector { operand, .. }
                    | ExprKind::TypeAssertion { operand, .. }
                    | ExprKind::TypeSwitchGuard { operand } => mark(*operand),
                    ExprKind::Binary { left, right, .. }
                    | ExprKind::KeyValue {
                        key: left,
                        value: right,
                    } => {
                        mark(*left);
                        mark(*right);
                    }
                    ExprKind::Composite {
                        literal_type,
                        elements,
                    } => {
                        if let Some(lt) = literal_type {
                            mark(*lt);
                        }
                        for &el in elements {
                            mark(el);
                        }
                    }
                    ExprKind::Call {
                        function,
                        arguments,
                        ..
                    } => {
                        mark(*function);
                        for &arg in arguments {
                            mark(arg);
                        }
                    }
                    ExprKind::Index { operand, arguments } => {
                        mark(*operand);
                        for &arg in arguments {
                            mark(arg);
                        }
                    }
                    ExprKind::Slice {
                        operand,
                        low,
                        high,
                        max,
                        ..
                    } => {
                        mark(*operand);
                        if let Some(id) = low {
                            mark(*id);
                        }
                        if let Some(id) = high {
                            mark(*id);
                        }
                        if let Some(id) = max {
                            mark(*id);
                        }
                    }
                }
            }
            let expr_tree = BlockTree {
                root: StmtId(0),
                statements: Vec::new(),
                expressions: exprs,
                diagnostics: Vec::new(),
                type_references: Vec::new(),
                type_parameters: Vec::new(),
            };
            let roots: Vec<_> = is_child
                .iter()
                .enumerate()
                .filter_map(|(index, child)| (!child).then_some(ExprId(index)))
                .collect();
            self.region.scopes.include_type_expressions(
                self.source,
                abs_offset,
                &expr_tree,
                &roots,
                self.cursor_offset,
            );
            self.region.extend_binding_storage();
            let snap = self.closure_snapshot();
            let mut child = RegionChecker::new(
                self.arena,
                self.decls,
                self.file,
                self.source,
                abs_offset,
                &expr_tree,
                self.region,
                self.iota,
                self.cursor_offset,
            );
            child.restore_closure_snapshot(snap);
            for root in roots {
                let _ = child.check_expr_outcome(root, None);
            }
        }

        let owner_sym = self
            .region
            .owner
            .filter(|d| d.symbol.is_type())
            .map(|d| d.symbol);

        for field in &parsed.fields {
            if field.embedded || text.get(field.name.clone()) == Some("_") {
                continue;
            }
            let abs_span = [
                (abs_offset + field.name.start) as u32,
                (abs_offset + field.name.end) as u32,
            ];
            self.region.defs.push((
                abs_span,
                ObjectRef::Field(MemberDeclId {
                    file: self.file,
                    owner: owner_sym,
                    selection: abs_span,
                    embedded: false,
                    builtin: false,
                }),
            ));
            if let Some(raw_ann) = text.get(field.annotation.clone()) {
                let ann_text = raw_ann.trim();
                if !ann_text.is_empty() {
                    let lead = raw_ann.len() - raw_ann.trim_start().len();
                    if let Some(ty) = self.resolve_type_text_unverified(
                        ann_text,
                        abs_offset + field.annotation.start + lead,
                    ) {
                        self.region.expr_types.push((
                            abs_span,
                            Operand {
                                mode: Mode::Variable,
                                ty,
                                value: None,
                            },
                        ));
                    }
                }
            }
        }

        for member in &parsed.interface_members {
            let Some(name_r) = &member.name else {
                continue;
            };
            if text.get(name_r.clone()) == Some("_") {
                continue;
            }
            let abs_span = [
                (abs_offset + name_r.start) as u32,
                (abs_offset + name_r.end) as u32,
            ];
            self.region.defs.push((
                abs_span,
                ObjectRef::Method(MethodOrigin::Interface(MemberDeclId {
                    file: self.file,
                    owner: owner_sym,
                    selection: abs_span,
                    embedded: false,
                    builtin: false,
                })),
            ));
        }

        for sig_name in &parsed.signature_names {
            if text.get(sig_name.name.clone()) == Some("_") {
                continue;
            }
            let abs_span = [
                (abs_offset + sig_name.name.start) as u32,
                (abs_offset + sig_name.name.end) as u32,
            ];
            let kind = match sig_name.kind {
                syntax::SignatureNameKind::Parameter => LocalBindingKind::Param,
                syntax::SignatureNameKind::Result => LocalBindingKind::NamedResult,
            };
            let Some(local_id) = self.region.scopes.signature_name(abs_span, kind) else {
                self.region.uses_complete = false;
                continue;
            };
            self.region.extend_binding_storage();
            if let Some(object) = self.local_object(local_id) {
                self.region.defs.push((abs_span, object));
            }
            let Some(raw_ann) = text.get(sig_name.annotation.clone()) else {
                continue;
            };
            let ann_text = raw_ann.trim();
            if ann_text.is_empty() {
                continue;
            }
            let lead = raw_ann.len() - raw_ann.trim_start().len();
            let ann_offset = abs_offset + sig_name.annotation.start + lead;
            let ty = if sig_name.variadic {
                let elem_raw = ann_text.strip_prefix("...").unwrap_or(ann_text);
                let elem_lead = elem_raw.len() - elem_raw.trim_start().len();
                let elem_text = elem_raw.trim_start();
                self.resolve_type_text_unverified(
                    elem_text,
                    ann_offset + (ann_text.len() - elem_raw.len()) + elem_lead,
                )
                .and_then(|elem| self.arena.slice_type(elem))
            } else {
                self.resolve_type_text_unverified(ann_text, ann_offset)
            };
            if let Some(ty) = ty {
                self.region.set_local_type(
                    local_id,
                    Operand {
                        mode: Mode::Variable,
                        ty,
                        value: None,
                    },
                );
                self.region.expr_types.push((
                    abs_span,
                    Operand {
                        mode: Mode::Variable,
                        ty,
                        value: None,
                    },
                ));
            }
        }
    }

    fn visible_type_env(
        &self,
        abs_offset: usize,
    ) -> (Vec<LocalTypeDefEntry>, Vec<(String, TypeId)>) {
        let mut seen = BTreeSet::new();
        let mut defs = Vec::new();
        let mut env = Vec::new();

        for (name, local_id) in self
            .region
            .scopes
            .visible_bindings(self.source, abs_offset as u32)
        {
            if !self.is_active(local_id) && self.declaring_type != Some(local_id) {
                continue;
            }
            if !seen.insert(name.to_string()) {
                continue;
            }
            let Some(b) = self.binding(local_id) else {
                continue;
            };
            match b.kind {
                LocalBindingKind::TypeDecl { .. } => {
                    if self.declaring_type == Some(local_id) {
                        if let Some(entry) = self
                            .local_type_defs
                            .iter()
                            .rev()
                            .find(|e| e.local_id == local_id)
                        {
                            defs.push(entry.clone());
                        }
                    } else if let Some(op) = self.region.local_type(local_id) {
                        env.push((name.to_string(), op.ty));
                    } else if let Some(entry) = self
                        .local_type_defs
                        .iter()
                        .rev()
                        .find(|e| e.local_id == local_id)
                    {
                        defs.push(entry.clone());
                    }
                }
                LocalBindingKind::TypeParam => {
                    if let Some(op) = self.region.local_type(local_id) {
                        env.push((name.to_string(), op.ty));
                    }
                }
                _ => {}
            }
        }

        for (name, ty) in &self.type_param_env {
            if seen.insert(name.clone()) {
                env.push((name.clone(), *ty));
            }
        }
        (defs, env)
    }

    pub(in crate::analysis::check) fn resolve_pkg_qualified_type_use(
        &mut self,
        qual: &str,
        name: &str,
        abs_offset: usize,
        qual_r: &Range<usize>,
        name_r: &Range<usize>,
        check_locals: bool,
    ) -> bool {
        if check_locals
            && let Some(local_id) =
                self.region
                    .scopes
                    .resolve_at(self.source, (abs_offset + qual_r.start) as u32, qual)
            && self.is_active(local_id)
        {
            return false;
        }
        let Some((pkg_obj, _target_dir)) = self.find_import(qual) else {
            self.report_missing_name(qual, abs_offset + qual_r.start..abs_offset + qual_r.end);
            return false;
        };
        let qual_span = [
            (abs_offset + qual_r.start) as u32,
            (abs_offset + qual_r.end) as u32,
        ];
        self.region.uses.push((qual_span, pkg_obj));

        let Some(decl) = self.resolve_imported_decl(qual, name) else {
            self.report_missing_imported_name(
                qual,
                name,
                abs_offset + qual_r.start,
                abs_offset + name_r.start..abs_offset + name_r.end,
            );
            return false;
        };
        if decl.symbol.kind != SymbolKind::Type {
            return false;
        }
        let name_span = [
            (abs_offset + name_r.start) as u32,
            (abs_offset + name_r.end) as u32,
        ];
        self.region.uses.push((name_span, decl_object(decl)));
        if decl.symbol.generic {
            true
        } else {
            self.decls.type_of(self.arena, decl).is_some()
        }
    }

    pub(in crate::analysis::check) fn resolve_unqualified_type_use(
        &mut self,
        name: &str,
        abs_offset: usize,
        name_r: &Range<usize>,
        check_locals: bool,
        allow_comparable: bool,
    ) -> bool {
        let name_span = [
            (abs_offset + name_r.start) as u32,
            (abs_offset + name_r.end) as u32,
        ];
        if check_locals
            && let Some(local_id) = self
                .region
                .scopes
                .resolve_at(self.source, name_span[0], name)
        {
            let active = self.is_active(local_id) || self.declaring_type == Some(local_id);
            if active {
                let Some(b) = self.binding(local_id) else {
                    return false;
                };
                match b.kind {
                    LocalBindingKind::TypeDecl { .. } => {
                        let op = self.region.local_type(local_id);
                        if let Some(obj) = self.local_object(local_id) {
                            self.region.uses.push((name_span, obj));
                        }
                        return op.is_some() || self.declaring_type == Some(local_id);
                    }
                    LocalBindingKind::TypeParam => {
                        let op = self.region.local_type(local_id);
                        if let Some(obj) = self.local_object(local_id) {
                            self.region.uses.push((name_span, obj));
                        }
                        return op.is_some();
                    }
                    _ => return false,
                }
            }
        }

        if check_locals && self.record_formal_type_param_use(name, name_span) {
            return true;
        }

        if self.find_import(name).is_some() {
            return false;
        }

        match self.resolve_file_or_package_symbol(name) {
            FileSymbolLookup::Found(decl) => {
                if decl.symbol.kind != SymbolKind::Type {
                    return false;
                }
                self.region.uses.push((name_span, decl_object(decl)));
                if decl.symbol.generic {
                    true
                } else {
                    self.decls.type_of(self.arena, decl).is_some()
                }
            }
            FileSymbolLookup::Ambiguous => false,
            FileSymbolLookup::NotFound => {
                if allow_comparable && name == "comparable" {
                    self.region
                        .uses
                        .push((name_span, ObjectRef::BuiltinType("comparable")));
                    true
                } else if self.arena.builtin_type(self.file, name).is_some() {
                    if let Some(builtin_name) = builtin_type_static_name(name) {
                        self.region
                            .uses
                            .push((name_span, ObjectRef::BuiltinType(builtin_name)));
                    }
                    true
                } else {
                    self.report_missing_name(name, name_span[0] as usize..name_span[1] as usize);
                    false
                }
            }
        }
    }

    pub(crate) fn find_import(&mut self, alias: &str) -> Option<(ObjectRef<'a>, &'a Path)> {
        let ws = self.arena.workspace();
        let target_dir = self.decls.find_import_dir(ws, self.file, alias)?;
        Some((
            ObjectRef::PkgName {
                import_path_span: [0, 0],
                target_dir,
            },
            target_dir,
        ))
    }

    pub(crate) fn resolve_file_or_package_symbol(&mut self, name: &str) -> FileSymbolLookup<'a> {
        let ws = self.arena.workspace();
        match self.decls.resolve_unqualified(ws, self.file, name) {
            crate::analysis::resolve::UnqualifiedLookup::Found(p, symbol) => {
                FileSymbolLookup::Found(DeclId { file: p, symbol })
            }
            crate::analysis::resolve::UnqualifiedLookup::Ambiguous => FileSymbolLookup::Ambiguous,
            crate::analysis::resolve::UnqualifiedLookup::NotFound => FileSymbolLookup::NotFound,
        }
    }

    pub(in crate::analysis::check) fn resolve_imported_decl(
        &mut self,
        alias: &str,
        name: &str,
    ) -> Option<DeclId<'a>> {
        let ws = self.arena.workspace();
        self.decls.resolve_imported(ws, self.file, alias, name)
    }

    pub(in crate::analysis::check) fn record_formal_type_param_use(
        &mut self,
        name: &str,
        name_span: [u32; 2],
    ) -> bool {
        let Some((index, (_, start, _))) = self
            .type_formals
            .iter()
            .enumerate()
            .find(|(_, (n, _, _))| n == name)
        else {
            return false;
        };
        let decl_span = [*start as u32, (*start + name.len()) as u32];
        if name_span != decl_span {
            self.region.uses.push((
                name_span,
                ObjectRef::TypeParam {
                    owner: self.type_param_owner,
                    index: index as u16,
                    span: decl_span,
                },
            ));
        }
        true
    }

    pub(crate) fn closure_snapshot(&self) -> ClosureSnapshot<'a> {
        ClosureSnapshot {
            source_base: self.source_base,
            active_locals: self.active_locals.clone(),
            type_param_owner: self.type_param_owner,
            type_param_env: self.type_param_env.clone(),
            type_formals: self.type_formals.clone(),
            recv_annotation: self.recv_annotation.clone(),
            local_type_defs: self.local_type_defs.clone(),
        }
    }

    pub(crate) fn restore_closure_snapshot(&mut self, snap: ClosureSnapshot<'a>) {
        self.source_base = snap.source_base;
        self.active_locals = snap.active_locals;
        self.type_param_owner = snap.type_param_owner;
        self.type_param_env = snap.type_param_env;
        self.type_formals = snap.type_formals;
        self.recv_annotation = snap.recv_annotation;
        self.local_type_defs = snap.local_type_defs;
    }

    fn record_expr_outcome(&mut self, id: ExprId, outcome: &ExprOutcome<'a, 'b>) {
        if let ExprOutcome::Value(op) = outcome
            && let Some(expr) = self.body.expression(id)
        {
            let span = self.abs_span(&expr.span);
            if op.mode == Mode::Invalid {
                self.region.uses_complete = false;
            }
            self.region.expr_types.push((span, *op));
        }
    }

    fn basic_bool(&mut self) -> Option<TypeId> {
        self.arena.basic(BasicKind::Bool)
    }
}

pub(crate) fn decl_object(decl: DeclId<'_>) -> ObjectRef<'_> {
    match decl.symbol.kind {
        SymbolKind::Constant => ObjectRef::Const(SymbolOrigin::Package(decl)),
        SymbolKind::Type => ObjectRef::TypeName {
            origin: SymbolOrigin::Package(decl),
            is_alias: decl.symbol.alias,
        },
        SymbolKind::Variable => ObjectRef::Var {
            origin: SymbolOrigin::Package(decl),
            kind: VarKind::Package,
        },
        SymbolKind::Function => ObjectRef::Func(decl),
        SymbolKind::Method => ObjectRef::Method(MethodOrigin::Concrete(decl)),
    }
}

pub(crate) enum FileSymbolLookup<'a> {
    Found(DeclId<'a>),
    Ambiguous,
    NotFound,
}

pub(crate) fn builtin_type_static_name(name: &str) -> Option<&'static str> {
    Some(match name {
        "any" => "any",
        "bool" => "bool",
        "byte" => "byte",
        "comparable" => "comparable",
        "complex64" => "complex64",
        "complex128" => "complex128",
        "error" => "error",
        "float32" => "float32",
        "float64" => "float64",
        "int" => "int",
        "int8" => "int8",
        "int16" => "int16",
        "int32" => "int32",
        "int64" => "int64",
        "rune" => "rune",
        "string" => "string",
        "uint" => "uint",
        "uint8" => "uint8",
        "uint16" => "uint16",
        "uint32" => "uint32",
        "uint64" => "uint64",
        "uintptr" => "uintptr",
        _ => return None,
    })
}
