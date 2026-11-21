//! Type-expression checking, reference recording, and AST lowering in [`RegionChecker`].

use std::ops::Range;

use crate::analysis::engine::check::RegionChecker;
use crate::analysis::engine::check::region::RecordMask;
use crate::analysis::engine::obj::{MemberDeclId, MethodOrigin, ObjectRef};
use crate::analysis::engine::scope::tree::LocalBindingKind;
use crate::analysis::engine::types::{LocalTypeDefinition, Mode, Operand, TypeId};
use crate::syntax::{self, BlockTree, ExprId, ExprKind, Signature, StmtId};

impl<'a, 'b> RegionChecker<'a, 'b> {
    pub(in crate::analysis::engine::check) fn sig_constraint_refs_valid(
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
        let parsed = syntax::parse_constraint(text);
        if !parsed.diagnostics.is_empty() {
            return false;
        }
        let abs_start = sig_base + ann_range.start;
        for tr in &parsed.references {
            let Some(name) = text.get(tr.name.clone()) else {
                return false;
            };
            if let Some(qual_r) = &tr.qualifier {
                let Some(qual) = text.get(qual_r.clone()) else {
                    return false;
                };
                if self.env.type_param_env.iter().any(|(n, _)| n == qual)
                    || self.env.type_formals.iter().any(|(n, _, _)| n == qual)
                {
                    return false;
                }
                if !self
                    .resolve_pkg_qualified_type_use(qual, name, abs_start, qual_r, &tr.name, true)
                {
                    return false;
                }
            } else {
                let name_span = [
                    (abs_start + tr.name.start) as u32,
                    (abs_start + tr.name.end) as u32,
                ];
                if self.record_formal_type_param_use(name, name_span)
                    || self.env.type_param_env.iter().any(|(n, _)| n == name)
                {
                    continue;
                }
                if !self.resolve_unqualified_type_use(name, abs_start, &tr.name, true, true) {
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

    pub(in crate::analysis::engine::check) fn sig_type_refs_valid(
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
            self.region.uses_complete = false;
            return false;
        }
        let abs_start = sig_base + ann_range.start;
        let ok = self.check_type_references(text, abs_start, &parsed);
        self.check_type_annotation_details(text, abs_start, &mut parsed);
        ok
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
            self.region.uses_complete = false;
            return None;
        }

        let refs_ok = self.check_type_references(text, abs_offset, &parsed);
        self.check_type_annotation_details(text, abs_offset, &mut parsed);
        if !refs_ok {
            return None;
        }

        self.resolve_type_text_parsed(text, abs_offset, &parsed)
    }

    pub(super) fn resolve_type_text_unverified(
        &mut self,
        text: &str,
        abs_offset: usize,
    ) -> Option<TypeId> {
        let parsed = syntax::parse_type(text);
        self.resolve_type_text_parsed(text, abs_offset, &parsed)
    }

    fn resolve_type_text_parsed(
        &mut self,
        text: &str,
        abs_offset: usize,
        parsed: &syntax::TypeSyntax,
    ) -> Option<TypeId> {
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

        let ty = self.arena.declared_scope_type_in_parsed(
            self.file,
            text,
            parsed,
            Some(self.file_offset(abs_offset)),
            &defs,
            &env_pairs,
        )?;
        self.arena.validate_type(ty).map(|()| ty)
    }

    pub(in crate::analysis::engine::check) fn check_type_references(
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
                if self.env.type_param_env.iter().any(|(n, _)| n == qual)
                    || self.env.type_formals.iter().any(|(n, _, _)| n == qual)
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

    pub(in crate::analysis::engine::check) fn check_type_annotation_details(
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
                    | ExprKind::Literal(_)
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
                delimiters: Vec::new(),
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
            let child_env = self.env.clone();
            let mut child = RegionChecker::with_env(
                self.arena,
                self.decls,
                self.file,
                self.source,
                abs_offset,
                &expr_tree,
                self.region,
                self.iota,
                self.cursor_offset,
                child_env,
            );
            for root in roots {
                let _ = child.check_expr_outcome(root, None);
            }
            child.flush_untyped();
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
            self.record_def(
                abs_span,
                ObjectRef::Field(MemberDeclId {
                    file: self.file,
                    owner: owner_sym,
                    selection: abs_span,
                    embedded: false,
                    builtin: false,
                }),
            );
            if self.record_mask.contains(RecordMask::TYPES)
                && let Some(raw_ann) = text.get(field.annotation.clone())
            {
                let ann_text = raw_ann.trim();
                if !ann_text.is_empty() {
                    let lead = raw_ann.len() - raw_ann.trim_start().len();
                    if let Some(ty) = self.resolve_type_text_unverified(
                        ann_text,
                        abs_offset + field.annotation.start + lead,
                    ) {
                        self.record_type_and_value(
                            abs_span,
                            Operand {
                                mode: Mode::Variable,
                                ty,
                                value: None,
                            },
                        );
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
            self.record_def(
                abs_span,
                ObjectRef::Method(MethodOrigin::Interface(MemberDeclId {
                    file: self.file,
                    owner: owner_sym,
                    selection: abs_span,
                    embedded: false,
                    builtin: false,
                })),
            );
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
                self.record_def(abs_span, object);
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
                self.record_type_and_value(
                    abs_span,
                    Operand {
                        mode: Mode::Variable,
                        ty,
                        value: None,
                    },
                );
            }
        }
    }
}
