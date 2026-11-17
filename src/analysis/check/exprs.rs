//! Expression checking for [`RegionChecker`].

use std::collections::BTreeSet;
use std::ops::Range;

use crate::analysis::check::region::{CallSiteInfo, InstanceInfo};
use crate::analysis::check::{
    ExprOutcome, FileSymbolLookup, RegionChecker, builtin_type_static_name, decl_object,
};
use crate::analysis::obj::{BuiltinFunction, DeclId, ObjectRef};
use crate::analysis::scope::tree::LocalBindingKind;
use crate::analysis::types::{BasicKind, Constant, Mode, Operand, TypeId};
use crate::syntax::{self, ExprId, ExprKind, Signature, SymbolKind};

impl<'a, 'b> RegionChecker<'a, 'b> {
    pub(crate) fn check_expr(&mut self, id: ExprId, expected: Option<TypeId>) -> Option<Operand> {
        match self.check_expr_outcome(id, expected) {
            ExprOutcome::Value(op) if op.mode.is_value() || op.mode == Mode::TypeExpr => Some(op),
            ExprOutcome::Value(_)
            | ExprOutcome::Package { .. }
            | ExprOutcome::PartialGenericFunc { .. }
            | ExprOutcome::GenericType(_) => None,
        }
    }

    pub(crate) fn check_multi_expr(
        &mut self,
        id: ExprId,
        expected_count: usize,
    ) -> Option<Vec<Operand>> {
        let int_bits = self.int_bits()?;

        if expected_count <= 1 {
            return self.check_expr(id, None).map(|op| vec![op]);
        }
        let target_id = self.unwrap_parens(id);
        enum MultiAction {
            Index {
                operand: ExprId,
                arguments: Vec<ExprId>,
            },
            TypeAssertion {
                operand: ExprId,
                asserted_type: Range<usize>,
            },
            Receive {
                operand: ExprId,
            },
            Call {
                function: ExprId,
                arguments: Vec<ExprId>,
                variadic: bool,
            },
            Other,
        }
        let outer_expr = self.body.expression(id)?;
        let outer_span = outer_expr.span.clone();
        let target_expr = self.body.expression(target_id)?;
        let target_span = target_expr.span.clone();
        let action = match &target_expr.kind {
            ExprKind::Index { operand, arguments } => MultiAction::Index {
                operand: *operand,
                arguments: arguments.clone(),
            },
            ExprKind::TypeAssertion {
                operand,
                asserted_type,
            } => MultiAction::TypeAssertion {
                operand: *operand,
                asserted_type: asserted_type.clone(),
            },
            ExprKind::Unary { operator, operand } if self.text(operator) == "<-" => {
                MultiAction::Receive { operand: *operand }
            }
            ExprKind::Call {
                function,
                arguments,
                variadic,
            } => MultiAction::Call {
                function: *function,
                arguments: arguments.clone(),
                variadic: *variadic,
            },
            _ => MultiAction::Other,
        };

        let ops = match action {
            MultiAction::Index { operand, arguments } if expected_count == 2 => {
                let recv = self.check_expr(operand, None)?;
                if arguments.len() != 1 {
                    return None;
                }
                let key = self.check_expr(arguments[0], None)?;
                let mut out = Vec::with_capacity(2);
                for result in 0..2 {
                    out.push(self.arena.index(recv, key, result, 2, int_bits)?);
                }
                Some(out)
            }
            MultiAction::TypeAssertion {
                operand,
                asserted_type,
            } if expected_count == 2 => {
                let src = self.check_expr(operand, None)?;
                let target_ty = self.resolve_type_syntax_span(&asserted_type)?;
                let mut out = Vec::with_capacity(2);
                for result in 0..2 {
                    out.push(self.arena.assertion(src, target_ty, result, 2, int_bits)?);
                }
                Some(out)
            }
            MultiAction::Receive { operand } if expected_count == 2 => {
                let ch = self.check_expr(operand, None)?;
                let mut out = Vec::with_capacity(2);
                for result in 0..2 {
                    out.push(self.arena.receive_slot(ch, result, 2)?);
                }
                Some(out)
            }
            MultiAction::Call {
                function,
                arguments,
                variadic,
            } => {
                self.check_call_multi(function, &arguments, variadic, &target_span, expected_count)
            }
            _ => {
                let _ = self.check_expr_outcome(target_id, None);
                None
            }
        };

        if let Some(ref ops) = ops
            && let Some(first) = ops.first()
        {
            if ops.len() > 1 {
                let tys: Vec<TypeId> = ops.iter().map(|o| o.ty).collect();
                if let Some(tuple_ty) = self.arena.tuple(&tys) {
                    let tuple_op = Operand {
                        mode: Mode::Value,
                        ty: tuple_ty,
                        value: None,
                    };
                    self.region
                        .expr_types
                        .push((self.abs_span(&target_span), tuple_op));
                    if target_id != id {
                        self.region
                            .expr_types
                            .push((self.abs_span(&outer_span), tuple_op));
                    }
                }
            } else {
                self.region
                    .expr_types
                    .push((self.abs_span(&target_span), *first));
            }
        }
        ops
    }

    pub(crate) fn check_expr_outcome(
        &mut self,
        id: ExprId,
        expected: Option<TypeId>,
    ) -> ExprOutcome<'a, 'b> {
        enum ExprAction {
            Name(Range<usize>),
            Literal(Range<usize>),
            Missing,
            Type(Range<usize>),
            Composite {
                span: Range<usize>,
                literal_type: Option<ExprId>,
                elements: Vec<ExprId>,
            },
            KeyValue {
                key: ExprId,
                value: ExprId,
            },
            FunctionLiteral {
                signature: Range<usize>,
                body: Range<usize>,
            },
            Unary {
                operator: Range<usize>,
                operand: ExprId,
            },
            Binary {
                left: ExprId,
                operator: Range<usize>,
                right: ExprId,
            },
            Parenthesized(ExprId),
            Selector {
                operand: ExprId,
                field: Range<usize>,
            },
            Call {
                span: Range<usize>,
                function: ExprId,
                arguments: Vec<ExprId>,
                variadic: bool,
            },
            Index {
                span: Range<usize>,
                operand: ExprId,
                arguments: Vec<ExprId>,
            },
            Slice {
                operand: ExprId,
                low: Option<ExprId>,
                high: Option<ExprId>,
                max: Option<ExprId>,
                full: bool,
            },
            TypeAssertion {
                operand: ExprId,
                asserted_type: Range<usize>,
            },
            TypeSwitchGuard(ExprId),
        }

        let Some(e) = self.body.expression(id) else {
            return ExprOutcome::invalid();
        };
        let span = e.span.clone();
        // Unknown inner contexts are barriers: an enclosing expression's
        // expectation must not leak into unrelated operands or callee names.
        self.region.record_expected(self.abs_span(&span), expected);
        let action = match &e.kind {
            ExprKind::Name => ExprAction::Name(span),
            ExprKind::Literal => ExprAction::Literal(span),
            ExprKind::Missing => ExprAction::Missing,
            ExprKind::Type => ExprAction::Type(span),
            ExprKind::Composite {
                literal_type,
                elements,
            } => ExprAction::Composite {
                span,
                literal_type: *literal_type,
                elements: elements.clone(),
            },
            ExprKind::KeyValue { key, value } => ExprAction::KeyValue {
                key: *key,
                value: *value,
            },
            ExprKind::FunctionLiteral { signature, body } => ExprAction::FunctionLiteral {
                signature: signature.clone(),
                body: body.clone(),
            },
            ExprKind::Unary { operator, operand } => ExprAction::Unary {
                operator: operator.clone(),
                operand: *operand,
            },
            ExprKind::Binary {
                left,
                operator,
                right,
            } => ExprAction::Binary {
                left: *left,
                operator: operator.clone(),
                right: *right,
            },
            ExprKind::Parenthesized { expression } => ExprAction::Parenthesized(*expression),
            ExprKind::Selector { operand, field } => ExprAction::Selector {
                operand: *operand,
                field: field.clone(),
            },
            ExprKind::Call {
                function,
                arguments,
                variadic,
            } => ExprAction::Call {
                span,
                function: *function,
                arguments: arguments.clone(),
                variadic: *variadic,
            },
            ExprKind::Index { operand, arguments } => ExprAction::Index {
                span,
                operand: *operand,
                arguments: arguments.clone(),
            },
            ExprKind::Slice {
                operand,
                low,
                high,
                max,
                full,
            } => ExprAction::Slice {
                operand: *operand,
                low: *low,
                high: *high,
                max: *max,
                full: *full,
            },
            ExprKind::TypeAssertion {
                operand,
                asserted_type,
            } => ExprAction::TypeAssertion {
                operand: *operand,
                asserted_type: asserted_type.clone(),
            },
            ExprKind::TypeSwitchGuard { operand } => ExprAction::TypeSwitchGuard(*operand),
        };

        let outcome = match action {
            ExprAction::Missing => ExprOutcome::invalid(),
            ExprAction::Name(span) => self.check_name(&span),
            ExprAction::Literal(span) => match self.arena.literal(self.text(&span)) {
                Some(op) => ExprOutcome::Value(op),
                None => ExprOutcome::invalid(),
            },
            ExprAction::Type(span) => match self.resolve_type_syntax_span(&span) {
                Some(ty) => ExprOutcome::Value(Operand {
                    mode: Mode::TypeExpr,
                    ty,
                    value: None,
                }),
                None => ExprOutcome::invalid(),
            },
            ExprAction::Composite {
                span,
                literal_type,
                elements,
            } => self.check_composite(&span, literal_type, &elements, expected),
            ExprAction::KeyValue { key, value } => {
                let _ = self.check_expr_outcome(key, None);
                self.check_expr_outcome(value, expected)
            }
            ExprAction::FunctionLiteral { signature, body } => {
                self.check_func_literal(&signature, &body)
            }
            ExprAction::Unary { operator, operand } => self.check_unary(&operator, operand),
            ExprAction::Binary {
                left,
                operator,
                right,
            } => self.check_binary(left, &operator, right, expected),
            ExprAction::Parenthesized(inner) => self.check_expr_outcome(inner, expected),
            ExprAction::Selector { operand, field } => self.check_selector(operand, &field),
            ExprAction::Call {
                span,
                function,
                arguments,
                variadic,
            } => self.check_call_single(function, &arguments, variadic, &span),
            ExprAction::Index {
                span,
                operand,
                arguments,
            } => self.check_index(&span, operand, &arguments),
            ExprAction::Slice {
                operand,
                low,
                high,
                max,
                full,
            } => self.check_slice(operand, low, high, max, full),
            ExprAction::TypeAssertion {
                operand,
                asserted_type,
            } => self.check_type_assertion(operand, &asserted_type),
            ExprAction::TypeSwitchGuard(operand) => self.check_expr_outcome(operand, None),
        };

        self.record_expr_outcome(id, &outcome);
        outcome
    }

    fn check_name(&mut self, span: &Range<usize>) -> ExprOutcome<'a, 'b> {
        let name = self.text(span);
        let abs_span = self.abs_span(span);

        if let Some(local_id) = self
            .region
            .scopes
            .resolve_at(self.source, abs_span[0], name)
            && self.is_active(local_id)
        {
            let local_op = self.region.local_type(local_id);
            if let Some(obj) = self.local_object(local_id) {
                self.region.uses.push((abs_span, obj));
            }
            let kind = self.binding(local_id).map(|b| b.kind);
            let Some(op) = local_op else {
                return ExprOutcome::invalid();
            };
            return match kind {
                Some(LocalBindingKind::Const) => ExprOutcome::Value(Operand {
                    mode: Mode::Constant,
                    ty: op.ty,
                    value: op.value,
                }),
                Some(LocalBindingKind::TypeDecl { .. } | LocalBindingKind::TypeParam) => {
                    ExprOutcome::Value(Operand {
                        mode: Mode::TypeExpr,
                        ty: op.ty,
                        value: None,
                    })
                }
                _ => ExprOutcome::Value(Operand {
                    mode: Mode::Variable,
                    ty: op.ty,
                    value: None,
                }),
            };
        }

        if let Some((pkg_obj, _)) = self.find_import(name) {
            self.region.uses.push((abs_span, pkg_obj));
            return ExprOutcome::Package { alias: name };
        }

        match self.resolve_file_or_package_symbol(name) {
            FileSymbolLookup::Found(decl) => {
                return self.resolve_decl_as_outcome(decl, abs_span);
            }
            FileSymbolLookup::Ambiguous => return ExprOutcome::invalid(),
            FileSymbolLookup::NotFound => {}
        }

        match name {
            "nil" => {
                let ty = self.arena.basic(BasicKind::UntypedNil);
                self.region.uses.push((abs_span, ObjectRef::Nil));
                ty.map(|ty| {
                    ExprOutcome::Value(Operand {
                        mode: Mode::Value,
                        ty,
                        value: None,
                    })
                })
                .unwrap_or(ExprOutcome::invalid())
            }
            "true" => {
                let ty = self.arena.basic(BasicKind::UntypedBool);
                self.region
                    .uses
                    .push((abs_span, ObjectRef::BuiltinConst("true")));
                ty.map(|ty| {
                    ExprOutcome::Value(Operand {
                        mode: Mode::Constant,
                        ty,
                        value: Some(Constant::Boolean),
                    })
                })
                .unwrap_or(ExprOutcome::invalid())
            }
            "false" => {
                let ty = self.arena.basic(BasicKind::UntypedBool);
                self.region
                    .uses
                    .push((abs_span, ObjectRef::BuiltinConst("false")));
                ty.map(|ty| {
                    ExprOutcome::Value(Operand {
                        mode: Mode::Constant,
                        ty,
                        value: Some(Constant::Boolean),
                    })
                })
                .unwrap_or(ExprOutcome::invalid())
            }
            "iota" => {
                let Some(iota_v) = self.iota else {
                    return ExprOutcome::invalid();
                };
                let ty = self.arena.basic(BasicKind::UntypedInt);
                self.region
                    .uses
                    .push((abs_span, ObjectRef::BuiltinConst("iota")));
                ty.map(|ty| {
                    ExprOutcome::Value(Operand {
                        mode: Mode::Constant,
                        ty,
                        value: Some(Constant::Int(iota_v as i128)),
                    })
                })
                .unwrap_or(ExprOutcome::invalid())
            }
            _ => {
                if let Some(bf) = BuiltinFunction::from_name(name) {
                    self.region
                        .uses
                        .push((abs_span, ObjectRef::BuiltinFunction(bf)));
                    ExprOutcome::builtin(bf)
                } else if let Some(ty) = self.arena.builtin_type(self.file, name) {
                    if let Some(bname) = builtin_type_static_name(name) {
                        self.region
                            .uses
                            .push((abs_span, ObjectRef::BuiltinType(bname)));
                    }
                    ExprOutcome::Value(Operand {
                        mode: Mode::TypeExpr,
                        ty,
                        value: None,
                    })
                } else {
                    self.report_missing_name(name, self.abs_range(span));
                    ExprOutcome::invalid()
                }
            }
        }
    }

    fn resolve_decl_as_outcome(
        &mut self,
        decl: DeclId<'a>,
        use_span: [u32; 2],
    ) -> ExprOutcome<'a, 'b> {
        let obj = decl_object(decl);
        self.region.uses.push((use_span, obj));

        if decl.symbol.kind == SymbolKind::Function
            && decl.symbol.generic
            && let Some((header, parsed_sig)) = self.decl_generic_signature(decl)
        {
            return ExprOutcome::PartialGenericFunc {
                decl,
                header,
                parsed_sig,
                explicit_type_args: Vec::new(),
            };
        }
        if decl.symbol.kind == SymbolKind::Type && decl.symbol.generic {
            return ExprOutcome::GenericType(decl);
        }

        let Some(op) = self.decls.operand_of(self.arena, decl) else {
            return ExprOutcome::invalid();
        };
        ExprOutcome::Value(op)
    }

    fn decl_generic_signature(&self, decl: DeclId<'a>) -> Option<(String, Signature)> {
        if !decl.symbol.generic {
            return None;
        }
        let decl_range = decl.symbol.declaration()?;
        let end = decl.symbol.body().map_or(decl_range.end, |b| b.start);
        let header = self
            .arena
            .workspace()
            .source_slice(decl.file, decl_range.start..end)?;
        let sig = syntax::parse_signature(&header);
        if sig.type_parameters.is_empty() {
            return None;
        }
        Some((header, sig))
    }

    fn check_unary(&mut self, operator: &Range<usize>, operand: ExprId) -> ExprOutcome<'a, 'b> {
        let Some(int_bits) = self.int_bits() else {
            return ExprOutcome::invalid();
        };

        let op_str = self.text(operator);
        let Some(op) = self.check_expr(operand, None) else {
            return ExprOutcome::invalid();
        };
        match op_str {
            "<-" => match self.arena.receive_slot(op, 0, 1) {
                Some(recv_op) => ExprOutcome::Value(recv_op),
                None => ExprOutcome::invalid(),
            },
            "*" => {
                if op.mode == Mode::TypeExpr {
                    match self.arena.pointer(op.ty) {
                        Some(ptr_ty) => ExprOutcome::Value(Operand {
                            mode: Mode::TypeExpr,
                            ty: ptr_ty,
                            value: None,
                        }),
                        None => ExprOutcome::invalid(),
                    }
                } else {
                    match self.arena.indirect(op) {
                        Some(ind_op) => ExprOutcome::Value(ind_op),
                        None => ExprOutcome::invalid(),
                    }
                }
            }
            "&" => {
                let is_composite = self
                    .body
                    .expression(self.unwrap_parens(operand))
                    .is_some_and(|e| matches!(e.kind, ExprKind::Composite { .. }));
                if op.mode == Mode::Variable || is_composite {
                    match self.arena.pointer(op.ty) {
                        Some(ptr_ty) => ExprOutcome::Value(Operand {
                            mode: Mode::Value,
                            ty: ptr_ty,
                            value: None,
                        }),
                        None => ExprOutcome::invalid(),
                    }
                } else {
                    ExprOutcome::invalid()
                }
            }
            _ => match self.arena.unary_expr(op_str, op, int_bits) {
                Some(res) => ExprOutcome::Value(res),
                None => ExprOutcome::invalid(),
            },
        }
    }

    fn check_binary(
        &mut self,
        left: ExprId,
        operator: &Range<usize>,
        right: ExprId,
        expected: Option<TypeId>,
    ) -> ExprOutcome<'a, 'b> {
        let Some(int_bits) = self.int_bits() else {
            return ExprOutcome::invalid();
        };

        let op_str = self.text(operator);
        let left_op = self.check_expr(left, None);
        let right_op = self.check_expr(right, None);
        let (Some(l), Some(r)) = (left_op, right_op) else {
            return ExprOutcome::invalid();
        };
        if l.mode == Mode::TypeExpr || r.mode == Mode::TypeExpr {
            return ExprOutcome::invalid();
        }
        match self.arena.binary_expr(op_str, l, r, expected, int_bits) {
            Some(res) => ExprOutcome::Value(res),
            None => ExprOutcome::invalid(),
        }
    }

    fn check_selector(&mut self, operand: ExprId, field: &Range<usize>) -> ExprOutcome<'a, 'b> {
        let field_name = self.text(field);
        let field_span = self.abs_span(field);

        match self.check_expr_outcome(operand, None) {
            ExprOutcome::Package { alias, .. } => {
                match self.resolve_imported_decl(alias, field_name) {
                    Some(d) => self.resolve_decl_as_outcome(d, field_span),
                    None => {
                        let qualifier_start = self
                            .body
                            .expression(operand)
                            .map_or(field_span[0] as usize, |expr| {
                                self.body_base + expr.span.start
                            });
                        self.report_missing_imported_name(
                            alias,
                            field_name,
                            qualifier_start,
                            self.abs_range(field),
                        );
                        ExprOutcome::invalid()
                    }
                }
            }
            ExprOutcome::Value(op) if op.mode.is_value() || op.mode == Mode::TypeExpr => {
                if op.mode == Mode::TypeExpr {
                    match self.arena.lookup_method_expr(op.ty, self.file, field_name) {
                        Some(Some(sel)) => {
                            let ty = sel.ty;
                            self.region.uses.push((field_span, sel.obj));
                            self.region.selections.push((field_span, sel));
                            ExprOutcome::Value(Operand {
                                mode: Mode::Value,
                                ty,
                                value: None,
                            })
                        }
                        _ => ExprOutcome::invalid(),
                    }
                } else {
                    let addressable = op.mode == Mode::Variable;
                    match self.arena.lookup_field_or_method(
                        op.ty,
                        addressable,
                        self.file,
                        field_name,
                    ) {
                        Some(Some(sel)) => {
                            let ty = sel.ty;
                            let mode = match sel.obj {
                                ObjectRef::Field(_) if addressable || sel.indirect => {
                                    Mode::Variable
                                }
                                _ => Mode::Value,
                            };
                            self.region.uses.push((field_span, sel.obj));
                            self.region.selections.push((field_span, sel));
                            ExprOutcome::Value(Operand {
                                mode,
                                ty,
                                value: None,
                            })
                        }
                        _ => ExprOutcome::invalid(),
                    }
                }
            }
            _ => ExprOutcome::invalid(),
        }
    }

    fn check_func_literal(
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
            let snap = self.closure_snapshot();
            let mut child = RegionChecker::new(
                self.arena,
                self.decls,
                self.file,
                self.source,
                abs_body.start,
                &child_tree,
                self.region,
                self.iota,
                self.cursor_offset,
            );
            child.restore_closure_snapshot(snap);
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

    fn check_composite(
        &mut self,
        span: &Range<usize>,
        literal_type: Option<ExprId>,
        elements: &[ExprId],
        expected: Option<TypeId>,
    ) -> ExprOutcome<'a, 'b> {
        let Some(int_bits) = self.int_bits() else {
            return ExprOutcome::invalid();
        };

        if elements.len() > crate::analysis::budget::MAX_NODES {
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
            let mut next_idx: u64 = 0;
            let mut max_len: u64 = 0;
            let mut seen_indices = BTreeSet::new();
            let mut ok = true;

            for &el_id in elements {
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
                        .assign_value(val_op, elem_ty, int_bits)
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
            return self
                .arena
                .array(elem_ty, max_len)
                .map(|ty| {
                    ExprOutcome::Value(Operand {
                        mode: Mode::Value,
                        ty,
                        value: None,
                    })
                })
                .unwrap_or(ExprOutcome::invalid());
        }

        let Some(target_ty) = container_ty else {
            self.check_unknown_composite_elements(elements);
            return ExprOutcome::invalid();
        };
        self.region.record_literal(self.abs_span(span), target_ty);

        if let Some((key_ty_opt, elem_ty)) = self.arena.composite_container_types(target_ty) {
            let mut ok = true;
            if let Some(key_ty) = key_ty_opt {
                if self.arena.constructor_map_keys(target_ty).is_none() {
                    ok = false;
                }
                let mut seen_int_keys = BTreeSet::new();
                for &el_id in elements {
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
                            .assign_value(key_op, key_ty, int_bits)
                            .unwrap_or(false)
                        {
                            ok = false;
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
                            .assign_value(val_op, elem_ty, int_bits)
                            .unwrap_or(false)
                        {
                            ok = false;
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
                            .assign_value(val_op, elem_ty, int_bits)
                            .unwrap_or(false)
                        {
                            ok = false;
                        }
                    } else {
                        ok = false;
                    }
                }
            }

            if !ok {
                return ExprOutcome::invalid();
            }
            return ExprOutcome::Value(Operand {
                mode: Mode::Value,
                ty: target_ty,
                value: None,
            });
        }

        if self.arena.is_struct(target_ty) == Some(true) {
            let mut pos_fields: Option<Vec<TypeId>> = None;
            let mut pos_fields_loaded = false;
            let mut has_keyed = false;
            let mut has_unkeyed = false;
            let mut seen_fields = BTreeSet::new();
            let mut seen_paths: Vec<crate::analysis::types::PromotionIndices> = Vec::new();
            let mut ok = true;

            for (idx, &el_id) in elements.iter().enumerate() {
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
                        self.region.uses.push((abs_key, selection.obj));
                        self.region.selections.push((abs_key, selection));
                        if let Some(val_op) = self.check_expr(val_id, Some(fty)) {
                            if !self
                                .arena
                                .assign_value(val_op, fty, int_bits)
                                .unwrap_or(false)
                            {
                                ok = false;
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
                        if let Some(fty) = expected_field
                            && !self
                                .arena
                                .assign_value(val_op, fty, int_bits)
                                .unwrap_or(false)
                        {
                            ok = false;
                        }
                    } else {
                        ok = false;
                    }
                }
            }
            if has_keyed && has_unkeyed {
                ok = false;
            }
            if has_unkeyed
                && pos_fields
                    .as_ref()
                    .is_none_or(|pf| elements.len() != pf.len())
            {
                ok = false;
            }
            if !ok {
                return ExprOutcome::invalid();
            }
            return ExprOutcome::Value(Operand {
                mode: Mode::Value,
                ty: target_ty,
                value: None,
            });
        }

        self.check_unknown_composite_elements(elements);
        ExprOutcome::invalid()
    }

    fn check_unknown_composite_elements(&mut self, elements: &[ExprId]) {
        for &el_id in elements {
            let target = match self.body.expression(el_id).map(|e| &e.kind) {
                Some(ExprKind::KeyValue { key, value }) => {
                    if !matches!(
                        self.body.expression(*key).map(|k| &k.kind),
                        Some(ExprKind::Name)
                    ) {
                        let _ = self.check_expr_outcome(*key, None);
                    }
                    *value
                }
                _ => el_id,
            };
            let _ = self.check_expr_outcome(target, None);
        }
    }

    fn check_index(
        &mut self,
        index_span: &Range<usize>,
        operand: ExprId,
        arguments: &[ExprId],
    ) -> ExprOutcome<'a, 'b> {
        let Some(int_bits) = self.int_bits() else {
            return ExprOutcome::invalid();
        };

        let ident_span = self
            .callee_ident_span(operand)
            .unwrap_or_else(|| self.abs_span(index_span));
        match self.check_expr_outcome(operand, None) {
            ExprOutcome::PartialGenericFunc {
                decl,
                header,
                parsed_sig,
                explicit_type_args,
            } => {
                if !explicit_type_args.is_empty() {
                    return ExprOutcome::invalid();
                }
                let mut type_args = Vec::with_capacity(arguments.len());
                let mut valid = !arguments.is_empty();
                for &arg_id in arguments {
                    match self.check_expr_outcome(arg_id, None) {
                        ExprOutcome::Value(op)
                            if op.mode == Mode::TypeExpr
                                && self.arena.validate_type(op.ty).is_some() =>
                        {
                            type_args.push(op.ty);
                        }
                        _ => valid = false,
                    }
                }
                if !valid
                    || type_args.is_empty()
                    || type_args.len() > parsed_sig.type_parameters.len()
                {
                    return ExprOutcome::invalid();
                }
                if type_args.len() < parsed_sig.type_parameters.len() {
                    return ExprOutcome::PartialGenericFunc {
                        decl,
                        header,
                        parsed_sig,
                        explicit_type_args: type_args,
                    };
                }
                match self
                    .arena
                    .instantiated_signature(
                        decl.file,
                        &header,
                        decl.symbol.declaration().map(|span| span.start),
                        &parsed_sig,
                        &type_args,
                    )
                    .filter(|&ty| self.arena.validate_type(ty).is_some())
                {
                    Some(ty) => {
                        self.region.instances.push((
                            ident_span,
                            InstanceInfo {
                                target: decl_object(decl),
                                type_args: type_args.into_boxed_slice(),
                                instantiated: ty,
                            },
                        ));
                        ExprOutcome::Value(Operand {
                            mode: Mode::Value,
                            ty,
                            value: None,
                        })
                    }
                    None => ExprOutcome::invalid(),
                }
            }
            ExprOutcome::GenericType(decl) => {
                let mut type_args = Vec::with_capacity(arguments.len());
                let mut valid = !arguments.is_empty();
                for &arg_id in arguments {
                    match self.check_expr_outcome(arg_id, None) {
                        ExprOutcome::Value(arg_op)
                            if arg_op.mode == Mode::TypeExpr
                                && self.arena.validate_type(arg_op.ty).is_some() =>
                        {
                            type_args.push(arg_op.ty);
                        }
                        _ => valid = false,
                    }
                }
                if !valid {
                    return ExprOutcome::invalid();
                }
                if let Some(inst_ty) = self
                    .arena
                    .push_named(decl, &type_args, decl.symbol.alias)
                    .filter(|&ty| self.arena.validate_type(ty).is_some())
                {
                    let final_ty = if decl.symbol.alias {
                        self.arena.unalias(inst_ty)
                    } else {
                        Some(inst_ty)
                    };
                    if let Some(final_ty) = final_ty {
                        self.region.instances.push((
                            ident_span,
                            InstanceInfo {
                                target: decl_object(decl),
                                type_args: type_args.into_boxed_slice(),
                                instantiated: final_ty,
                            },
                        ));
                        return ExprOutcome::Value(Operand {
                            mode: Mode::TypeExpr,
                            ty: final_ty,
                            value: None,
                        });
                    }
                }
                ExprOutcome::invalid()
            }
            ExprOutcome::Value(op) if op.mode == Mode::TypeExpr => {
                let mut type_args = Vec::with_capacity(arguments.len());
                let mut valid = !arguments.is_empty();
                for &arg_id in arguments {
                    match self.check_expr_outcome(arg_id, None) {
                        ExprOutcome::Value(arg_op)
                            if arg_op.mode == Mode::TypeExpr
                                && self.arena.validate_type(arg_op.ty).is_some() =>
                        {
                            type_args.push(arg_op.ty);
                        }
                        _ => valid = false,
                    }
                }
                if !valid {
                    return ExprOutcome::invalid();
                }
                if let Some((decl, _existing, is_alias)) = self.arena.named_declaration(op.ty)
                    && let Some(inst_ty) = self
                        .arena
                        .push_named(decl, &type_args, is_alias)
                        .filter(|&ty| self.arena.validate_type(ty).is_some())
                {
                    let final_ty = if is_alias {
                        self.arena.unalias(inst_ty)
                    } else {
                        Some(inst_ty)
                    };
                    if let Some(final_ty) = final_ty {
                        self.region.instances.push((
                            ident_span,
                            InstanceInfo {
                                target: decl_object(decl),
                                type_args: type_args.into_boxed_slice(),
                                instantiated: final_ty,
                            },
                        ));
                        return ExprOutcome::Value(Operand {
                            mode: Mode::TypeExpr,
                            ty: final_ty,
                            value: None,
                        });
                    }
                }
                ExprOutcome::invalid()
            }
            ExprOutcome::Value(op) if op.mode.is_value() || op.mode == Mode::TypeExpr => {
                if arguments.len() != 1 {
                    for &arg_id in arguments {
                        let _ = self.check_expr_outcome(arg_id, None);
                    }
                    return ExprOutcome::invalid();
                }
                let Some(idx_op) = self.check_expr(arguments[0], None) else {
                    return ExprOutcome::invalid();
                };
                match self.arena.index(op, idx_op, 0, 1, int_bits) {
                    Some(res) => ExprOutcome::Value(res),
                    None => ExprOutcome::invalid(),
                }
            }
            _ => {
                for &arg_id in arguments {
                    let _ = self.check_expr_outcome(arg_id, None);
                }
                ExprOutcome::invalid()
            }
        }
    }

    fn check_slice(
        &mut self,
        operand: ExprId,
        low: Option<ExprId>,
        high: Option<ExprId>,
        max: Option<ExprId>,
        full: bool,
    ) -> ExprOutcome<'a, 'b> {
        let Some(int_bits) = self.int_bits() else {
            return ExprOutcome::invalid();
        };

        let op = self.check_expr(operand, None);
        let low_op = low.and_then(|id| self.check_expr(id, None));
        let high_op = high.and_then(|id| self.check_expr(id, None));
        let max_op = max.and_then(|id| self.check_expr(id, None));
        let Some(input) = op else {
            return ExprOutcome::invalid();
        };
        match self
            .arena
            .slice(input, [low_op, high_op, max_op], full, int_bits)
        {
            Some(res) => ExprOutcome::Value(res),
            None => ExprOutcome::invalid(),
        }
    }

    fn check_type_assertion(
        &mut self,
        operand: ExprId,
        asserted_type: &Range<usize>,
    ) -> ExprOutcome<'a, 'b> {
        let Some(int_bits) = self.int_bits() else {
            return ExprOutcome::invalid();
        };

        let src = self.check_expr(operand, None);
        let target = self.resolve_type_syntax_span(asserted_type);
        let (Some(src), Some(target)) = (src, target) else {
            return ExprOutcome::invalid();
        };
        match self.arena.assertion(src, target, 0, 1, int_bits) {
            Some(res) => ExprOutcome::Value(res),
            None => ExprOutcome::invalid(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn build_call_site_info(
        &self,
        call_span: &Range<usize>,
        function: ExprId,
        arguments: &[ExprId],
        callee_obj: Option<ObjectRef<'a>>,
        signature: Option<TypeId>,
        is_conversion: bool,
        is_const_builtin: bool,
    ) -> CallSiteInfo<'a> {
        let abs_call = self.abs_span(call_span);
        let callee_span = self
            .body
            .expression(function)
            .map(|e| self.abs_span(&e.span))
            .unwrap_or([abs_call[0], abs_call[0]]);
        let callee_ident_span = self.callee_ident_span(function);
        let open_paren = self
            .source
            .get(callee_span[1] as usize..abs_call[1] as usize)
            .and_then(|tail| tail.find('('))
            .map(|rel| callee_span[1] + rel as u32)
            .unwrap_or(callee_span[1]);
        let arg_spans = arguments
            .iter()
            .filter_map(|&id| self.body.expression(id).map(|e| self.abs_span(&e.span)))
            .collect();

        CallSiteInfo {
            span: abs_call,
            callee_span,
            callee_ident_span,
            open_paren,
            arg_spans,
            callee_obj,
            signature,
            is_conversion,
            is_const_builtin,
        }
    }

    fn callee_obj_at(&self, function: ExprId) -> Option<ObjectRef<'a>> {
        let span = self.callee_ident_span(function)?;
        self.region
            .uses
            .iter()
            .rev()
            .find(|(sp, _)| *sp == span)
            .map(|(_, obj)| *obj)
            .or_else(|| {
                self.region
                    .selections
                    .iter()
                    .rev()
                    .find(|(sp, _)| *sp == span)
                    .map(|(_, sel)| sel.obj)
            })
    }

    fn finish_conversion_call(
        &mut self,
        function: ExprId,
        target_ty: TypeId,
        arguments: &[ExprId],
        variadic: bool,
        call_span: &Range<usize>,
        int_bits: u32,
    ) -> ExprOutcome<'a, 'b> {
        let info = self.build_call_site_info(
            call_span,
            function,
            arguments,
            self.callee_obj_at(function),
            Some(target_ty),
            true,
            false,
        );
        self.region.calls.push(info);
        if arguments.len() != 1 || variadic {
            for &arg_id in arguments {
                let _ = self.check_expr_outcome(arg_id, None);
            }
            return ExprOutcome::invalid();
        }
        let Some(arg_op) = self.check_expr(arguments[0], None) else {
            return ExprOutcome::invalid();
        };
        match self.arena.convert(arg_op, target_ty, int_bits) {
            Some(res) => ExprOutcome::Value(res),
            None => ExprOutcome::invalid(),
        }
    }

    fn check_call_single(
        &mut self,
        function: ExprId,
        arguments: &[ExprId],
        variadic: bool,
        call_span: &Range<usize>,
    ) -> ExprOutcome<'a, 'b> {
        let Some(int_bits) = self.int_bits() else {
            return ExprOutcome::invalid();
        };

        match self.check_expr_outcome(function, None) {
            ExprOutcome::Value(Operand {
                mode: Mode::Builtin(bf),
                ..
            }) => self.check_builtin_call(bf, function, arguments, variadic, call_span),
            ExprOutcome::Value(callee) if callee.mode == Mode::TypeExpr => self
                .finish_conversion_call(
                    function, callee.ty, arguments, variadic, call_span, int_bits,
                ),
            ExprOutcome::PartialGenericFunc {
                decl,
                header,
                parsed_sig,
                explicit_type_args,
            } => {
                let Some(callee_span) = self.callee_ident_span(function) else {
                    for &arg_id in arguments {
                        let _ = self.check_expr_outcome(arg_id, None);
                    }
                    let info = self.build_call_site_info(
                        call_span,
                        function,
                        arguments,
                        Some(decl_object(decl)),
                        None,
                        false,
                        false,
                    );
                    self.region.calls.push(info);
                    return ExprOutcome::invalid();
                };
                let Some((inst_ty, actual_ops)) = self.instantiate_generic_call(
                    decl,
                    callee_span,
                    &header,
                    &parsed_sig,
                    &explicit_type_args,
                    arguments,
                    variadic,
                ) else {
                    let info = self.build_call_site_info(
                        call_span,
                        function,
                        arguments,
                        Some(decl_object(decl)),
                        None,
                        false,
                        false,
                    );
                    self.region.calls.push(info);
                    return ExprOutcome::invalid();
                };
                self.finish_signature_call_with_args(
                    function,
                    inst_ty,
                    Some(decl_object(decl)),
                    arguments,
                    actual_ops,
                    variadic,
                    call_span,
                    0,
                    1,
                )
            }
            ExprOutcome::Value(callee) if callee.mode.is_value() => {
                let obj = self.callee_obj_at(function);
                self.finish_signature_call(
                    function, callee.ty, obj, arguments, variadic, call_span, 0, 1,
                )
            }
            _ => {
                let obj = self.callee_obj_at(function);
                for &arg_id in arguments {
                    let _ = self.check_expr_outcome(arg_id, None);
                }
                if obj.is_some() {
                    let info = self.build_call_site_info(
                        call_span, function, arguments, obj, None, false, false,
                    );
                    self.region.calls.push(info);
                }
                ExprOutcome::invalid()
            }
        }
    }

    fn check_call_multi(
        &mut self,
        function: ExprId,
        arguments: &[ExprId],
        variadic: bool,
        call_span: &Range<usize>,
        expected_count: usize,
    ) -> Option<Vec<Operand>> {
        let int_bits = self.int_bits()?;

        let (callee_ty, actual_ops) =
            self.resolve_callable_with_info(function, arguments, variadic, call_span)?;
        let (_, results, _) = self.arena.signature_parts(callee_ty)?;
        if results.len() != expected_count {
            return None;
        }
        self.evaluate_signature_results(callee_ty, &actual_ops, variadic, expected_count, int_bits)
    }

    fn resolve_callable_with_info(
        &mut self,
        function: ExprId,
        arguments: &[ExprId],
        variadic: bool,
        call_span: &Range<usize>,
    ) -> Option<(TypeId, Vec<Operand>)> {
        match self.check_expr_outcome(function, None) {
            ExprOutcome::Value(op) if op.mode.is_value() => {
                let obj = self.callee_obj_at(function);
                let info = self.build_call_site_info(
                    call_span,
                    function,
                    arguments,
                    obj,
                    Some(op.ty),
                    false,
                    false,
                );
                self.region.calls.push(info);
                let ops = self.evaluate_call_args(op.ty, arguments, variadic)?;
                Some((op.ty, ops))
            }
            ExprOutcome::PartialGenericFunc {
                decl,
                header,
                parsed_sig,
                explicit_type_args,
            } => {
                let Some(callee_span) = self.callee_ident_span(function) else {
                    for &arg_id in arguments {
                        let _ = self.check_expr_outcome(arg_id, None);
                    }
                    let info = self.build_call_site_info(
                        call_span,
                        function,
                        arguments,
                        Some(decl_object(decl)),
                        None,
                        false,
                        false,
                    );
                    self.region.calls.push(info);
                    return None;
                };
                let Some((inst, ops)) = self.instantiate_generic_call(
                    decl,
                    callee_span,
                    &header,
                    &parsed_sig,
                    &explicit_type_args,
                    arguments,
                    variadic,
                ) else {
                    let info = self.build_call_site_info(
                        call_span,
                        function,
                        arguments,
                        Some(decl_object(decl)),
                        None,
                        false,
                        false,
                    );
                    self.region.calls.push(info);
                    return None;
                };
                let info = self.build_call_site_info(
                    call_span,
                    function,
                    arguments,
                    Some(decl_object(decl)),
                    Some(inst),
                    false,
                    false,
                );
                self.region.calls.push(info);
                Some((inst, ops))
            }
            _ => {
                let obj = self.callee_obj_at(function);
                for &arg_id in arguments {
                    let _ = self.check_expr_outcome(arg_id, None);
                }
                if obj.is_some() {
                    let info = self.build_call_site_info(
                        call_span, function, arguments, obj, None, false, false,
                    );
                    self.region.calls.push(info);
                }
                None
            }
        }
    }

    fn evaluate_signature_results(
        &mut self,
        callee_ty: TypeId,
        actual_ops: &[Operand],
        variadic: bool,
        count: usize,
        int_bits: u32,
    ) -> Option<Vec<Operand>> {
        let mut out = Vec::with_capacity(count);
        for result in 0..count {
            let res_ty = self
                .arena
                .call_signature(callee_ty, actual_ops, variadic, result, count, int_bits)?;
            out.push(Operand {
                mode: Mode::Value,
                ty: res_ty,
                value: None,
            });
        }
        Some(out)
    }

    fn check_call_any_arity(
        &mut self,
        function: ExprId,
        arguments: &[ExprId],
        variadic: bool,
        call_span: &Range<usize>,
    ) -> Option<Vec<Operand>> {
        let int_bits = self.int_bits()?;

        let (callee_ty, actual_ops) = match self.check_expr_outcome(function, None) {
            ExprOutcome::Value(Operand {
                mode: Mode::Builtin(bf),
                ..
            }) => {
                let outcome = self.check_builtin_call(bf, function, arguments, variadic, call_span);
                if let ExprOutcome::Value(op) = outcome {
                    self.region.expr_types.push((self.abs_span(call_span), op));
                    return Some(vec![op]);
                }
                return None;
            }
            ExprOutcome::Value(callee) if callee.mode == Mode::TypeExpr => {
                let outcome = self.finish_conversion_call(
                    function, callee.ty, arguments, variadic, call_span, int_bits,
                );
                if let ExprOutcome::Value(op) = outcome {
                    self.region.expr_types.push((self.abs_span(call_span), op));
                    return Some(vec![op]);
                }
                return None;
            }
            ExprOutcome::PartialGenericFunc {
                decl,
                header,
                parsed_sig,
                explicit_type_args,
            } => {
                let Some(callee_span) = self.callee_ident_span(function) else {
                    for &arg_id in arguments {
                        let _ = self.check_expr_outcome(arg_id, None);
                    }
                    let info = self.build_call_site_info(
                        call_span,
                        function,
                        arguments,
                        Some(decl_object(decl)),
                        None,
                        false,
                        false,
                    );
                    self.region.calls.push(info);
                    return None;
                };
                let Some((inst_ty, actual_ops)) = self.instantiate_generic_call(
                    decl,
                    callee_span,
                    &header,
                    &parsed_sig,
                    &explicit_type_args,
                    arguments,
                    variadic,
                ) else {
                    let info = self.build_call_site_info(
                        call_span,
                        function,
                        arguments,
                        Some(decl_object(decl)),
                        None,
                        false,
                        false,
                    );
                    self.region.calls.push(info);
                    return None;
                };
                let info = self.build_call_site_info(
                    call_span,
                    function,
                    arguments,
                    Some(decl_object(decl)),
                    Some(inst_ty),
                    false,
                    false,
                );
                self.region.calls.push(info);
                (inst_ty, actual_ops)
            }
            ExprOutcome::Value(callee) if callee.mode.is_value() => {
                let obj = self.callee_obj_at(function);
                let info = self.build_call_site_info(
                    call_span,
                    function,
                    arguments,
                    obj,
                    Some(callee.ty),
                    false,
                    false,
                );
                self.region.calls.push(info);
                let actual_ops = self.evaluate_call_args(callee.ty, arguments, variadic)?;
                (callee.ty, actual_ops)
            }
            _ => {
                let obj = self.callee_obj_at(function);
                for &arg_id in arguments {
                    let _ = self.check_expr_outcome(arg_id, None);
                }
                if obj.is_some() {
                    let info = self.build_call_site_info(
                        call_span, function, arguments, obj, None, false, false,
                    );
                    self.region.calls.push(info);
                }
                return None;
            }
        };

        let (params, results, is_var) = self.arena.signature_parts(callee_ty)?;
        if results.is_empty() {
            let _ = self.validate_void_call(&params, is_var, &actual_ops, variadic, int_bits);
            return None;
        }
        let count = results.len();
        let out =
            self.evaluate_signature_results(callee_ty, &actual_ops, variadic, count, int_bits)?;
        if out.len() == 1 {
            self.region
                .expr_types
                .push((self.abs_span(call_span), out[0]));
        } else {
            let tys: Vec<TypeId> = out.iter().map(|op| op.ty).collect();
            if let Some(tuple_ty) = self.arena.tuple(&tys) {
                self.region.expr_types.push((
                    self.abs_span(call_span),
                    Operand {
                        mode: Mode::Value,
                        ty: tuple_ty,
                        value: None,
                    },
                ));
            }
        }
        Some(out)
    }

    #[allow(clippy::too_many_arguments)]
    fn instantiate_generic_call(
        &mut self,
        decl: DeclId<'a>,
        callee_span: [u32; 2],
        header: &str,
        parsed_sig: &Signature,
        explicit_type_args: &[TypeId],
        arguments: &[ExprId],
        variadic: bool,
    ) -> Option<(TypeId, Vec<Operand>)> {
        let int_bits = self.int_bits()?;

        let actual_ops = self.eval_call_arg_operands(arguments, variadic, None)?;
        let inferred = self.arena.infer_call_arguments(
            decl.file,
            header,
            parsed_sig,
            &actual_ops,
            explicit_type_args,
            int_bits,
            variadic,
        )?;
        let inst_ty = self.arena.instantiated_signature(
            decl.file,
            header,
            decl.symbol.declaration().map(|span| span.start),
            parsed_sig,
            &inferred,
        )?;
        self.region.instances.push((
            callee_span,
            InstanceInfo {
                target: decl_object(decl),
                type_args: inferred.into_boxed_slice(),
                instantiated: inst_ty,
            },
        ));
        Some((inst_ty, actual_ops))
    }

    fn eval_call_arg_operands(
        &mut self,
        arguments: &[ExprId],
        variadic: bool,
        param_hints: Option<&[TypeId]>,
    ) -> Option<Vec<Operand>> {
        if arguments.len() == 1
            && !variadic
            && let Some(expr) = self.body.expression(arguments[0])
            && let ExprKind::Call {
                function,
                arguments: ref inner_args,
                variadic: inner_var,
            } = expr.kind
        {
            let inner_args = inner_args.clone();
            let call_span = expr.span.clone();
            return self.check_call_any_arity(function, &inner_args, inner_var, &call_span);
        }
        let mut ops = Vec::with_capacity(arguments.len());
        let mut ok = true;
        for (idx, &arg_id) in arguments.iter().enumerate() {
            let expected =
                param_hints.and_then(|p| if variadic { None } else { p.get(idx).copied() });
            match self.check_expr(arg_id, expected) {
                Some(op) => ops.push(op),
                None => ok = false,
            }
        }
        ok.then_some(ops)
    }

    fn evaluate_call_args(
        &mut self,
        callee_ty: TypeId,
        arguments: &[ExprId],
        variadic: bool,
    ) -> Option<Vec<Operand>> {
        let (params, _, _) = self.arena.signature_parts(callee_ty)?;
        self.eval_call_arg_operands(arguments, variadic, Some(&params))
    }

    #[allow(clippy::too_many_arguments)]
    fn finish_signature_call(
        &mut self,
        function: ExprId,
        callee_ty: TypeId,
        callee_obj: Option<ObjectRef<'a>>,
        arguments: &[ExprId],
        variadic: bool,
        call_span: &Range<usize>,
        result: usize,
        count: usize,
    ) -> ExprOutcome<'a, 'b> {
        let Some(int_bits) = self.int_bits() else {
            return ExprOutcome::invalid();
        };

        let info = self.build_call_site_info(
            call_span,
            function,
            arguments,
            callee_obj,
            Some(callee_ty),
            false,
            false,
        );
        self.region.calls.push(info);

        let Some((params, results, is_var)) = self.arena.signature_parts(callee_ty) else {
            for &arg_id in arguments {
                let _ = self.check_expr_outcome(arg_id, None);
            }
            return ExprOutcome::invalid();
        };

        let Some(actual_ops) = self.evaluate_call_args(callee_ty, arguments, variadic) else {
            return ExprOutcome::invalid();
        };

        if results.is_empty() {
            if !self.validate_void_call(&params, is_var, &actual_ops, variadic, int_bits) {
                return ExprOutcome::invalid();
            }
            return ExprOutcome::no_value();
        }

        match self
            .arena
            .call_signature(callee_ty, &actual_ops, variadic, result, count, int_bits)
        {
            Some(res_ty) => ExprOutcome::Value(Operand {
                mode: Mode::Value,
                ty: res_ty,
                value: None,
            }),
            None => ExprOutcome::invalid(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn finish_signature_call_with_args(
        &mut self,
        function: ExprId,
        callee_ty: TypeId,
        callee_obj: Option<ObjectRef<'a>>,
        arguments: &[ExprId],
        actual_ops: Vec<Operand>,
        variadic: bool,
        call_span: &Range<usize>,
        result: usize,
        count: usize,
    ) -> ExprOutcome<'a, 'b> {
        let Some(int_bits) = self.int_bits() else {
            return ExprOutcome::invalid();
        };

        let info = self.build_call_site_info(
            call_span,
            function,
            arguments,
            callee_obj,
            Some(callee_ty),
            false,
            false,
        );
        self.region.calls.push(info);

        let Some((params, results, is_var)) = self.arena.signature_parts(callee_ty) else {
            return ExprOutcome::invalid();
        };

        if results.is_empty() {
            if !self.validate_void_call(&params, is_var, &actual_ops, variadic, int_bits) {
                return ExprOutcome::invalid();
            }
            return ExprOutcome::no_value();
        }

        match self
            .arena
            .call_signature(callee_ty, &actual_ops, variadic, result, count, int_bits)
        {
            Some(res_ty) => ExprOutcome::Value(Operand {
                mode: Mode::Value,
                ty: res_ty,
                value: None,
            }),
            None => ExprOutcome::invalid(),
        }
    }

    fn validate_void_call(
        &mut self,
        params: &[TypeId],
        sig_variadic: bool,
        actuals: &[Operand],
        call_spread: bool,
        int_bits: u32,
    ) -> bool {
        if sig_variadic {
            if params.is_empty() {
                return false;
            }
            let fixed = params.len() - 1;
            if call_spread {
                if actuals.len() != params.len() {
                    return false;
                }
                for (&actual, &target) in actuals.iter().zip(params.iter()) {
                    if !self
                        .arena
                        .assign_value(actual, target, int_bits)
                        .unwrap_or(false)
                    {
                        return false;
                    }
                }
                return true;
            }
            if actuals.len() < fixed {
                return false;
            }
            for (&actual, &target) in actuals[..fixed].iter().zip(params[..fixed].iter()) {
                if !self
                    .arena
                    .assign_value(actual, target, int_bits)
                    .unwrap_or(false)
                {
                    return false;
                }
            }
            let elem_ty = self
                .arena
                .composite_container_types(params[fixed])
                .map(|(_, elem)| elem)
                .unwrap_or(params[fixed]);
            for &actual in &actuals[fixed..] {
                if !self
                    .arena
                    .assign_value(actual, elem_ty, int_bits)
                    .unwrap_or(false)
                {
                    return false;
                }
            }
            true
        } else {
            if call_spread || actuals.len() != params.len() {
                return false;
            }
            actuals.iter().zip(params.iter()).all(|(&actual, &target)| {
                self.arena
                    .assign_value(actual, target, int_bits)
                    .unwrap_or(false)
            })
        }
    }

    fn check_builtin_call(
        &mut self,
        bf: BuiltinFunction,
        function: ExprId,
        arguments: &[ExprId],
        variadic: bool,
        call_span: &Range<usize>,
    ) -> ExprOutcome<'a, 'b> {
        let Some(int_bits) = self.int_bits() else {
            return ExprOutcome::invalid();
        };

        let outcome = match bf {
            BuiltinFunction::Len | BuiltinFunction::Cap => {
                if arguments.len() != 1 || variadic {
                    for &arg_id in arguments {
                        let _ = self.check_expr_outcome(arg_id, None);
                    }
                    ExprOutcome::invalid()
                } else {
                    let has_call = self.expr_has_call(arguments[0]);
                    match self.check_expr(arguments[0], None) {
                        Some(arg_op) => match self.arena.builtin_length(
                            arg_op,
                            bf == BuiltinFunction::Cap,
                            has_call,
                            int_bits,
                        ) {
                            Some(op) => ExprOutcome::Value(op),
                            None => ExprOutcome::invalid(),
                        },
                        None => ExprOutcome::invalid(),
                    }
                }
            }
            BuiltinFunction::New => {
                if arguments.len() != 1 || variadic {
                    for &arg_id in arguments {
                        let _ = self.check_expr_outcome(arg_id, None);
                    }
                    ExprOutcome::invalid()
                } else {
                    match self.check_expr_outcome(arguments[0], None) {
                        ExprOutcome::Value(op) if op.mode == Mode::TypeExpr => {
                            match self.arena.builtin_new(op.ty) {
                                Some(res) => ExprOutcome::Value(res),
                                None => ExprOutcome::invalid(),
                            }
                        }
                        ExprOutcome::Value(op)
                            if op.mode.is_value() || op.mode == Mode::TypeExpr =>
                        {
                            match self
                                .arena
                                .default_operand(op, int_bits)
                                .and_then(|ty| self.arena.builtin_new(ty))
                            {
                                Some(res) => ExprOutcome::Value(res),
                                None => ExprOutcome::invalid(),
                            }
                        }
                        _ => ExprOutcome::invalid(),
                    }
                }
            }
            BuiltinFunction::Make => {
                if arguments.is_empty() || variadic {
                    for &arg_id in arguments {
                        let _ = self.check_expr_outcome(arg_id, None);
                    }
                    ExprOutcome::invalid()
                } else {
                    let target_ty = match self.check_expr_outcome(arguments[0], None) {
                        ExprOutcome::Value(op) if op.mode == Mode::TypeExpr => Some(op.ty),
                        _ => None,
                    };
                    let mut sizes = Vec::with_capacity(arguments.len().saturating_sub(1));
                    let mut sizes_ok = true;
                    for &arg_id in &arguments[1..] {
                        match self.check_expr(arg_id, None) {
                            Some(op) => sizes.push(op),
                            None => sizes_ok = false,
                        }
                    }
                    if let (Some(ty), true) = (target_ty, sizes_ok) {
                        match self.arena.builtin_make(ty, &sizes, int_bits) {
                            Some(res) => ExprOutcome::Value(res),
                            None => ExprOutcome::invalid(),
                        }
                    } else {
                        ExprOutcome::invalid()
                    }
                }
            }
            BuiltinFunction::Append => {
                if let Some(args) = self.eval_call_arg_operands(arguments, variadic, None) {
                    match self.arena.builtin_append(&args, variadic, int_bits) {
                        Some(res) => ExprOutcome::Value(res),
                        None => ExprOutcome::invalid(),
                    }
                } else {
                    ExprOutcome::invalid()
                }
            }
            BuiltinFunction::Copy => {
                if !variadic
                    && let Some(args) = self.eval_call_arg_operands(arguments, false, None)
                    && args.len() == 2
                {
                    match self.arena.builtin_copy(args[0], args[1], int_bits) {
                        Some(res) => ExprOutcome::Value(res),
                        None => ExprOutcome::invalid(),
                    }
                } else {
                    if variadic {
                        for &arg_id in arguments {
                            let _ = self.check_expr_outcome(arg_id, None);
                        }
                    }
                    ExprOutcome::invalid()
                }
            }
            BuiltinFunction::Complex => {
                if !variadic
                    && let Some(args) = self.eval_call_arg_operands(arguments, false, None)
                    && args.len() == 2
                {
                    match self.arena.builtin_complex(&args, int_bits) {
                        Some(res) => ExprOutcome::Value(res),
                        None => ExprOutcome::invalid(),
                    }
                } else {
                    if variadic {
                        for &arg_id in arguments {
                            let _ = self.check_expr_outcome(arg_id, None);
                        }
                    }
                    ExprOutcome::invalid()
                }
            }
            BuiltinFunction::Real | BuiltinFunction::Imag => {
                if arguments.len() != 1 || variadic {
                    for &arg_id in arguments {
                        let _ = self.check_expr_outcome(arg_id, None);
                    }
                    ExprOutcome::invalid()
                } else {
                    match self.check_expr(arguments[0], None) {
                        Some(arg_op) => {
                            match self
                                .arena
                                .builtin_component(arg_op, bf == BuiltinFunction::Imag)
                            {
                                Some(res) => ExprOutcome::Value(res),
                                None => ExprOutcome::invalid(),
                            }
                        }
                        None => ExprOutcome::invalid(),
                    }
                }
            }
            BuiltinFunction::Min | BuiltinFunction::Max => {
                if arguments.is_empty() || variadic {
                    for &arg_id in arguments {
                        let _ = self.check_expr_outcome(arg_id, None);
                    }
                    ExprOutcome::invalid()
                } else if let Some(args) = self.eval_call_arg_operands(arguments, false, None)
                    && !args.is_empty()
                {
                    match self
                        .arena
                        .builtin_minmax(&args, bf == BuiltinFunction::Max, int_bits)
                    {
                        Some(res) => ExprOutcome::Value(res),
                        None => ExprOutcome::invalid(),
                    }
                } else {
                    ExprOutcome::invalid()
                }
            }
            BuiltinFunction::Recover => {
                for &arg_id in arguments {
                    let _ = self.check_expr_outcome(arg_id, None);
                }
                if arguments.is_empty() && !variadic {
                    match self.arena.builtin_type(self.file, "any") {
                        Some(ty) => ExprOutcome::Value(Operand {
                            mode: Mode::Value,
                            ty,
                            value: None,
                        }),
                        None => ExprOutcome::invalid(),
                    }
                } else {
                    ExprOutcome::invalid()
                }
            }
            BuiltinFunction::Close
            | BuiltinFunction::Delete
            | BuiltinFunction::Clear
            | BuiltinFunction::Panic
            | BuiltinFunction::Print
            | BuiltinFunction::Println => {
                if matches!(
                    bf,
                    BuiltinFunction::Delete | BuiltinFunction::Print | BuiltinFunction::Println
                ) && arguments.len() == 1
                    && !variadic
                {
                    let _ = self.eval_call_arg_operands(arguments, false, None);
                } else {
                    for &arg_id in arguments {
                        let _ = self.check_expr_outcome(arg_id, None);
                    }
                }
                ExprOutcome::no_value()
            }
        };

        let is_const_builtin = matches!(
            outcome,
            ExprOutcome::Value(op) if op.mode == Mode::Constant
        );
        let info = self.build_call_site_info(
            call_span,
            function,
            arguments,
            Some(ObjectRef::BuiltinFunction(bf)),
            None,
            false,
            is_const_builtin,
        );
        self.region.calls.push(info);
        outcome
    }

    fn callee_ident_span(&self, mut id: ExprId) -> Option<[u32; 2]> {
        for _ in 0..16 {
            let expr = self.body.expression(id)?;
            match &expr.kind {
                ExprKind::Parenthesized { expression } => id = *expression,
                ExprKind::Index { operand, .. } => id = *operand,
                ExprKind::Selector { field, .. } => return Some(self.abs_span(field)),
                ExprKind::Name => return Some(self.abs_span(&expr.span)),
                _ => return None,
            }
        }
        None
    }

    fn expr_has_call(&self, id: ExprId) -> bool {
        let Some(expr) = self.body.expression(id) else {
            return false;
        };
        match &expr.kind {
            ExprKind::Call { .. } => true,
            ExprKind::Parenthesized { expression } => self.expr_has_call(*expression),
            ExprKind::Unary { operand, .. } => self.expr_has_call(*operand),
            ExprKind::Binary { left, right, .. } => {
                self.expr_has_call(*left) || self.expr_has_call(*right)
            }
            ExprKind::Selector { operand, .. } => self.expr_has_call(*operand),
            ExprKind::Index { operand, arguments } => {
                self.expr_has_call(*operand) || arguments.iter().any(|&a| self.expr_has_call(a))
            }
            ExprKind::Slice {
                operand,
                low,
                high,
                max,
                ..
            } => {
                self.expr_has_call(*operand)
                    || low.is_some_and(|e| self.expr_has_call(e))
                    || high.is_some_and(|e| self.expr_has_call(e))
                    || max.is_some_and(|e| self.expr_has_call(e))
            }
            ExprKind::TypeAssertion { operand, .. } => self.expr_has_call(*operand),
            ExprKind::Composite { elements, .. } => elements.iter().any(|&e| self.expr_has_call(e)),
            ExprKind::KeyValue { key, value } => {
                self.expr_has_call(*key) || self.expr_has_call(*value)
            }
            _ => false,
        }
    }
}
