//! Bounded cursor-region cache and streaming region iteration for [`Semantics`].

use std::path::Path;

use crate::analysis::engine::budget::MAX_BYTES;
use crate::analysis::engine::check::decls::DeclTable;
use crate::analysis::engine::check::region::RegionInfo;
use crate::analysis::engine::check::{RegionChecker, decl_object};
use crate::analysis::engine::obj::DeclId;
use crate::analysis::engine::scope::tree::ScopeTree;
use crate::analysis::engine::types::TypeArena;
use crate::index::IndexedSymbol;
use crate::syntax::{self, DeclSpecKind};

use super::Semantics;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RegionCoverage {
    Full,
    UpTo(usize),
}

impl RegionCoverage {
    pub(super) fn from_cursor(cursor_offset: Option<usize>) -> Self {
        match cursor_offset {
            None => Self::Full,
            Some(offset) => Self::UpTo(offset),
        }
    }

    pub(super) fn satisfies(self, needed: Self) -> bool {
        match (self, needed) {
            (Self::Full, _) => true,
            (Self::UpTo(a), Self::UpTo(b)) => a == b,
            (Self::UpTo(_), Self::Full) => false,
        }
    }
}

type BuiltFunctionParts<'a> = (
    RegionInfo<'a>,
    Option<(usize, std::sync::Arc<syntax::BlockTree>)>,
    Option<(usize, syntax::Signature)>,
);

pub(super) struct CachedRegion<'a> {
    pub(super) file: &'a Path,
    pub(super) span: [u32; 2],
    pub(super) coverage: RegionCoverage,
    pub(super) region: RegionInfo<'a>,
}

impl<'a> Semantics<'a> {
    pub(super) fn prepare_for_new_region(&mut self) {
        self.failed = None;
        self.cached = None;
        self.cached_completion = None;
        self.arena.reset_local();
    }

    pub(super) fn ensure_region(
        &mut self,
        file: &'a Path,
        source: &str,
        offset: u32,
        cursor_offset: Option<usize>,
    ) -> bool {
        let needed = RegionCoverage::from_cursor(cursor_offset);
        if let Some(cached) = self.cached.as_ref()
            && cached.file == file
            && cached.span[0] <= offset
            && offset <= cached.span[1]
            && cached.coverage.satisfies(needed)
        {
            self.arena.reset_steps();
            return true;
        }
        if let Some((f_file, f_span, f_cov)) = self.failed
            && f_file == file
            && f_span[0] <= offset
            && offset <= f_span[1]
            && f_cov == needed
        {
            return false;
        }

        let Some(stub) = self.workspace.files.get(file) else {
            return false;
        };

        for sym in &stub.symbols {
            if !(sym.is_function() || sym.is_method()) {
                continue;
            }
            let Some(decl) = sym.declaration() else {
                continue;
            };
            if decl.start <= offset as usize && (offset as usize) <= decl.end {
                let built = self.build_function_region(file, source, sym, cursor_offset);
                if !built {
                    self.failed = Some((file, [decl.start as u32, decl.end as u32], needed));
                }
                return built;
            }
        }

        for &[g_start, g_end] in &stub.constant_groups {
            if g_start <= offset && offset <= g_end {
                let built = self.build_constant_group_region(
                    file,
                    source,
                    stub,
                    [g_start, g_end],
                    cursor_offset,
                );
                if !built {
                    self.failed = Some((file, [g_start, g_end], needed));
                }
                return built;
            }
        }

        for sym in &stub.symbols {
            if !(sym.is_var() || sym.is_const()) {
                continue;
            }
            let Some(decl) = sym.declaration() else {
                continue;
            };
            if decl.start <= offset as usize && (offset as usize) <= decl.end {
                let built = self.build_value_spec_region(file, source, stub, sym, cursor_offset);
                if !built {
                    self.failed = Some((file, [decl.start as u32, decl.end as u32], needed));
                }
                return built;
            }
        }

        for sym in &stub.symbols {
            if !sym.is_type() {
                continue;
            }
            let Some(decl) = sym.declaration() else {
                continue;
            };
            if decl.start <= offset as usize && (offset as usize) <= decl.end {
                let built = self.build_type_decl_region(file, source, sym, cursor_offset);
                if !built {
                    self.failed = Some((file, [decl.start as u32, decl.end as u32], needed));
                }
                return built;
            }
        }

        false
    }

    pub(crate) fn visit_file_regions(
        &mut self,
        file: &'a Path,
        source: &str,
        window: Option<std::ops::Range<usize>>,
        visit: impl FnMut(&mut super::RegionView<'_, 'a>),
    ) {
        self.visit_matching_file_regions(
            file,
            source,
            |[s, e]| {
                window
                    .as_ref()
                    .is_none_or(|w| (s as usize) < w.end && (e as usize) > w.start)
            },
            visit,
        );
    }

    pub(crate) fn visit_matching_file_regions(
        &mut self,
        file: &'a Path,
        source: &str,
        mut include: impl FnMut([u32; 2]) -> bool,
        mut visit: impl FnMut(&mut super::RegionView<'_, 'a>),
    ) {
        self.stream_matching_file_regions(
            file,
            source,
            &mut include,
            |region, arena, decls, body, sig| {
                let mut view = super::RegionView {
                    workspace: self.workspace,
                    file,
                    source,
                    region,
                    arena,
                    decls,
                    syntax_body: body,
                    syntax_sig: sig,
                };
                visit(&mut view);
            },
        );
    }

    pub(super) fn stream_matching_file_regions(
        &mut self,
        file: &'a Path,
        source: &str,
        include: &mut impl FnMut([u32; 2]) -> bool,
        mut visit: impl FnMut(
            &RegionInfo<'a>,
            &mut TypeArena<'a>,
            &mut DeclTable<'a>,
            Option<(usize, &syntax::BlockTree)>,
            Option<(usize, &syntax::Signature)>,
        ),
    ) {
        enum RegionItem<'a> {
            Function(&'a IndexedSymbol),
            ConstGroup([u32; 2]),
            ValueSpec(&'a IndexedSymbol),
            TypeDecl(&'a IndexedSymbol),
        }

        self.cached = None;
        let Some(stub) = self.workspace.files.get(file) else {
            return;
        };

        let mut items: Vec<([u32; 2], RegionItem<'a>)> = Vec::new();
        for &group in &stub.constant_groups {
            items.push((group, RegionItem::ConstGroup(group)));
        }
        let mut seen_value_specs: Vec<[u32; 2]> = Vec::new();
        let mut seen_type_specs: Vec<[u32; 2]> = Vec::new();
        for sym in &stub.symbols {
            let Some(decl) = sym.declaration() else {
                continue;
            };
            let span = [decl.start as u32, decl.end as u32];
            if sym.is_function() || sym.is_method() {
                items.push((span, RegionItem::Function(sym)));
            } else if sym.is_var() || sym.is_const() {
                if stub
                    .constant_groups
                    .iter()
                    .any(|&[gs, ge]| gs <= span[0] && span[1] <= ge)
                {
                    continue;
                }
                if seen_value_specs.contains(&span) {
                    continue;
                }
                seen_value_specs.push(span);
                items.push((span, RegionItem::ValueSpec(sym)));
            } else if sym.is_type() {
                if seen_type_specs.contains(&span) {
                    continue;
                }
                seen_type_specs.push(span);
                items.push((span, RegionItem::TypeDecl(sym)));
            }
        }

        items.retain(|(span, _)| include(*span));
        items.sort_by_key(|(span, _)| (span[0], span[1]));

        let mut prev_end = 0u32;
        for (span, item) in items {
            if span[1] <= prev_end {
                continue;
            }
            prev_end = span[1];
            match item {
                RegionItem::Function(sym) => {
                    if let Some((region, body, sig)) =
                        self.compute_function_region_parts(file, source, sym, None)
                    {
                        visit(
                            &region,
                            &mut self.arena,
                            &mut self.decls,
                            body.as_ref().map(|(base, tree)| (*base, tree.as_ref())),
                            sig.as_ref().map(|(base, s)| (*base, s)),
                        );
                    }
                }
                RegionItem::ConstGroup(group) => {
                    if let Some(region) =
                        self.compute_constant_group_region(file, source, stub, group, None)
                    {
                        visit(&region, &mut self.arena, &mut self.decls, None, None);
                    }
                }
                RegionItem::ValueSpec(sym) => {
                    if let Some(region) =
                        self.compute_value_spec_region(file, source, stub, sym, None)
                    {
                        visit(&region, &mut self.arena, &mut self.decls, None, None);
                    }
                }
                RegionItem::TypeDecl(sym) => {
                    if let Some(region) = self.compute_type_decl_region(file, source, sym, None) {
                        visit(&region, &mut self.arena, &mut self.decls, None, None);
                    }
                }
            }
            self.parsed_body = None;
        }
        self.prepare_for_new_region();
    }

    pub(super) fn build_function_region(
        &mut self,
        file: &'a Path,
        source: &str,
        sym: &'a IndexedSymbol,
        cursor_offset: Option<usize>,
    ) -> bool {
        let Some((region, _, _)) =
            self.compute_function_region_parts(file, source, sym, cursor_offset)
        else {
            return false;
        };
        let span = region.span;
        self.cached = Some(CachedRegion {
            file,
            span,
            coverage: RegionCoverage::from_cursor(cursor_offset),
            region,
        });
        true
    }

    fn compute_function_region_parts(
        &mut self,
        file: &'a Path,
        source: &str,
        sym: &'a IndexedSymbol,
        cursor_offset: Option<usize>,
    ) -> Option<BuiltFunctionParts<'a>> {
        let decl = sym.declaration()?;
        let header_end = sym.body().map_or(decl.end, |b| b.start);
        if header_end
            .checked_sub(decl.start)
            .is_none_or(|len| len > MAX_BYTES)
        {
            return None;
        }
        let header_slice = source.get(decl.start..header_end)?;
        let body_range = sym.body();
        let sig = syntax::parse_signature(header_slice);
        self.prepare_for_new_region();

        let owner = DeclId { file, symbol: sym };
        let span = [decl.start as u32, decl.end as u32];
        if let Some(body_range) = body_range {
            let body_tree = self.shared_body(file, source, body_range.clone())?;
            let scopes = ScopeTree::from_function(
                source,
                decl.start,
                Some(&sig),
                body_range.start,
                &body_tree,
                cursor_offset,
            );
            let mut region = RegionInfo::new(Some(owner), span, scopes);
            region
                .defs
                .push(([sym.start as u32, sym.end as u32], decl_object(owner)));
            {
                let mut checker = RegionChecker::new(
                    &mut self.arena,
                    &mut self.decls,
                    file,
                    source,
                    body_range.start,
                    &body_tree,
                    &mut region,
                    None,
                    cursor_offset,
                );
                checker.set_type_param_owner(owner);
                checker.populate_signature(header_slice, decl.start, &sig);
                checker.check_body();
            }
            region.finish_local_diagnostics(source);
            region.finalize();
            Some((
                region,
                Some((body_range.start, body_tree)),
                Some((decl.start, sig)),
            ))
        } else {
            let empty_tree = syntax::parse_block("{}");
            let scopes = ScopeTree::from_function(
                source,
                decl.start,
                Some(&sig),
                decl.end,
                &empty_tree,
                None,
            );
            let mut region = RegionInfo::new(Some(owner), span, scopes);
            region
                .defs
                .push(([sym.start as u32, sym.end as u32], decl_object(owner)));
            {
                let mut checker = RegionChecker::new(
                    &mut self.arena,
                    &mut self.decls,
                    file,
                    source,
                    decl.end,
                    &empty_tree,
                    &mut region,
                    None,
                    None,
                );
                checker.set_type_param_owner(owner);
                checker.populate_signature(header_slice, decl.start, &sig);
            }
            region.finish_local_diagnostics(source);
            region.finalize();
            Some((region, None, Some((decl.start, sig))))
        }
    }

    fn build_constant_group_region(
        &mut self,
        file: &'a Path,
        source: &str,
        stub: &'a crate::index::IndexedFile,
        group: [u32; 2],
        cursor_offset: Option<usize>,
    ) -> bool {
        let Some(region) =
            self.compute_constant_group_region(file, source, stub, group, cursor_offset)
        else {
            return false;
        };
        let span = region.span;
        self.cached = Some(CachedRegion {
            file,
            span,
            coverage: RegionCoverage::from_cursor(cursor_offset),
            region,
        });
        true
    }

    fn compute_constant_group_region(
        &mut self,
        file: &'a Path,
        source: &str,
        stub: &'a crate::index::IndexedFile,
        group: [u32; 2],
        cursor_offset: Option<usize>,
    ) -> Option<RegionInfo<'a>> {
        let [g_start, g_end] = group;
        if (g_end.saturating_sub(g_start) as usize) > MAX_BYTES {
            return None;
        }
        let group_slice = source.get(g_start as usize..g_end as usize)?;
        let tree = syntax::parse_declaration(group_slice);
        self.prepare_for_new_region();

        let scopes = ScopeTree::from_top_level_decl(source, g_start as usize, &tree, cursor_offset);
        let mut region = RegionInfo::new(None, group, scopes);
        for s in &stub.symbols {
            if s.start as u32 >= g_start && s.end as u32 <= g_end && (s.is_const() || s.is_var()) {
                let decl = DeclId { file, symbol: s };
                region
                    .defs
                    .push(([s.start as u32, s.end as u32], decl_object(decl)));
            }
        }
        {
            let mut checker = RegionChecker::new(
                &mut self.arena,
                &mut self.decls,
                file,
                source,
                g_start as usize,
                &tree,
                &mut region,
                None,
                cursor_offset,
            );
            checker.check_body();
        }
        region.finish_local_diagnostics(source);
        region.finalize();
        Some(region)
    }

    fn build_value_spec_region(
        &mut self,
        file: &'a Path,
        source: &str,
        stub: &'a crate::index::IndexedFile,
        sym: &'a IndexedSymbol,
        cursor_offset: Option<usize>,
    ) -> bool {
        let Some(region) = self.compute_value_spec_region(file, source, stub, sym, cursor_offset)
        else {
            return false;
        };
        let span = region.span;
        self.cached = Some(CachedRegion {
            file,
            span,
            coverage: RegionCoverage::from_cursor(cursor_offset),
            region,
        });
        true
    }

    fn compute_value_spec_region(
        &mut self,
        file: &'a Path,
        source: &str,
        stub: &'a crate::index::IndexedFile,
        sym: &'a IndexedSymbol,
        cursor_offset: Option<usize>,
    ) -> Option<RegionInfo<'a>> {
        let decl = sym.declaration()?;
        if decl.len() > MAX_BYTES {
            return None;
        }
        let spec_slice = source.get(decl.clone())?;
        let kind = if sym.is_const() {
            DeclSpecKind::Const
        } else {
            DeclSpecKind::Var
        };
        let tree = syntax::parse_decl_spec(spec_slice, kind);
        self.prepare_for_new_region();

        let span = [decl.start as u32, decl.end as u32];
        let scopes = ScopeTree::from_top_level_decl(source, decl.start, &tree, cursor_offset);
        let owner = DeclId { file, symbol: sym };
        let mut region = RegionInfo::new(Some(owner), span, scopes);
        for s in &stub.symbols {
            if s.start >= decl.start && s.end <= decl.end && (s.is_var() || s.is_const()) {
                let id = DeclId { file, symbol: s };
                region
                    .defs
                    .push(([s.start as u32, s.end as u32], decl_object(id)));
            }
        }
        {
            let mut checker = RegionChecker::new(
                &mut self.arena,
                &mut self.decls,
                file,
                source,
                decl.start,
                &tree,
                &mut region,
                sym.is_const().then_some(0),
                cursor_offset,
            );
            checker.check_decl_spec(sym.is_const());
        }
        if cursor_offset.is_none() && tree.diagnostics().is_empty() {
            for s in &stub.symbols {
                if s.start >= decl.start
                    && s.end <= decl.end
                    && (s.is_var() || s.is_const())
                    && !stub
                        .constant_groups
                        .iter()
                        .any(|g| (g[0] as usize) <= s.start && s.start < (g[1] as usize))
                    && let Some(local_id) = region.scopes.definition_at(s.start as u32)
                    && let Some(op) = region.local_type(local_id)
                {
                    self.decls.seed_value(DeclId { file, symbol: s }, op);
                }
            }
        }
        region.finish_local_diagnostics(source);
        region.finalize();
        Some(region)
    }

    fn build_type_decl_region(
        &mut self,
        file: &'a Path,
        source: &str,
        sym: &'a IndexedSymbol,
        cursor_offset: Option<usize>,
    ) -> bool {
        let Some(region) = self.compute_type_decl_region(file, source, sym, cursor_offset) else {
            return false;
        };
        let span = region.span;
        self.cached = Some(CachedRegion {
            file,
            span,
            coverage: RegionCoverage::from_cursor(cursor_offset),
            region,
        });
        true
    }

    fn compute_type_decl_region(
        &mut self,
        file: &'a Path,
        source: &str,
        sym: &'a IndexedSymbol,
        cursor_offset: Option<usize>,
    ) -> Option<RegionInfo<'a>> {
        let decl = sym.declaration()?;
        if decl.len() > MAX_BYTES {
            return None;
        }
        let spec_slice = source.get(decl.clone())?;
        let tree = syntax::parse_decl_spec(spec_slice, DeclSpecKind::Type);
        self.prepare_for_new_region();

        let span = [decl.start as u32, decl.end as u32];
        let scopes = ScopeTree::empty(span);
        let owner = DeclId { file, symbol: sym };
        let mut region = RegionInfo::new(Some(owner), span, scopes);
        region
            .defs
            .push(([sym.start as u32, sym.end as u32], decl_object(owner)));
        {
            let mut checker = RegionChecker::new(
                &mut self.arena,
                &mut self.decls,
                file,
                source,
                decl.start,
                &tree,
                &mut region,
                None,
                cursor_offset,
            );
            checker.check_top_level_type_spec(sym);
        }
        region.finish_local_diagnostics(source);
        region.finalize();
        Some(region)
    }
}
