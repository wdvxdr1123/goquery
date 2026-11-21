//! Expand nested bodies on demand, attributing their calls to the named function.
use super::{CallGroup, Groups, finish, record_call};
use crate::analysis::query::{Context, SourceRange};
use crate::analysis::sema::{MethodOrigin, ObjectRef};
use crate::index::IndexedSymbol as Symbol;
use crate::text::SourceMap;
use std::path::Path;

impl<'a, View> Context<'a, View> {
    pub(crate) fn outgoing_calls(
        &mut self,
        file: &'a Path,
        target: &'a Symbol,
    ) -> Result<Vec<CallGroup<'a>>, (i32, String)> {
        let mut groups = Groups::new();
        crate::analysis::resolve::trace_candidates(self.workspace, "outgoingCalls", 1);
        let Some(source) = self.workspace.source(file) else {
            return Ok(finish(groups));
        };
        let map = SourceMap::new(&source);
        let Some(start) = map.offset(target.range.start) else {
            return Ok(finish(groups));
        };
        let Some(end) = map.offset(target.range.end) else {
            return Ok(finish(groups));
        };
        if !target.is_value() && target.body().is_none() {
            return Ok(finish(groups));
        }

        self.with_sema(|sema| {
            let Some(interned_file) = sema.intern_file(file) else {
                return Ok(finish(groups));
            };
            let view = if target.is_value() {
                let Some(view) = sema.region_at(
                    interned_file,
                    &source,
                    start as u32,
                    crate::analysis::sema::RegionMode::Full,
                ) else {
                    return Ok(finish(groups));
                };
                view
            } else {
                let Some(view) = sema.check_function_view(interned_file, &source, target) else {
                    return Ok(finish(groups));
                };
                view
            };

            let mut count = 0;
            for call in view.calls() {
                if call.is_conversion {
                    continue;
                }
                if target.is_value()
                    && ((call.span[0] as usize) < start || (call.span[1] as usize) > end)
                {
                    continue;
                }
                let Some(ident_span) = call.callee_ident_span else {
                    continue;
                };
                let decl = match call.callee_obj {
                    Some(ObjectRef::Func(decl))
                    | Some(ObjectRef::Method(MethodOrigin::Concrete(decl)))
                        if decl.symbol.is_callable() =>
                    {
                        decl
                    }
                    _ => continue,
                };
                let Some((callee_file, _)) = self.workspace.files.get_key_value(decl.file) else {
                    continue;
                };
                record_call(
                    &mut groups,
                    callee_file,
                    decl.symbol,
                    SourceRange::new(&map, ident_span[0] as usize..ident_span[1] as usize),
                    &mut count,
                )?;
            }
            Ok(finish(groups))
        })
    }
}
