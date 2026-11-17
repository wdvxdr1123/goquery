//! Bounded cursor-region cache and streaming region iteration for [`Semantics`].

use std::path::Path;

use crate::analysis::budget::MAX_BYTES;
use crate::analysis::check::decls::DeclCache;
use crate::analysis::check::region::RegionInfo;
use crate::analysis::check::{RegionChecker, decl_object};
use crate::analysis::obj::DeclId;
use crate::analysis::scope::tree::ScopeTree;
use crate::analysis::types::TypeArena;
use crate::index::IndexedSymbol;
use crate::syntax::{self, DeclSpecKind};

use super::Semantics;

pub(super) struct CachedRegion<'a> {
    pub(super) file: &'a Path,
    pub(super) span: [u32; 2],
    pub(super) cursor_offset: Option<usize>,
    pub(super) region: RegionInfo<'a>,
}

impl<'a> Semantics<'a> {
    pub(super) fn prepare_for_new_region(&mut self) {
        let declarations = self
            .decls
            .temporary_declarations(self.base_mark.node_count());
        self.failed = None;
        self.cached = None;
        self.arena.rewind(self.base_mark);
        self.decls.rewind_above(self.arena.node_count());
        // Evict the declaration prefix when it would crowd the next body.
        // A cache miss is preferable to exhausting another function's locals.
        if self.arena.needs_rewind() {
            self.arena.rewind(self.root_mark);
            self.decls.rewind_above(self.arena.node_count());
        }
        self.arena.reset_steps();
        for declaration in declarations {
            let _ = self.decls.operand_of(&mut self.arena, declaration);
            if self.arena.needs_rewind() {
                break;
            }
        }
        if self.arena.needs_rewind() {
            self.arena.rewind(self.root_mark);
            self.decls.rewind_above(self.arena.node_count());
        }
        self.decls.rewind_above(self.arena.node_count());
        self.base_mark = self.arena.mark();
        self.arena.reset_steps();
    }

    pub(super) fn ensure_region(
        &mut self,
        file: &'a Path,
        source: &str,
        offset: u32,
        cursor_offset: Option<usize>,
    ) -> bool {
        if let Some(cached) = self.cached.as_ref()
            && cached.file == file
            && cached.span[0] <= offset
            && offset <= cached.span[1]
            && cached.cursor_offset == cursor_offset
        {
            self.arena.reset_steps();
            return true;
        }
        if let Some((f_file, f_span, f_cursor)) = self.failed
            && f_file == file
            && f_span[0] <= offset
            && offset <= f_span[1]
            && f_cursor == cursor_offset
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
                    self.failed = Some((file, [decl.start as u32, decl.end as u32], cursor_offset));
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
                    self.failed = Some((file, [g_start, g_end], cursor_offset));
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
                    self.failed = Some((file, [decl.start as u32, decl.end as u32], cursor_offset));
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
                    self.failed = Some((file, [decl.start as u32, decl.end as u32], cursor_offset));
                }
                return built;
            }
        }

        false
    }

    pub(crate) fn arena_and_decls(&mut self) -> (&mut TypeArena<'a>, &mut DeclCache<'a>) {
        (&mut self.arena, &mut self.decls)
    }

    pub(crate) fn stream_file_regions(
        &mut self,
        file: &'a Path,
        source: &str,
        window: Option<std::ops::Range<usize>>,
        mut visit: impl FnMut(&RegionInfo<'a>, &mut TypeArena<'a>, &mut DeclCache<'a>),
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

        if let Some(ref w) = window {
            items.retain(|(span, _)| (span[0] as usize) < w.end && (span[1] as usize) > w.start);
        }
        items.sort_by_key(|(span, _)| (span[0], span[1]));

        let mut prev_end = 0u32;
        for (span, item) in items {
            if span[1] <= prev_end {
                continue;
            }
            prev_end = span[1];
            let region = match item {
                RegionItem::Function(sym) => self.compute_function_region(file, source, sym, None),
                RegionItem::ConstGroup(group) => {
                    self.compute_constant_group_region(file, source, stub, group, None)
                }
                RegionItem::ValueSpec(sym) => {
                    self.compute_value_spec_region(file, source, stub, sym, None)
                }
                RegionItem::TypeDecl(sym) => self.compute_type_decl_region(file, source, sym, None),
            };
            if let Some(region) = region {
                visit(&region, &mut self.arena, &mut self.decls);
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
        let Some(region) = self.compute_function_region(file, source, sym, cursor_offset) else {
            return false;
        };
        let span = region.span;
        self.cached = Some(CachedRegion {
            file,
            span,
            cursor_offset,
            region,
        });
        true
    }

    fn compute_function_region(
        &mut self,
        file: &'a Path,
        source: &str,
        sym: &'a IndexedSymbol,
        cursor_offset: Option<usize>,
    ) -> Option<RegionInfo<'a>> {
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
            Some(region)
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
            Some(region)
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
            cursor_offset,
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
            cursor_offset,
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
            cursor_offset,
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
