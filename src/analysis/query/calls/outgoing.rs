//! Expand nested bodies on demand, attributing their calls to the named function.
use super::{CallGroup, Groups, finish, record_call};
use crate::analysis::obj::{MethodOrigin, ObjectRef};
use crate::analysis::query::{Context, SourceRange};
use crate::analysis::sema::Semantics;
use crate::index::IndexedSymbol as Symbol;
use crate::text::SourceMap;
use std::path::Path;

impl<'a, View> Context<'a, View> {
    pub(crate) fn outgoing_calls(
        &self,
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

        let mut sema = Semantics::new(self.workspace);
        let Some(interned_file) = sema.intern_file(file) else {
            return Ok(finish(groups));
        };
        let region = if target.is_value() {
            let Some((region, _)) =
                sema.region_and_arena_at(interned_file, &source, start as u32, None)
            else {
                return Ok(finish(groups));
            };
            region
        } else {
            let Some((region, _)) = sema.check_function_full(interned_file, &source, target, None)
            else {
                return Ok(finish(groups));
            };
            region
        };

        let mut count = 0;
        for call in &region.calls {
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
    }
}
