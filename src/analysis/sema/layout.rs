//! Layout and constant queries share the request's checker and type arena.
use super::Semantics;
use crate::analysis::engine::obj::DeclId;
pub(crate) use crate::analysis::engine::types::{FieldLayout, Layout, StructLayout, align_to};
use std::path::Path;

impl<'a> Semantics<'a> {
    pub(crate) fn layout_of_decl(&mut self, decl: DeclId<'a>) -> Option<Layout> {
        self.prepare_for_new_region();
        let ty = self.decls.type_of(&mut self.arena, decl)?;
        self.arena.layout(ty)
    }

    pub(crate) fn struct_layout_of_decl(&mut self, decl: DeclId<'a>) -> Option<StructLayout> {
        self.prepare_for_new_region();
        let ty = self.decls.type_of(&mut self.arena, decl)?;
        self.arena.struct_layout(ty)
    }

    pub(crate) fn struct_layout_of_text(
        &mut self,
        file: &Path,
        text: &str,
    ) -> Option<StructLayout> {
        self.prepare_for_new_region();
        let ty = self.arena.parse(file, text)?;
        self.arena.struct_layout(ty)
    }
}
