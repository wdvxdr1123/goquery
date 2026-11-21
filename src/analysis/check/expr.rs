//! Primary expression checking (`Name`, unary, binary, selector, slice, assertion).

use std::ops::Range;

use crate::analysis::engine::check::diagnostics::Code;
use crate::analysis::engine::check::{
    ExprOutcome, FileSymbolLookup, RegionChecker, builtin_type_static_name, decl_object,
};
use crate::analysis::engine::obj::{BuiltinFunction, DeclId, ObjectRef};
use crate::analysis::engine::scope::tree::LocalBindingKind;
use crate::analysis::engine::types::{
    BasicKind, CheckError, Constant, Mode, Operand, SliceError, TypeId,
};
use crate::syntax::{ExprId, ExprKind, SymbolKind};

impl<'a, 'b> RegionChecker<'a, 'b> {
    pub(crate) fn check_expr(&mut self, id: ExprId, expected: Option<TypeId>) -> Option<Operand> {
        self.check_expr_outcome(id, expected).operand()
    }

    pub(crate) fn check_multi_expr(
        &mut self,
        id: ExprId,
        expected_count: usize,
    ) -> Option<Vec<Operand>> {
        if expected_count <= 1 {
            return self.check_expr(id, None).map(|op| vec![op]);
        }
        self.check_value_pack(id, expected_count == 2, None)
            .filter(|values| values.len() == expected_count)
    }

    /// Evaluate a sole RHS once and retain the actual arity for diagnostics.
    /// Comma-ok forms only expand in an assignment to exactly two variables.
    pub(super) fn check_value_pack(
        &mut self,
        id: ExprId,
        comma_ok: bool,
        expected: Option<TypeId>,
    ) -> Option<Vec<Operand>> {
        self.int_bits()?;
        let target_id = self.unwrap_parens(id);
        let tree = self.body;
        let outer_span = &tree.expression(id)?.span;
        let target_expr = tree.expression(target_id)?;
        let target_span = &target_expr.span;

        let (ops, recorded) = match &target_expr.kind {
            ExprKind::Call {
                function,
                arguments,
                ellipsis,
                ..
            } => {
                self.region
                    .record_expected(self.abs_span(outer_span), expected);
                if target_id != id {
                    self.region
                        .record_expected(self.abs_span(target_span), expected);
                }
                (
                    self.check_call_any_arity(*function, arguments, *ellipsis, target_span)?,
                    true,
                )
            }
            _ => {
                let mut operand = self.check_expr(id, expected)?;
                if !operand.mode.is_value() {
                    return None;
                }
                if !comma_ok || !matches!(operand.mode, Mode::MapIndex | Mode::CommaOk) {
                    return Some(vec![operand]);
                }
                operand.mode = Mode::Value;
                let ok = Operand {
                    mode: Mode::Value,
                    ty: self.arena.basic(BasicKind::Bool)?,
                    value: None,
                };
                (vec![operand, ok], false)
            }
        };

        if !ops.iter().all(|operand| operand.mode.is_value()) {
            return None;
        }
        if (!recorded || target_id != id)
            && let Some(first) = ops.first()
        {
            if ops.len() > 1 {
                let tuple_ty = match ops.as_slice() {
                    [first, second] => self.arena.tuple(&[first.ty, second.ty]),
                    _ => self
                        .arena
                        .tuple(&ops.iter().map(|op| op.ty).collect::<Vec<_>>()),
                };
                if let Some(tuple_ty) = tuple_ty {
                    let tuple_op = Operand {
                        mode: Mode::Value,
                        ty: tuple_ty,
                        value: None,
                    };
                    if !recorded {
                        self.record_type_and_value(self.abs_span(target_span), tuple_op);
                    }
                    if target_id != id {
                        self.record_type_and_value(self.abs_span(outer_span), tuple_op);
                    }
                }
            } else {
                self.record_type_and_value(self.abs_span(outer_span), *first);
            }
        }
        Some(ops)
    }

    pub(crate) fn check_expr_outcome(
        &mut self,
        id: ExprId,
        expected: Option<TypeId>,
    ) -> ExprOutcome<'a, 'b> {
        let tree = self.body;
        let Some(expr) = tree.expression(id) else {
            return ExprOutcome::invalid();
        };
        let span = &expr.span;
        // Unknown inner contexts are barriers: an enclosing expression's
        // expectation must not leak into unrelated operands or callee names.
        self.region.record_expected(self.abs_span(span), expected);

        let outcome = match &expr.kind {
            ExprKind::Missing => ExprOutcome::invalid(),
            ExprKind::Name => self.check_name(span),
            ExprKind::Literal(kind) => match self.arena.literal(*kind, self.text(span)) {
                Some(op) => ExprOutcome::Value(op),
                None => ExprOutcome::invalid(),
            },
            ExprKind::Type => match self.resolve_type_syntax_span(span) {
                Some(ty) => ExprOutcome::Value(Operand {
                    mode: Mode::TypeExpr,
                    ty,
                    value: None,
                }),
                None => ExprOutcome::invalid(),
            },
            ExprKind::Composite {
                literal_type,
                elements,
            } => self.check_composite(span, *literal_type, elements, expected),
            ExprKind::KeyValue { key, value } => {
                let _ = self.check_expr_outcome(*key, None);
                self.check_expr_outcome(*value, expected)
            }
            ExprKind::FunctionLiteral { signature, body } => {
                self.check_func_literal(signature, body)
            }
            ExprKind::Unary { operator, operand } => self.check_unary(operator, *operand),
            ExprKind::Binary {
                left,
                operator,
                right,
            } => self.check_binary(span, *left, operator, *right, expected),
            ExprKind::Parenthesized { expression } => {
                self.check_expr_outcome(*expression, expected)
            }
            ExprKind::Selector { operand, field } => self.check_selector(*operand, field),
            ExprKind::Call {
                function,
                arguments,
                ellipsis,
                ..
            } => self.check_call_single(*function, arguments, *ellipsis, span),
            ExprKind::Index { operand, arguments } => self.check_index(span, *operand, arguments),
            ExprKind::Slice {
                operand,
                low,
                high,
                max,
                full,
            } => self.check_slice(*operand, *low, *high, *max, *full),
            ExprKind::TypeAssertion {
                operand,
                asserted_type,
            } => self.check_type_assertion(*operand, asserted_type),
            ExprKind::TypeSwitchGuard { operand } => self.check_expr_outcome(*operand, None),
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
                self.record_use(abs_span, obj);
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
            self.record_use(abs_span, pkg_obj);
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
                self.record_use(abs_span, ObjectRef::Nil);
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
                self.record_use(abs_span, ObjectRef::BuiltinConst("true"));
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
                self.record_use(abs_span, ObjectRef::BuiltinConst("false"));
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
                self.record_use(abs_span, ObjectRef::BuiltinConst("iota"));
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
                    self.record_use(abs_span, ObjectRef::BuiltinFunction(bf));
                    ExprOutcome::builtin(bf)
                } else if let Some(ty) = self.arena.builtin_type(self.file, name) {
                    if let Some(bname) = builtin_type_static_name(name) {
                        self.record_use(abs_span, ObjectRef::BuiltinType(bname));
                    }
                    ExprOutcome::Value(Operand {
                        mode: Mode::TypeExpr,
                        ty,
                        value: None,
                    })
                } else if self.report_missing_name(name, self.abs_range(span)) {
                    ExprOutcome::MissingName
                } else {
                    ExprOutcome::invalid()
                }
            }
        }
    }

    pub(super) fn resolve_decl_as_outcome(
        &mut self,
        decl: DeclId<'a>,
        use_span: [u32; 2],
    ) -> ExprOutcome<'a, 'b> {
        let obj = decl_object(decl);
        self.record_use(use_span, obj);

        if decl.symbol.kind == SymbolKind::Type && decl.symbol.generic {
            return ExprOutcome::GenericType(decl);
        }

        let Some(op) = self.decls.operand_of(self.arena, decl) else {
            return ExprOutcome::invalid();
        };
        if decl.symbol.kind == SymbolKind::Function && decl.symbol.generic {
            return ExprOutcome::GenericFunc {
                decl,
                ty: op.ty,
                explicit_type_args: Vec::new(),
            };
        }
        ExprOutcome::Value(op)
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
            "<-" => match self.arena.receive(op) {
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
        span: &Range<usize>,
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
        match self
            .arena
            .checked_binary_expr(op_str, l, r, expected, int_bits)
        {
            Some(Ok(res)) => ExprOutcome::Value(res),
            Some(Err(error)) => {
                self.binary_error(span, (left, l), (right, r), error);
                ExprOutcome::invalid()
            }
            None => ExprOutcome::invalid(),
        }
    }

    fn check_selector(&mut self, operand: ExprId, field: &Range<usize>) -> ExprOutcome<'a, 'b> {
        let field_name = self.text(field);
        let field_span = self.abs_span(field);

        match self.check_expr_outcome(operand, None) {
            ExprOutcome::Package { alias, .. } => {
                match self.resolve_imported_decl(alias, field_name) {
                    Some(d) => {
                        let outcome = self.resolve_decl_as_outcome(d, field_span);
                        if self.decls.invalid_var(d) {
                            ExprOutcome::InvalidImportedVar
                        } else {
                            outcome
                        }
                    }
                    None => {
                        let qualifier_start = self
                            .body
                            .expression(operand)
                            .map_or(field_span[0] as usize, |expr| {
                                self.body_base + expr.span.start
                            });
                        if self.report_missing_imported_name(
                            alias,
                            field_name,
                            qualifier_start,
                            self.abs_range(field),
                        ) {
                            ExprOutcome::MissingName
                        } else {
                            ExprOutcome::invalid()
                        }
                    }
                }
            }
            ExprOutcome::Value(op) if op.mode.is_value() || op.mode == Mode::TypeExpr => {
                if op.mode == Mode::TypeExpr {
                    match self.arena.lookup_method_expr(op.ty, self.file, field_name) {
                        Some(Some(sel)) => {
                            let ty = sel.ty;
                            self.record_use(field_span, sel.obj);
                            self.record_selection(field_span, sel);
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
                            let mode = match sel.obj {
                                ObjectRef::Field(_) if addressable || sel.indirect => {
                                    Mode::Variable
                                }
                                _ => Mode::Value,
                            };
                            let ty = sel.ty;
                            self.record_use(field_span, sel.obj);
                            self.record_selection(field_span, sel);
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
        let sites = [low, high, max];
        let indices = sites.map(|site| {
            site.map(|id| {
                self.check_expr(id, None)
                    .unwrap_or(Operand::non_value(Mode::Invalid))
            })
        });
        let Some(input) = op.filter(|op| op.mode.is_value()) else {
            return ExprOutcome::invalid();
        };
        match self.arena.slice(input, indices, full, int_bits) {
            Ok(result) => {
                for ((site, value), error) in sites.into_iter().zip(indices).zip(result.errors) {
                    if let (Some(site), Some(value), Some(error)) = (site, value, error) {
                        self.index_error((operand, input), (site, value), error);
                    }
                }
                if let Some((position, low, high)) = result.swapped
                    && let Some(expr) = sites[position].and_then(|id| self.body.expression(id))
                {
                    self.compiler_error(
                        &expr.span,
                        Code::SwappedSliceIndices,
                        &format!("invalid slice indices: {high} < {low}"),
                    );
                }
                result
                    .operand
                    .map_or_else(ExprOutcome::invalid, ExprOutcome::Value)
            }
            Err(CheckError::Invalid(SliceError::StringFull)) => {
                if let Some(expr) = max.and_then(|id| self.body.expression(id)) {
                    self.compiler_error(
                        &expr.span,
                        Code::InvalidSliceExpr,
                        "invalid operation: 3-index slice of string",
                    );
                }
                ExprOutcome::invalid()
            }
            Err(CheckError::Invalid(error)) => {
                if let Some(expr) = self.body.expression(operand)
                    && let Some(value) = self.operand_description(operand, input)
                {
                    let qualifier = if matches!(error, SliceError::Unaddressable) {
                        "unaddressable value "
                    } else {
                        ""
                    };
                    self.compiler_error(
                        &expr.span,
                        Code::NonSliceableOperand,
                        &format!("cannot slice {qualifier}{value}"),
                    );
                }
                ExprOutcome::invalid()
            }
            Err(CheckError::Unknown) => ExprOutcome::invalid(),
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
        match self.arena.assertion(src, target, int_bits) {
            Some(res) => ExprOutcome::Value(res),
            None => ExprOutcome::invalid(),
        }
    }

    pub(crate) fn unwrap_parens(&self, mut id: ExprId) -> ExprId {
        while let Some(inner) = self.body.expression(id).and_then(|e| match &e.kind {
            ExprKind::Parenthesized { expression } => Some(*expression),
            _ => None,
        }) {
            id = inner;
        }
        id
    }

    pub(super) fn expr_has_call(&self, id: ExprId) -> bool {
        let Some(expr) = self.body.expression(id) else {
            return false;
        };
        match &expr.kind {
            ExprKind::Call { .. } => true,
            ExprKind::Parenthesized { expression } => self.expr_has_call(*expression),
            ExprKind::Unary { operand, operator } => {
                self.text(operator) == "<-" || self.expr_has_call(*operand)
            }
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
