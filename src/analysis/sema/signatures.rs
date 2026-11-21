//! Callable views come from checked signatures, never initializer spelling.
use super::{CallableView, MemberTypeView, Semantics};
use crate::analysis::engine::obj::{BuiltinFunction, ObjectRef};
use crate::analysis::engine::types::Mode;
use std::path::Path;

pub(crate) enum CheckedCallable {
    Builtin(BuiltinFunction),
    Signature(CallableView),
}

impl Semantics<'_> {
    pub(crate) fn checked_callable(
        &mut self,
        file: &Path,
        source: &str,
        callee: [u32; 2],
        ident: Option<[u32; 2]>,
        cursor: Option<usize>,
        in_call: bool,
    ) -> Option<CheckedCallable> {
        let (ty, object) = {
            let (region, _) = self.region_and_arena_at(file, source, callee[0], cursor)?;
            let call = in_call.then(|| region.call_by_callee(callee)).flatten();
            if call.is_some_and(|call| call.is_conversion || call.is_const_builtin) {
                return None;
            }
            let object = call
                .and_then(|call| call.callee_obj)
                .or_else(|| ident.and_then(|span| region.use_by_span(span)))
                .or_else(|| {
                    ident.and_then(|span| {
                        region
                            .selection_by_span(span)
                            .map(|selection| selection.obj)
                    })
                });
            let ty = call
                .and_then(|call| call.signature)
                .or_else(|| {
                    region
                        .expr_type(callee)
                        .filter(|operand| operand.mode.is_value())
                        .map(|operand| operand.ty)
                })
                .or_else(|| {
                    ident.and_then(|span| {
                        region.selection_by_span(span).map(|selection| selection.ty)
                    })
                });
            (ty, object)
        };
        if let Some(ObjectRef::BuiltinFunction(function)) = object {
            return Some(CheckedCallable::Builtin(function));
        }
        let view = if let Some(ty) = ty {
            MemberTypeView::read(&mut self.arena, ty, None, file, false)?.callable?
        } else if let Some(ObjectRef::Func(declaration)) = object.filter(|_| !in_call) {
            self.arena.with_projection(true, |arena| {
                let ty = arena.formal_function_signature(declaration)?;
                MemberTypeView::read(arena, ty, None, file, false)?.callable
            })?
        } else {
            return None;
        };
        Some(CheckedCallable::Signature(view))
    }

    pub(crate) fn binding_callable(
        &mut self,
        file: &Path,
        source: &str,
        selection: [u32; 2],
        scope_start: u32,
    ) -> Option<CallableView> {
        let (region, arena) = self.region_and_arena_at(file, source, selection[0], None)?;
        let (id, _) = region.local_binding_at(selection, scope_start)?;
        let operand = region.local_type(id)?;
        if operand.mode != Mode::Variable {
            return None;
        }
        MemberTypeView::read(arena, operand.ty, None, file, false)?.callable
    }
}
