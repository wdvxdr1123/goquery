//! Request-scoped semantic session coordinating [`TypeArena`], [`DeclTable`], and [`RegionChecker`].
mod bodies;
mod completion;
mod diagnostics;
mod expected;
mod expressions;
pub(crate) mod layout;
mod members;
mod navigation;
mod region_cache;
mod relations;
mod signatures;
mod ty;
mod view;

use std::path::Path;

use crate::analysis::engine::check::decls::DeclTable;
use crate::analysis::engine::check::region::RegionInfo;
use crate::analysis::engine::types::{Operand, TypeArena, TypeId};
use crate::index::IndexedSymbol;
use crate::workspace::Workspace;

pub(crate) use crate::analysis::engine::check::diagnostics::{
    Code, Diagnostic, Kind, MAX_DIAGNOSTICS,
};
pub(crate) use crate::analysis::engine::check::region::WriteKind;
pub(crate) use crate::analysis::engine::obj::{
    DeclId, LocalId, MemberDeclId, MethodOrigin, NamedObject, Object, Object as SemanticObject,
    Object as ObjectRef, SymbolOrigin, VarKind, decl_object,
};
pub(crate) use crate::analysis::engine::scope::occurrences::{
    identifier_at, scan_name as lexical_name_spans,
    scan_qualified_name as lexical_qualified_name_spans,
};
pub(crate) use crate::analysis::engine::scope::universe::{
    builtin as is_builtin_universe, builtin_function as is_builtin_function,
    builtin_type as is_builtin_type, imported_name as imported_name_in,
};
pub(crate) use crate::analysis::engine::scope::{named_root, named_type};
pub(crate) use crate::analysis::engine::types::{Constant, Qualifier, TypeDeclaration, TypeForm};
pub(crate) use bodies::CursorBody;
pub(crate) use completion::{CompletionSite, CompletionTypeView, MemberRequest};
pub(crate) use expected::CompletionProbe;
pub(crate) use layout::{FieldLayout, Layout, StructLayout};
pub(crate) use members::MethodView;
pub(crate) use members::{CallableView, MemberTypeView};
use region_cache::{CachedRegion, RegionCoverage};
pub(crate) use relations::TypeRelationQuery;
pub(crate) use signatures::CheckedCallable;
pub(crate) use ty::{TypeAndValue, TypeHandle, TypeInspector, TypeRef, TypeShape};
pub(crate) use view::{LocalBindingView, LocalDeclKind, PackageView, RegionMode, RegionView};

/// Transition alias while `query/` callers adopt `SemanticSession`.
pub(crate) type Semantics<'a> = SemanticSession<'a>;

/// Request-scoped semantic session backed by a two-tier [`TypeArena`].
pub(crate) struct SemanticSession<'a> {
    workspace: &'a Workspace,
    arena: TypeArena<'a>,
    decls: DeclTable<'a>,
    cached: Option<CachedRegion<'a>>,
    failed: Option<(&'a Path, [u32; 2], RegionCoverage)>,
    parsed_body: Option<bodies::ParsedBody<'a>>,
    cached_completion: Option<completion::CachedCompletionSelections<'a>>,
}

impl<'a> SemanticSession<'a> {
    pub(crate) fn new(workspace: &'a Workspace, view: crate::workspace::BuildView<'a>) -> Self {
        let arena = TypeArena::with_shared_budget(workspace, view);
        Self {
            workspace,
            arena,
            decls: DeclTable::new(),
            cached: None,
            failed: None,
            parsed_body: None,
            cached_completion: None,
        }
    }

    pub(super) fn canonical_decl(&self, declaration: DeclId<'_>) -> Option<DeclId<'a>> {
        let (file, stub) = self.workspace.files.get_key_value(declaration.file)?;
        let symbol = stub.symbols.iter().find(|symbol| {
            symbol.start == declaration.symbol.start && symbol.name == declaration.symbol.name
        })?;
        Some(DeclId { file, symbol })
    }

    pub(crate) fn intern_file(&self, file: &Path) -> Option<&'a Path> {
        self.workspace
            .files
            .get_key_value(file)
            .map(|(path, _)| path.as_path())
    }

    pub(crate) fn region_at<'r>(
        &'r mut self,
        file: &Path,
        source: &'r str,
        offset: u32,
        mode: RegionMode,
    ) -> Option<RegionView<'r, 'a>> {
        let file = self.intern_file(file)?;
        if !self.ensure_region(file, source, offset, mode.cursor_offset()) {
            return None;
        }
        let cached = self.cached.as_ref()?;
        Some(RegionView {
            workspace: self.workspace,
            file,
            source,
            region: &cached.region,
            arena: &mut self.arena,
            decls: &mut self.decls,
            syntax_body: None,
            syntax_sig: None,
        })
    }

    pub(crate) fn check_function_view<'r>(
        &'r mut self,
        file: &'a Path,
        source: &'r str,
        sym: &'a IndexedSymbol,
    ) -> Option<RegionView<'r, 'a>> {
        let decl = sym.declaration()?;
        let span = [decl.start as u32, decl.end as u32];
        let cache_hit = self.cached.as_ref().is_some_and(|c| {
            c.file == file && c.span == span && c.coverage.satisfies(RegionCoverage::Full)
        });
        if !cache_hit {
            if !self.build_function_region(file, source, sym, None) {
                return None;
            }
        } else {
            self.arena.reset_steps();
        }
        let cached = self.cached.as_ref()?;
        Some(RegionView {
            workspace: self.workspace,
            file,
            source,
            region: &cached.region,
            arena: &mut self.arena,
            decls: &mut self.decls,
            syntax_body: None,
            syntax_sig: None,
        })
    }

    pub(crate) fn package_view<'r>(
        &'r mut self,
        file: &Path,
        source: &'r str,
    ) -> Option<PackageView<'r, 'a>> {
        let file = self.intern_file(file)?;
        Some(PackageView {
            workspace: self.workspace,
            file,
            source,
            arena: &mut self.arena,
            decls: &mut self.decls,
        })
    }

    pub(crate) fn relations(&mut self) -> TypeRelationQuery<'_, 'a> {
        TypeRelationQuery::new(self)
    }

    pub(crate) fn declaration_member(
        &mut self,
        declaration: DeclId<'_>,
        consumer: &Path,
        name: &str,
    ) -> Option<Object<'a>> {
        let declaration = self.canonical_decl(declaration)?;
        self.arena.declaration_member(declaration, consumer, name)
    }

    pub(crate) fn completion_inferable(
        &mut self,
        file: &Path,
        parameters: &[(&str, &str)],
        inputs: &[&str],
    ) -> Option<Vec<bool>> {
        crate::analysis::engine::types::completion_inferable(
            &mut self.arena,
            file,
            parameters,
            inputs,
        )
    }

    pub(crate) fn decl_operand_presentation(
        &mut self,
        declaration: DeclId<'_>,
        consumer: &Path,
    ) -> Option<(String, Option<Constant>)> {
        let decl = self.canonical_decl(declaration)?;
        self.arena.reset_steps();
        let operand = self.decls.operand_of(&mut self.arena, decl)?;
        let text = self
            .arena
            .format_type(operand.ty, Qualifier::Presentation(consumer))?;
        Some((text, operand.value))
    }

    pub(crate) fn decl_array_length(&mut self, declaration: DeclId<'_>) -> Option<usize> {
        let decl = self.canonical_decl(declaration)?;
        self.arena.reset_steps();
        let operand = self.decls.operand_of(&mut self.arena, decl)?;
        usize::try_from(self.arena.array_length(operand.ty)?).ok()
    }

    pub(crate) fn imported_name(&self, file: &Path, qualifier: &str) -> bool {
        self.arena.resolver().imported_name(file, qualifier)
    }

    pub(crate) fn universe_builtin_visible(
        &mut self,
        file: &Path,
        source: &str,
        offset: u32,
        name: &str,
    ) -> bool {
        if !crate::analysis::engine::scope::universe::builtin(name) {
            return false;
        }
        if self.imported_name(file, name)
            || crate::analysis::engine::resolve::unqualified_symbol(self.workspace, file, name)
                .is_some()
        {
            return false;
        }
        !self
            .region_at(file, source, offset, RegionMode::Prefix(offset as usize))
            .is_some_and(|view| view.is_local_defined_at(offset, name))
    }

    pub(super) fn region_and_arena_at(
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
}

#[allow(dead_code)]
fn _keep_internal_types_used(_op: Option<Operand>, _id: Option<TypeId>) {}
