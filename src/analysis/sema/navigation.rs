//! Exact cursor identities from declaration metadata and checked region facts.
use super::Semantics;
use crate::analysis::engine::check::region::RecordMask;
use crate::analysis::engine::obj::{DeclId, ObjectRef};
use std::path::Path;

const NAVIGATION_MASK: RecordMask = RecordMask::DEFS
    .union(RecordMask::USES)
    .union(RecordMask::IMPLICITS)
    .union(RecordMask::SELECTIONS);

impl<'a> Semantics<'a> {
    pub(crate) fn alias_rhs_declaration(&mut self, declaration: DeclId<'_>) -> Option<DeclId<'a>> {
        let declaration = self.canonical_decl(declaration)?;
        self.arena
            .with_projection(true, |arena| arena.alias_named_rhs(declaration))
    }

    /// Type-switch cases have distinct objects sharing the guard declaration.
    /// Reference queries group that source declaration without inventing a
    /// go/types object for the guard's nil Defs entry.
    pub(crate) fn implicit_declaration_at(
        &mut self,
        file: &Path,
        source: &str,
        span: [u32; 2],
    ) -> bool {
        if !self.arena.resolver().file_visible(file) {
            return false;
        }
        self.region_and_arena_at_with_mask(file, source, span[0], None, RecordMask::IMPLICITS)
            .is_some_and(|(region, _)| {
                region.implicits.iter().any(|(_, object)| {
                    matches!(
                        object,
                        ObjectRef::Var {
                            origin: crate::analysis::engine::obj::SymbolOrigin::Local {
                                span: declaration,
                                ..
                            },
                            ..
                        } if *declaration == span
                    )
                })
            })
    }

    pub(crate) fn resolve_offset(
        &mut self,
        file: &Path,
        source: &str,
        span: [u32; 2],
    ) -> Option<ObjectRef<'a>> {
        self.object_for_span(file, source, span, None)
    }

    pub(crate) fn resolve_navigation_offset(
        &mut self,
        file: &Path,
        source: &str,
        span: [u32; 2],
    ) -> Option<ObjectRef<'a>> {
        self.object_for_span(file, source, span, Some(span[0] as usize))
    }

    fn object_for_span(
        &mut self,
        file: &Path,
        source: &str,
        span: [u32; 2],
        cursor: Option<usize>,
    ) -> Option<ObjectRef<'a>> {
        let file = self.intern_file(file)?;
        if !self.arena.resolver().file_visible(file) {
            return None;
        }
        let stub = self.workspace.files.get(file)?;
        if let Some(symbol) = stub.symbols.iter().find(|symbol| {
            symbol.start == span[0] as usize
                && symbol.end == span[1] as usize
                && source.get(symbol.start..symbol.end) == Some(symbol.name.as_str())
        }) {
            return Some(crate::analysis::engine::check::decl_object(DeclId {
                file,
                symbol,
            }));
        }
        let (region, _) =
            self.region_and_arena_at_with_mask(file, source, span[0], cursor, NAVIGATION_MASK)?;
        region
            .def_by_span(span)
            .or_else(|| region.use_by_span(span))
            .or_else(|| {
                region
                    .selection_by_span(span)
                    .map(|selection| selection.obj)
            })
    }
}
