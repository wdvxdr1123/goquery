//! Request-scoped semantic facade coordinating [`TypeArena`], [`DeclCache`], and [`RegionChecker`].
mod bodies;
mod completion;
mod diagnostics;
mod expected;
mod expressions;
mod identity;
pub(crate) mod layout;
mod literals;
mod members;
mod navigation;
mod region_cache;
mod relations;
mod signatures;

use std::path::Path;

use crate::analysis::check::decls::DeclCache;
use crate::analysis::check::region::RegionInfo;
use crate::analysis::obj::{DeclId, ObjectRef};
use crate::analysis::types::{ArenaWatermark, Operand, TypeArena, TypeId};
use crate::index::IndexedSymbol;
use crate::workspace::Workspace;

pub(crate) use completion::{CompletionSite, CompletionTypeView, MemberRequest};
pub(crate) use expected::CompletionProbe;
pub(crate) use identity::{Identity, IdentityResolver};
pub(crate) use literals::LiteralKey;
pub(crate) use members::MethodView;
pub(crate) use members::{CallableView, MemberTypeView};
use region_cache::CachedRegion;
pub(crate) use signatures::CheckedCallable;

/// Request-scoped semantic query facade backed by a bounded shared [`TypeArena`].
pub(crate) struct Semantics<'a> {
    workspace: &'a Workspace,
    arena: TypeArena<'a>,
    decls: DeclCache<'a>,
    base_mark: ArenaWatermark,
    root_mark: ArenaWatermark,
    cached: Option<CachedRegion<'a>>,
    failed: Option<(&'a Path, [u32; 2], Option<usize>)>,
    parsed_body: Option<bodies::ParsedBody<'a>>,
}

impl<'a> Semantics<'a> {
    pub(crate) fn new(workspace: &'a Workspace, view: crate::workspace::BuildView<'a>) -> Self {
        let arena = TypeArena::with_shared_budget(workspace, view);
        let base_mark = arena.mark();
        Self {
            workspace,
            arena,
            decls: DeclCache::new(view),
            base_mark,
            root_mark: base_mark,
            cached: None,
            failed: None,
            parsed_body: None,
        }
    }

    pub(crate) fn intern_file(&self, file: &Path) -> Option<&'a Path> {
        self.workspace
            .files
            .get_key_value(file)
            .map(|(path, _)| path.as_path())
    }

    pub(crate) fn arena_mut(&mut self) -> &mut TypeArena<'a> {
        &mut self.arena
    }

    pub(crate) fn type_of_decl(&mut self, decl: DeclId<'a>) -> Option<TypeId> {
        self.arena.reset_steps();
        self.decls.type_of(&mut self.arena, decl)
    }

    pub(crate) fn operand_of_decl(&mut self, decl: DeclId<'a>) -> Option<Operand> {
        self.arena.reset_steps();
        self.decls.operand_of(&mut self.arena, decl)
    }

    pub(crate) fn check_function_full(
        &mut self,
        file: &'a Path,
        source: &str,
        sym: &'a IndexedSymbol,
        cursor_offset: Option<usize>,
    ) -> Option<(&RegionInfo<'a>, &mut TypeArena<'a>)> {
        let decl = sym.declaration()?;
        let span = [decl.start as u32, decl.end as u32];
        let cache_hit = self
            .cached
            .as_ref()
            .is_some_and(|c| c.file == file && c.span == span && c.cursor_offset == cursor_offset);
        if !cache_hit {
            if !self.build_function_region(file, source, sym, cursor_offset) {
                return None;
            }
        } else {
            self.arena.reset_steps();
        }
        let cached = self.cached.as_ref()?;
        Some((&cached.region, &mut self.arena))
    }

    pub(crate) fn region_and_arena_at(
        &mut self,
        file: &Path,
        source: &str,
        offset: u32,
        cursor_offset: Option<usize>,
    ) -> Option<(&RegionInfo<'a>, &mut TypeArena<'a>)> {
        let file = self.intern_file(file)?;
        if !self.ensure_region(file, source, offset, cursor_offset) {
            return None;
        }
        let cached = self.cached.as_ref()?;
        Some((&cached.region, &mut self.arena))
    }

    pub(crate) fn object_at(
        &mut self,
        file: &Path,
        source: &str,
        offset: u32,
    ) -> Option<([u32; 2], ObjectRef<'a>)> {
        let (region, _) = self.region_and_arena_at(file, source, offset, None)?;
        region.object_at(offset)
    }
}
