//! Call, index, conversion, and generic instantiation checking for [`RegionChecker`].

use std::num::NonZeroU32;
use std::ops::Range;

use crate::analysis::engine::check::diagnostics::Code;
use crate::analysis::engine::check::region::{CallSiteInfo, InstanceInfo, RecordMask};
use crate::analysis::engine::check::{ExprOutcome, RegionChecker, decl_object};
use crate::analysis::engine::obj::{DeclId, ObjectRef};
use crate::analysis::engine::scope::tree::ScopeKind;
use crate::analysis::engine::types::{CheckError, Mode, Operand, TypeId};
use crate::syntax::{ExprId, ExprKind};

impl<'a, 'b> RegionChecker<'a, 'b> {
    pub(super) fn check_index(
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
            ExprOutcome::GenericFunc {
                decl,
                ty: generic_ty,
                explicit_type_args,
            } => {
                if !explicit_type_args.is_empty() {
                    self.check_exprs(arguments);
                    return ExprOutcome::invalid();
                }
                let Some(type_args) = self.collect_explicit_type_args(arguments) else {
                    return ExprOutcome::invalid();
                };
                let Some(param_count) = self.arena.signature_type_param_count(generic_ty) else {
                    return ExprOutcome::invalid();
                };
                if type_args.len() > param_count {
                    return ExprOutcome::invalid();
                }
                if type_args.len() < param_count {
                    return ExprOutcome::GenericFunc {
                        decl,
                        ty: generic_ty,
                        explicit_type_args: type_args,
                    };
                }
                match self
                    .arena
                    .instantiate_generic_signature(generic_ty, &type_args)
                    .filter(|&ty| self.arena.validate_type(ty).is_some())
                {
                    Some(ty) => {
                        self.record_instance(
                            ident_span,
                            InstanceInfo {
                                target: decl_object(decl),
                                type_args: type_args.into_boxed_slice(),
                                instantiated: ty,
                            },
                        );
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
                let Some(type_args) = self.collect_explicit_type_args(arguments) else {
                    return ExprOutcome::invalid();
                };
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
                        self.record_instance(
                            ident_span,
                            InstanceInfo {
                                target: decl_object(decl),
                                type_args: type_args.into_boxed_slice(),
                                instantiated: final_ty,
                            },
                        );
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
                let Some(type_args) = self.collect_explicit_type_args(arguments) else {
                    return ExprOutcome::invalid();
                };
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
                        self.record_instance(
                            ident_span,
                            InstanceInfo {
                                target: decl_object(decl),
                                type_args: type_args.into_boxed_slice(),
                                instantiated: final_ty,
                            },
                        );
                        return ExprOutcome::Value(Operand {
                            mode: Mode::TypeExpr,
                            ty: final_ty,
                            value: None,
                        });
                    }
                }
                ExprOutcome::invalid()
            }
            ExprOutcome::Value(op) if op.mode.is_value() => {
                if arguments.len() != 1 {
                    self.check_exprs(arguments);
                    if let Some(extra) = arguments.get(1).and_then(|&id| self.body.expression(id)) {
                        self.compiler_error(
                            &extra.span,
                            Code::InvalidIndex,
                            "invalid operation: more than one index",
                        );
                    }
                    return ExprOutcome::invalid();
                }
                let Some(idx_op) = self.check_expr(arguments[0], None) else {
                    return ExprOutcome::invalid();
                };
                let error = match self.arena.index(op, idx_op, int_bits) {
                    Ok(res) => return ExprOutcome::Value(res),
                    Err(CheckError::Invalid(error)) => error,
                    Err(CheckError::Unknown) => return ExprOutcome::invalid(),
                };
                self.index_error((operand, op), (arguments[0], idx_op), error);
                ExprOutcome::invalid()
            }
            _ => {
                for &arg_id in arguments {
                    let _ = self.check_expr_outcome(arg_id, None);
                }
                ExprOutcome::invalid()
            }
        }
    }

    fn collect_explicit_type_args(&mut self, arguments: &[ExprId]) -> Option<Vec<TypeId>> {
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
        (valid && !type_args.is_empty()).then_some(type_args)
    }

    #[allow(clippy::too_many_arguments)]
    fn record_call_site(
        &mut self,
        call_span: &Range<usize>,
        function: ExprId,
        arguments: &[ExprId],
        callee_obj: Option<ObjectRef<'a>>,
        signature: Option<TypeId>,
        is_conversion: bool,
        is_const_builtin: bool,
    ) {
        if !self.record_mask.contains(RecordMask::CALLS) {
            return;
        }
        let info = self.build_call_site_info(
            call_span,
            function,
            arguments,
            callee_obj,
            signature,
            is_conversion,
            is_const_builtin,
        );
        self.record_call(info);
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn build_call_site_info(
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
        let callee_obj = self
            .record_mask
            .contains(RecordMask::CALLS)
            .then(|| self.callee_obj_at(function))
            .flatten();
        self.record_call_site(
            call_span,
            function,
            arguments,
            callee_obj,
            Some(target_ty),
            true,
            false,
        );
        if arguments.len() != 1 || variadic {
            self.check_exprs(arguments);
            if arguments.len() != 1
                && self.record_mask.contains(RecordMask::DIAGNOSTICS)
                && let Some(ty) = self.arena.render_presentation(target_ty, self.file)
            {
                let (span, message) =
                    if let Some(last) = arguments.last().and_then(|&id| self.body.expression(id)) {
                        (
                            &last.span,
                            format!("too many arguments in conversion to {ty}"),
                        )
                    } else {
                        (call_span, format!("missing argument in conversion to {ty}"))
                    };
                self.compiler_error(span, Code::WrongArgCount, &message);
            }
            return ExprOutcome::invalid();
        }
        let Some(arg_op) = self.check_expr(arguments[0], None) else {
            return ExprOutcome::invalid();
        };
        match self.arena.checked_conversion(arg_op, target_ty, int_bits) {
            Some(Ok(res)) => {
                self.update_expr_type(arguments[0], target_ty, int_bits);
                ExprOutcome::Value(res)
            }
            Some(Err(error)) => {
                self.conversion_error(arguments[0], arg_op, target_ty, error);
                ExprOutcome::invalid()
            }
            None => ExprOutcome::invalid(),
        }
    }

    pub(super) fn check_call_single(
        &mut self,
        function: ExprId,
        arguments: &[ExprId],
        ellipsis: Option<NonZeroU32>,
        call_span: &Range<usize>,
    ) -> ExprOutcome<'a, 'b> {
        let Some(int_bits) = self.int_bits() else {
            return ExprOutcome::invalid();
        };

        let variadic = ellipsis.is_some();
        match self.check_expr_outcome(function, None) {
            ExprOutcome::Value(Operand {
                mode: Mode::Builtin(bf),
                ..
            }) => self.check_builtin_call(bf, function, arguments, ellipsis, call_span),
            ExprOutcome::Value(callee) if callee.mode == Mode::TypeExpr => self
                .finish_conversion_call(
                    function, callee.ty, arguments, variadic, call_span, int_bits,
                ),
            ExprOutcome::GenericFunc {
                decl,
                ty: generic_ty,
                explicit_type_args,
            } => {
                let Some(callee_span) = self.callee_ident_span(function) else {
                    self.check_exprs(arguments);
                    self.record_call_site(
                        call_span,
                        function,
                        arguments,
                        Some(decl_object(decl)),
                        None,
                        false,
                        false,
                    );
                    return ExprOutcome::invalid();
                };
                let Some((inst_ty, actual_ops)) = self.instantiate_generic_call(
                    decl,
                    callee_span,
                    generic_ty,
                    &explicit_type_args,
                    arguments,
                    variadic,
                ) else {
                    self.record_call_site(
                        call_span,
                        function,
                        arguments,
                        Some(decl_object(decl)),
                        None,
                        false,
                        false,
                    );
                    return ExprOutcome::invalid();
                };
                self.finish_signature_call(
                    function,
                    inst_ty,
                    Some(decl_object(decl)),
                    arguments,
                    Some(actual_ops),
                    variadic,
                    call_span,
                )
            }
            ExprOutcome::Value(callee) if callee.mode.is_value() => {
                let obj = self
                    .record_mask
                    .contains(RecordMask::CALLS)
                    .then(|| self.callee_obj_at(function))
                    .flatten();
                self.finish_signature_call(
                    function, callee.ty, obj, arguments, None, variadic, call_span,
                )
            }
            other => {
                self.recover_call(function, arguments, call_span, &other);
                if matches!(other, ExprOutcome::MissingName) {
                    ExprOutcome::MissingName
                } else {
                    ExprOutcome::invalid()
                }
            }
        }
    }

    pub(super) fn check_exprs(&mut self, expressions: &[ExprId]) {
        for &id in expressions {
            let _ = self.check_expr_outcome(id, None);
        }
    }

    fn recover_call(
        &mut self,
        function: ExprId,
        arguments: &[ExprId],
        call_span: &Range<usize>,
        callee: &ExprOutcome<'a, 'b>,
    ) {
        let obj = self
            .record_mask
            .contains(RecordMask::CALLS)
            .then(|| self.callee_obj_at(function))
            .flatten();
        if matches!(callee, ExprOutcome::InvalidImportedVar) {
            // go/types stops at signature selection for this callee. A
            // skipped closure can have bindings that the scope walk seeded
            // but the checker will never activate.
            if let Some(first) = arguments.first().and_then(|&id| self.body.expression(id))
                && self.region.scopes.nodes.iter().any(|scope| {
                    scope.kind == ScopeKind::FuncSignature
                        && (self.body_base + first.span.start..self.body_base + call_span.end)
                            .contains(&(scope.extent[0] as usize))
                })
            {
                self.region.uses_complete = false;
            }
        } else {
            self.check_exprs(arguments);
        }
        if obj.is_some() {
            self.record_call_site(call_span, function, arguments, obj, None, false, false);
        }
    }

    fn resolve_callable_with_info(
        &mut self,
        callee: ExprOutcome<'a, 'b>,
        function: ExprId,
        arguments: &[ExprId],
        variadic: bool,
        call_span: &Range<usize>,
    ) -> Option<(TypeId, Vec<Operand>)> {
        match callee {
            ExprOutcome::Value(op) if op.mode.is_value() => {
                let obj = self
                    .record_mask
                    .contains(RecordMask::CALLS)
                    .then(|| self.callee_obj_at(function))
                    .flatten();
                self.record_call_site(
                    call_span,
                    function,
                    arguments,
                    obj,
                    Some(op.ty),
                    false,
                    false,
                );
                let ops = self.evaluate_call_args(op.ty, arguments, variadic)?;
                Some((op.ty, ops))
            }
            ExprOutcome::GenericFunc {
                decl,
                ty: generic_ty,
                explicit_type_args,
            } => {
                let Some(callee_span) = self.callee_ident_span(function) else {
                    self.check_exprs(arguments);
                    self.record_call_site(
                        call_span,
                        function,
                        arguments,
                        Some(decl_object(decl)),
                        None,
                        false,
                        false,
                    );
                    return None;
                };
                let Some((inst, ops)) = self.instantiate_generic_call(
                    decl,
                    callee_span,
                    generic_ty,
                    &explicit_type_args,
                    arguments,
                    variadic,
                ) else {
                    self.record_call_site(
                        call_span,
                        function,
                        arguments,
                        Some(decl_object(decl)),
                        None,
                        false,
                        false,
                    );
                    return None;
                };
                self.record_call_site(
                    call_span,
                    function,
                    arguments,
                    Some(decl_object(decl)),
                    Some(inst),
                    false,
                    false,
                );
                Some((inst, ops))
            }
            other => {
                self.recover_call(function, arguments, call_span, &other);
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
        let results = self
            .arena
            .call_results(callee_ty, actual_ops, variadic, count, int_bits)?;
        Some(
            results
                .iter()
                .map(|&ty| Operand {
                    mode: Mode::Value,
                    ty,
                    value: None,
                })
                .collect(),
        )
    }

    pub(super) fn check_call_any_arity(
        &mut self,
        function: ExprId,
        arguments: &[ExprId],
        ellipsis: Option<NonZeroU32>,
        call_span: &Range<usize>,
    ) -> Option<Vec<Operand>> {
        let int_bits = self.int_bits()?;

        let variadic = ellipsis.is_some();
        let (callee_ty, actual_ops) = match self.check_expr_outcome(function, None) {
            ExprOutcome::Value(Operand {
                mode: Mode::Builtin(bf),
                ..
            }) => {
                let outcome = self.check_builtin_call(bf, function, arguments, ellipsis, call_span);
                if let ExprOutcome::Value(op) = outcome {
                    self.record_type_and_value(self.abs_span(call_span), op);
                    return Some(vec![op]);
                }
                return None;
            }
            ExprOutcome::Value(callee) if callee.mode == Mode::TypeExpr => {
                let outcome = self.finish_conversion_call(
                    function, callee.ty, arguments, variadic, call_span, int_bits,
                );
                if let ExprOutcome::Value(op) = outcome {
                    self.record_type_and_value(self.abs_span(call_span), op);
                    return Some(vec![op]);
                }
                return None;
            }
            callee => {
                self.resolve_callable_with_info(callee, function, arguments, variadic, call_span)?
            }
        };

        let (params, results, is_var) = self.arena.signature_parts(callee_ty)?;
        if !self.check_argument_count(
            function,
            arguments,
            call_span,
            &actual_ops,
            &params,
            variadic,
            is_var,
        ) {
            return None;
        }
        let out = self.evaluate_signature_results(
            callee_ty,
            &actual_ops,
            variadic,
            results.len(),
            int_bits,
        )?;
        if out.is_empty() {
            self.no_call_value(function, arguments, variadic, call_span);
            return None;
        }
        if out.len() == 1 {
            self.record_type_and_value(self.abs_span(call_span), out[0]);
        } else {
            let tys: Vec<TypeId> = out.iter().map(|op| op.ty).collect();
            if let Some(tuple_ty) = self.arena.tuple(&tys) {
                self.record_type_and_value(
                    self.abs_span(call_span),
                    Operand {
                        mode: Mode::Value,
                        ty: tuple_ty,
                        value: None,
                    },
                );
            }
        }
        Some(out)
    }

    fn instantiate_generic_call(
        &mut self,
        decl: DeclId<'a>,
        callee_span: [u32; 2],
        generic_ty: TypeId,
        explicit_type_args: &[TypeId],
        arguments: &[ExprId],
        variadic: bool,
    ) -> Option<(TypeId, Vec<Operand>)> {
        let int_bits = self.int_bits()?;

        let actual_ops = self.eval_call_arg_operands(arguments, variadic, None)?;
        let (inferred, inst_ty) = self.arena.infer_and_instantiate_generic_signature(
            generic_ty,
            &actual_ops,
            explicit_type_args,
            int_bits,
            variadic,
        )?;
        self.record_instance(
            callee_span,
            InstanceInfo {
                target: decl_object(decl),
                type_args: inferred.into_boxed_slice(),
                instantiated: inst_ty,
            },
        );
        Some((inst_ty, actual_ops))
    }

    pub(super) fn eval_call_arg_operands(
        &mut self,
        arguments: &[ExprId],
        variadic: bool,
        param_hints: Option<&[TypeId]>,
    ) -> Option<Vec<Operand>> {
        let body = self.body;
        if arguments.len() == 1
            && !variadic
            && let Some(expr) = body.expression(arguments[0])
            && let ExprKind::Call {
                function,
                arguments: ref inner_args,
                ellipsis,
                ..
            } = expr.kind
        {
            return self.check_call_any_arity(function, inner_args, ellipsis, &expr.span);
        }
        let mut ops = Vec::with_capacity(arguments.len());
        let mut ok = true;
        for (idx, &arg_id) in arguments.iter().enumerate() {
            let expected =
                param_hints.and_then(|p| if variadic { None } else { p.get(idx).copied() });
            match self.check_expr(arg_id, expected) {
                Some(op) if op.mode.is_value() => ops.push(op),
                _ => ok = false,
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
        let Some((params, _, _)) = self.arena.signature_parts(callee_ty) else {
            self.check_exprs(arguments);
            return None;
        };
        self.eval_call_arg_operands(arguments, variadic, Some(&params))
    }

    #[allow(clippy::too_many_arguments)]
    fn finish_signature_call(
        &mut self,
        function: ExprId,
        callee_ty: TypeId,
        callee_obj: Option<ObjectRef<'a>>,
        arguments: &[ExprId],
        actual_ops: Option<Vec<Operand>>,
        variadic: bool,
        call_span: &Range<usize>,
    ) -> ExprOutcome<'a, 'b> {
        let Some(int_bits) = self.int_bits() else {
            return ExprOutcome::invalid();
        };

        self.record_call_site(
            call_span,
            function,
            arguments,
            callee_obj,
            Some(callee_ty),
            false,
            false,
        );

        let Some((params, results, is_var)) = self.arena.signature_parts(callee_ty) else {
            if actual_ops.is_none() {
                self.check_exprs(arguments);
            }
            return ExprOutcome::invalid();
        };

        let Some(actual_ops) =
            actual_ops.or_else(|| self.eval_call_arg_operands(arguments, variadic, Some(&params)))
        else {
            return ExprOutcome::invalid();
        };

        if !self.check_argument_count(
            function,
            arguments,
            call_span,
            &actual_ops,
            &params,
            variadic,
            is_var,
        ) {
            return ExprOutcome::invalid();
        }
        match self.arena.call_results(
            callee_ty,
            &actual_ops,
            variadic,
            usize::from(!results.is_empty()),
            int_bits,
        ) {
            Some([]) => ExprOutcome::no_value(),
            Some([res_ty]) => ExprOutcome::Value(Operand {
                mode: Mode::Value,
                ty: *res_ty,
                value: None,
            }),
            _ => ExprOutcome::invalid(),
        }
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
}
