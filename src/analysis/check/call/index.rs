//! Index expressions and generic function/type instantiation for [`RegionChecker`].

use std::ops::Range;

use crate::analysis::engine::check::diagnostics::Code;
use crate::analysis::engine::check::region::InstanceInfo;
use crate::analysis::engine::check::{ExprOutcome, RegionChecker, decl_object};
use crate::analysis::engine::obj::DeclId;
use crate::analysis::engine::types::{CheckError, Mode, Operand, TypeId};
use crate::syntax::ExprId;

impl<'a, 'b> RegionChecker<'a, 'b> {
    pub(in crate::analysis::engine::check) fn check_index(
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
                let _ = self.decls.generic_type_origin(self.arena, decl);
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

    pub(super) fn instantiate_generic_call(
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
}
