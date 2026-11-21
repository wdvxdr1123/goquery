//! Bounded cursor-region cache and streaming region iteration for [`Semantics`].

use std::path::Path;

use crate::analysis::engine::budget::MAX_BYTES;
use crate::analysis::engine::check::decls::DeclTable;
use crate::analysis::engine::check::region::{RecordMask, RegionInfo};
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
    pub(super) mask: RecordMask,
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
        self.ensure_region_with_mask(file, source, offset, cursor_offset, RecordMask::ALL)
    }

    pub(super) fn region_and_arena_at_with_mask(
        &mut self,
        file: &Path,
        source: &str,
        offset: u32,
        cursor_offset: Option<usize>,
        mask: RecordMask,
    ) -> Option<(&RegionInfo<'a>, &mut TypeArena<'a>)> {
        let file = self.intern_file(file)?;
        if !self.ensure_region_with_mask(file, source, offset, cursor_offset, mask) {
            return None;
        }
        let cached = self.cached.as_ref()?;
        Some((&cached.region, &mut self.arena))
    }

    pub(super) fn ensure_region_with_mask(
        &mut self,
        file: &'a Path,
        source: &str,
        offset: u32,
        cursor_offset: Option<usize>,
        mask: RecordMask,
    ) -> bool {
        let needed = RegionCoverage::from_cursor(cursor_offset);
        if let Some(cached) = self.cached.as_ref()
            && cached.file == file
            && cached.span[0] <= offset
            && offset <= cached.span[1]
            && cached.coverage.satisfies(needed)
            && cached.mask.contains(mask)
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
                let built =
                    self.build_function_region_with_mask(file, source, sym, cursor_offset, mask);
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
                    mask,
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
                let built =
                    self.build_value_spec_region(file, source, stub, sym, cursor_offset, mask);
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
                let built = self.build_type_decl_region(file, source, sym, cursor_offset, mask);
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
        visit: impl FnMut(
            &RegionInfo<'a>,
            &mut TypeArena<'a>,
            &mut DeclTable<'a>,
            Option<(usize, &syntax::BlockTree)>,
            Option<(usize, &syntax::Signature)>,
        ),
    ) {
        self.stream_matching_file_regions_with_mask(file, source, RecordMask::ALL, include, visit);
    }

    pub(super) fn stream_matching_file_regions_with_mask(
        &mut self,
        file: &'a Path,
        source: &str,
        mask: RecordMask,
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
                        self.compute_function_region_parts(file, source, sym, None, mask)
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
                        self.compute_constant_group_region(file, source, stub, group, None, mask)
                    {
                        visit(&region, &mut self.arena, &mut self.decls, None, None);
                    }
                }
                RegionItem::ValueSpec(sym) => {
                    if let Some(region) =
                        self.compute_value_spec_region(file, source, stub, sym, None, mask)
                    {
                        visit(&region, &mut self.arena, &mut self.decls, None, None);
                    }
                }
                RegionItem::TypeDecl(sym) => {
                    if let Some(region) =
                        self.compute_type_decl_region(file, source, sym, None, mask)
                    {
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
        self.build_function_region_with_mask(file, source, sym, cursor_offset, RecordMask::ALL)
    }

    fn build_function_region_with_mask(
        &mut self,
        file: &'a Path,
        source: &str,
        sym: &'a IndexedSymbol,
        cursor_offset: Option<usize>,
        mask: RecordMask,
    ) -> bool {
        let Some((region, _, _)) =
            self.compute_function_region_parts(file, source, sym, cursor_offset, mask)
        else {
            return false;
        };
        let span = region.span;
        self.cached = Some(CachedRegion {
            file,
            span,
            coverage: RegionCoverage::from_cursor(cursor_offset),
            mask,
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
        mask: RecordMask,
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
            let mut region = RegionInfo::with_mask(Some(owner), span, scopes, mask);
            region.push_def([sym.start as u32, sym.end as u32], decl_object(owner));
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
            if mask.contains(RecordMask::DIAGNOSTICS) {
                region.finish_local_diagnostics(source);
            }
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
            let mut region = RegionInfo::with_mask(Some(owner), span, scopes, mask);
            region.push_def([sym.start as u32, sym.end as u32], decl_object(owner));
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
            if mask.contains(RecordMask::DIAGNOSTICS) {
                region.finish_local_diagnostics(source);
            }
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
        mask: RecordMask,
    ) -> bool {
        let Some(region) =
            self.compute_constant_group_region(file, source, stub, group, cursor_offset, mask)
        else {
            return false;
        };
        let span = region.span;
        self.cached = Some(CachedRegion {
            file,
            span,
            coverage: RegionCoverage::from_cursor(cursor_offset),
            mask,
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
        mask: RecordMask,
    ) -> Option<RegionInfo<'a>> {
        let [g_start, g_end] = group;
        if (g_end.saturating_sub(g_start) as usize) > MAX_BYTES {
            return None;
        }
        let group_slice = source.get(g_start as usize..g_end as usize)?;
        let tree = syntax::parse_declaration(group_slice);
        self.prepare_for_new_region();

        let scopes = ScopeTree::from_top_level_decl(source, g_start as usize, &tree, cursor_offset);
        let mut region = RegionInfo::with_mask(None, group, scopes, mask);
        for s in &stub.symbols {
            if s.start as u32 >= g_start && s.end as u32 <= g_end && (s.is_const() || s.is_var()) {
                let decl = DeclId { file, symbol: s };
                region.push_def([s.start as u32, s.end as u32], decl_object(decl));
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
        if mask.contains(RecordMask::DIAGNOSTICS) {
            region.finish_local_diagnostics(source);
        }
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
        mask: RecordMask,
    ) -> bool {
        let Some(region) =
            self.compute_value_spec_region(file, source, stub, sym, cursor_offset, mask)
        else {
            return false;
        };
        let span = region.span;
        self.cached = Some(CachedRegion {
            file,
            span,
            coverage: RegionCoverage::from_cursor(cursor_offset),
            mask,
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
        mask: RecordMask,
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
        let mut region = RegionInfo::with_mask(Some(owner), span, scopes, mask);
        for s in &stub.symbols {
            if s.start >= decl.start && s.end <= decl.end && (s.is_var() || s.is_const()) {
                let id = DeclId { file, symbol: s };
                region.push_def([s.start as u32, s.end as u32], decl_object(id));
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
        if mask.contains(RecordMask::DIAGNOSTICS) {
            region.finish_local_diagnostics(source);
        }
        region.finalize();
        Some(region)
    }

    fn build_type_decl_region(
        &mut self,
        file: &'a Path,
        source: &str,
        sym: &'a IndexedSymbol,
        cursor_offset: Option<usize>,
        mask: RecordMask,
    ) -> bool {
        let Some(region) = self.compute_type_decl_region(file, source, sym, cursor_offset, mask)
        else {
            return false;
        };
        let span = region.span;
        self.cached = Some(CachedRegion {
            file,
            span,
            coverage: RegionCoverage::from_cursor(cursor_offset),
            mask,
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
        mask: RecordMask,
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
        let mut region = RegionInfo::with_mask(Some(owner), span, scopes, mask);
        region.push_def([sym.start as u32, sym.end as u32], decl_object(owner));
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
        if mask.contains(RecordMask::DIAGNOSTICS) {
            region.finish_local_diagnostics(source);
        }
        region.finalize();
        Some(region)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::engine::check::diagnostics::Code;
    use crate::analysis::engine::obj::ObjectRef;
    use crate::workspace::{self, Workspace};
    use serde_json::json;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TempMod {
        root: PathBuf,
    }

    impl TempMod {
        fn new(name: &str, files: &[(&str, &str)]) -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root = std::env::temp_dir().join(format!(
                "goquery_phase1_{name}_{}_{nanos}",
                std::process::id()
            ));
            std::fs::create_dir_all(&root).unwrap();
            for (rel, content) in files {
                let path = root.join(rel);
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).unwrap();
                }
                std::fs::write(&path, content).unwrap();
            }
            Self { root }
        }

        fn workspace(&self) -> Workspace {
            let mut ws = Workspace::default();
            ws.initialize(&json!({
                "rootUri": workspace::uri(&self.root),
                "initializationOptions": {
                    "goroot": "",
                    "fileWatch": false,
                }
            }))
            .unwrap();
            ws
        }
    }

    impl Drop for TempMod {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn record_mask_skips_tables_and_upgrades_cache() {
        let dir = TempMod::new(
            "mask",
            &[
                ("go.mod", "module example.com/m\n\ngo 1.24\n"),
                (
                    "dep/dep.go",
                    "package dep\n\nfunc Helper() int { return 1 }\n",
                ),
                (
                    "main.go",
                    "package main\n\nimport \"example.com/m/dep\"\n\ntype I interface { M() }\ntype S struct{}\nfunc (S) M() {}\n\nfunc run() int {\n\tunused := 1\n\tused := dep.Helper()\n\tused = used + 1\n\treturn used\n}\n",
                ),
            ],
        );
        let ws = dir.workspace();
        let main_path = ws
            .files
            .keys()
            .find(|p| p.ends_with("main.go"))
            .unwrap()
            .as_path();
        let source = ws.source(main_path).unwrap();
        let run_offset = source.find("unused").unwrap() as u32;

        let mut sema = Semantics::new(&ws, ws.build_view(None));

        // 1. DIAGNOSTICS-only skips non-diagnostic tables while still catching UnusedVar.
        assert!(sema.ensure_region_with_mask(
            main_path,
            &source,
            run_offset,
            None,
            RecordMask::DIAGNOSTICS,
        ));
        let cached = sema.cached.as_ref().unwrap();
        assert_eq!(cached.mask, RecordMask::DIAGNOSTICS);
        assert!(cached.region.defs.is_empty());
        assert!(cached.region.uses.is_empty());
        assert!(cached.region.expr_types.is_empty());
        assert!(cached.region.calls.is_empty());
        assert!(cached.region.selections.is_empty());
        assert!(cached.region.instances.is_empty());
        assert!(cached.region.expected_types.is_empty());
        assert!(cached.region.writes.is_empty());
        assert!(cached.region.diagnostics.iter().any(|d| {
            d.kind == crate::analysis::engine::check::diagnostics::Kind::Compiler(Code::UnusedVar)
                && d.message.contains("unused")
        }));
        assert!(
            !cached
                .region
                .diagnostics
                .iter()
                .any(|d| d.message.contains("used ="))
        );

        // 2. Requesting a wider mask upgrades the cache; requesting a subset hits it.
        let nav =
            RecordMask::DEFS | RecordMask::USES | RecordMask::IMPLICITS | RecordMask::SELECTIONS;
        assert!(sema.ensure_region_with_mask(main_path, &source, run_offset, None, nav));
        let cached = sema.cached.as_ref().unwrap();
        assert_eq!(cached.mask, nav);
        assert!(!cached.region.uses.is_empty());
        assert!(cached.region.expr_types.is_empty());
        assert!(cached.region.calls.is_empty());
        assert!(cached.region.diagnostics.is_empty());

        // Real import_path_span is populated on PkgName (even for a closed file).
        let dep_use = source.find("dep.Helper").unwrap();
        let dep_span = [dep_use as u32, (dep_use + 3) as u32];
        let expected_import_start = source.find("\"example.com/m/dep\"").unwrap() as u32;
        let expected_import_span = [
            expected_import_start,
            expected_import_start + "\"example.com/m/dep\"".len() as u32,
        ];
        match cached.region.use_by_span(dep_span) {
            Some(ObjectRef::PkgName {
                import_path_span, ..
            }) => assert_eq!(import_path_span, expected_import_span),
            other => panic!("expected PkgName with real span, got {other:?}"),
        }

        // 3. Relation probe preserves `self.cached` instead of thrashing it.
        let stub = ws.files.get(main_path).unwrap();
        let s_sym = stub.symbols.iter().find(|s| s.name == "S").unwrap();
        let i_sym = stub.symbols.iter().find(|s| s.name == "I").unwrap();
        let s_decl = DeclId {
            file: main_path,
            symbol: s_sym,
        };
        let i_decl = DeclId {
            file: main_path,
            symbol: i_sym,
        };
        assert_eq!(
            sema.relations().implementation_matches(s_decl, i_decl),
            Some(true)
        );
        assert!(
            sema.cached.is_some(),
            "speculative relation probe should preserve cached region"
        );
    }
}
