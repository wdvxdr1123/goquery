//! Tier 1 demand-driven declaration resolution with cycle detection (`obj_path`).

use crate::analysis::budget::{MAX_BYTES, MAX_DECL_CACHE, MAX_DEPTH};
use crate::analysis::check::region::RegionInfo;
use crate::analysis::check::{ExprOutcome, RegionChecker};
use crate::analysis::obj::DeclId;
use crate::analysis::resolve::UnqualifiedLookup;
use crate::analysis::scope::tree::ScopeTree;
use crate::analysis::types::{Mode, Operand, TypeArena, TypeId};
use crate::syntax::{DeclSpecKind, ExprId, StmtKind};
use crate::workspace::Workspace;
use std::collections::HashMap;
use std::ops::{Bound, Range};
use std::path::Path;

#[derive(Clone, Copy)]
enum DeclSlot {
    Checking,
    Ready { operand: Operand, arena_len: u16 },
    InvalidVar,
    Failed,
}

#[derive(Default)]
struct FileScopeMemo<'a> {
    imports: HashMap<Box<str>, Option<&'a Path>>,
    imported_decls: HashMap<Box<str>, HashMap<Box<str>, Option<DeclId<'a>>>>,
    imported_decl_count: usize,
    unqualified: HashMap<Box<str>, UnqualifiedLookup<'a>>,
}

/// Request-scoped cache of cross-file / cross-package declaration operands.
/// Only declaration header slices are read on demand; bodies are never parsed here.
pub(crate) struct DeclCache<'a> {
    slots: HashMap<DeclId<'a>, DeclSlot>,
    obj_path: Vec<DeclId<'a>>,
    file_scope: Option<(&'a Path, FileScopeMemo<'a>)>,
    view: crate::workspace::BuildView<'a>,
}

impl<'a> DeclCache<'a> {
    /// Only canonical declarations cross a region transition. Their operands
    /// are recreated before local allocations so the permanent prefix cannot
    /// retain another function's formals or local type facts.
    pub(crate) fn temporary_declarations(&self, permanent_nodes: u16) -> Vec<DeclId<'a>> {
        let mut declarations: Vec<_> = self
            .slots
            .iter()
            .filter_map(|(decl, slot)| {
                matches!(slot, DeclSlot::Ready {arena_len,..} if *arena_len > permanent_nodes)
                    .then_some(*decl)
            })
            .collect();
        declarations.sort_by_key(|decl| (decl.file, decl.symbol.start));
        declarations.truncate(crate::analysis::budget::MAX_DECL_PROMOTIONS);
        declarations
    }

    pub(crate) fn new(view: crate::workspace::BuildView<'a>) -> Self {
        Self {
            view,
            slots: HashMap::new(),
            obj_path: Vec::new(),
            file_scope: None,
        }
    }

    pub(crate) fn rewind_above(&mut self, max_nodes: u16) {
        self.slots.retain(|_, slot| {
            matches!(slot, DeclSlot::Ready { arena_len, .. } if *arena_len <= max_nodes)
                || matches!(slot, DeclSlot::InvalidVar)
        });
    }

    fn ensure_file_scope(&mut self, file: &'a Path) -> Option<&mut FileScopeMemo<'a>> {
        if self.file_scope.is_none() {
            self.file_scope = Some((file, FileScopeMemo::default()));
        }
        self.file_scope
            .as_mut()
            .and_then(|(cached_file, memo)| (*cached_file == file).then_some(memo))
    }

    pub(crate) fn find_import_dir(
        &mut self,
        ws: &'a Workspace,
        file: &'a Path,
        alias: &str,
    ) -> Option<&'a Path> {
        let view = self.view;
        let file = ws.files.get_key_value(file)?.0.as_path();
        if let Some(memo) = self.ensure_file_scope(file) {
            if let Some(&cached) = memo.imports.get(alias) {
                return cached;
            }
            let target_dir = Self::compute_import_dir(ws, view, file, alias);
            if (target_dir.is_some() && memo.imports.len() < MAX_DECL_CACHE)
                || memo.imports.len() < MAX_DECL_CACHE / 2
            {
                memo.imports.insert(alias.into(), target_dir);
            }
            return target_dir;
        }
        Self::compute_import_dir(ws, view, file, alias)
    }

    fn compute_import_dir(
        ws: &'a Workspace,
        view: crate::workspace::BuildView<'a>,
        file: &'a Path,
        alias: &str,
    ) -> Option<&'a Path> {
        let imp = ws.files.get(file)?.imports.iter().find(|imp| {
            !imp.is_dot()
                && !imp.is_blank()
                && crate::analysis::resolve::View::new(ws, view).import_name(file, imp)
                    == Some(alias)
        })?;
        if let Some((p, _)) = crate::analysis::resolve::View::new(ws, view)
            .imported_symbol_iter(file, alias)
            .next()
        {
            return p.parent();
        }
        let import_path = imp.path.as_str();
        for (mod_root, mod_path) in &ws.modules {
            let suffix = if mod_path == "std" {
                import_path
            } else if import_path == mod_path {
                ""
            } else if let Some(suffix) = import_path
                .strip_prefix(mod_path.as_str())
                .and_then(|suffix| suffix.strip_prefix('/'))
            {
                suffix
            } else {
                continue;
            };
            let candidate = mod_root.join(suffix);
            if ws.directory_import_path(&candidate).as_deref() != Some(import_path) {
                continue;
            }
            if let Some((file_path, _)) = ws
                .files
                .range::<Path, _>((Bound::Included(candidate.as_path()), Bound::Unbounded))
                .take_while(|(path, _)| path.starts_with(&candidate))
                .find(|(path, _)| {
                    path.parent() == Some(candidate.as_path()) && view.visible(ws, path)
                })
            {
                return file_path.parent();
            }
        }
        None
    }

    pub(crate) fn resolve_unqualified(
        &mut self,
        ws: &'a Workspace,
        file: &'a Path,
        name: &str,
    ) -> UnqualifiedLookup<'a> {
        let view = self.view;
        let Some(file) = ws.files.get_key_value(file).map(|(p, _)| p.as_path()) else {
            return UnqualifiedLookup::NotFound;
        };
        if let Some(memo) = self.ensure_file_scope(file) {
            if let Some(&cached) = memo.unqualified.get(name) {
                return cached;
            }
            let result =
                crate::analysis::resolve::View::new(ws, view).unqualified_lookup(file, name);
            if memo.unqualified.len() < MAX_DECL_CACHE {
                memo.unqualified.insert(name.into(), result);
            }
            return result;
        }
        crate::analysis::resolve::View::new(ws, view).unqualified_lookup(file, name)
    }

    pub(crate) fn resolve_imported(
        &mut self,
        ws: &'a Workspace,
        file: &'a Path,
        alias: &str,
        name: &str,
    ) -> Option<DeclId<'a>> {
        let view = self.view;
        let file = ws.files.get_key_value(file)?.0.as_path();
        if let Some(memo) = self.ensure_file_scope(file) {
            if let Some(&cached) = memo.imported_decls.get(alias).and_then(|m| m.get(name)) {
                return cached;
            }
            let resolved = Self::resolve_imported_uncached(ws, view, file, alias, name);
            if memo.imported_decl_count < MAX_DECL_CACHE {
                let members = memo.imported_decls.entry(alias.into()).or_default();
                if !members.contains_key(name) {
                    members.insert(name.into(), resolved);
                    memo.imported_decl_count += 1;
                }
            }
            return resolved;
        }
        Self::resolve_imported_uncached(ws, view, file, alias, name)
    }

    fn resolve_imported_uncached(
        ws: &'a Workspace,
        view: crate::workspace::BuildView<'a>,
        file: &'a Path,
        alias: &str,
        name: &str,
    ) -> Option<DeclId<'a>> {
        let probe = crate::bloom::Fingerprint::new(name.as_bytes());
        let mut candidates = crate::analysis::resolve::View::new(ws, view)
            .imported_symbol_candidates(file, alias, Some(probe))
            .filter(|(_, symbol)| symbol.name == name);
        let (file, symbol) = candidates.next()?;
        candidates
            .next()
            .is_none()
            .then_some(DeclId { file, symbol })
    }

    pub(crate) fn operand_of(
        &mut self,
        arena: &mut TypeArena<'a>,
        decl: DeclId<'a>,
    ) -> Option<Operand> {
        let file = arena
            .workspace()
            .files
            .get_key_value(decl.file)?
            .0
            .as_path();
        let decl = DeclId {
            file,
            symbol: decl.symbol,
        };
        if let Some(slot) = self.slots.get(&decl).copied() {
            return match slot {
                DeclSlot::Ready { operand, .. } => Some(operand),
                DeclSlot::Checking | DeclSlot::InvalidVar | DeclSlot::Failed => None,
            };
        }
        if self.slots.len() >= MAX_DECL_CACHE || self.obj_path.len() >= MAX_DEPTH {
            return None;
        }
        self.slots.insert(decl, DeclSlot::Checking);
        self.obj_path.push(decl);
        let resolved = self.resolve_uncached(arena, decl);
        self.obj_path.pop();
        match resolved {
            Some(operand) if operand.mode == Mode::Invalid => {
                self.slots.insert(decl, DeclSlot::InvalidVar);
                None
            }
            Some(operand) => {
                self.slots.insert(
                    decl,
                    DeclSlot::Ready {
                        operand,
                        arena_len: arena.node_count(),
                    },
                );
                Some(operand)
            }
            None => {
                self.slots.insert(decl, DeclSlot::Failed);
                None
            }
        }
    }

    pub(super) fn invalid_var(&self, decl: DeclId<'a>) -> bool {
        matches!(self.slots.get(&decl), Some(DeclSlot::InvalidVar))
    }

    pub(crate) fn type_of(
        &mut self,
        arena: &mut TypeArena<'a>,
        decl: DeclId<'a>,
    ) -> Option<TypeId> {
        self.operand_of(arena, decl).map(|op| op.ty)
    }

    fn resolve_uncached(&mut self, arena: &mut TypeArena<'a>, decl: DeclId<'a>) -> Option<Operand> {
        if decl.symbol.is_type() {
            if decl.symbol.generic {
                return None;
            }
            // unsafe.Pointer is a compiler-defined basic type; its SDK doc stub `*ArbitraryType`
            // must not be treated as a dereferenceable pointer type.
            if arena.workspace().import_path(decl.file).as_deref() == Some("unsafe") {
                if decl.symbol.name == "Pointer" {
                    let ty = arena.basic(crate::analysis::types::BasicKind::UnsafePointer)?;
                    return Some(Operand {
                        mode: Mode::TypeExpr,
                        ty,
                        value: None,
                    });
                }
                if matches!(decl.symbol.name.as_str(), "ArbitraryType" | "IntegerType") {
                    return None;
                }
            }
            let ty = arena.push_named(decl, &[], decl.symbol.alias)?;
            if decl.symbol.alias {
                arena.unalias(ty)?;
            }
            return Some(Operand {
                mode: Mode::TypeExpr,
                ty,
                value: None,
            });
        }
        if decl.symbol.is_function() || decl.symbol.is_method() {
            return self.resolve_signature(arena, decl);
        }
        if decl.symbol.is_const() || decl.symbol.is_var() {
            return self.resolve_value(arena, decl);
        }
        None
    }

    fn resolve_signature(&self, arena: &mut TypeArena<'a>, decl: DeclId<'a>) -> Option<Operand> {
        let workspace = arena.workspace();
        let declaration = decl.symbol.declaration()?;
        let end = decl
            .symbol
            .body()
            .map_or(declaration.end, |body| body.start);
        if end.saturating_sub(declaration.start) > MAX_BYTES {
            return None;
        }
        let header = workspace.source_slice(decl.file, declaration.start..end)?;
        let signature = crate::syntax::parse_signature(&header);
        if !signature.diagnostics.is_empty() || !signature.type_parameters.is_empty() {
            return None;
        }
        if let Some(receiver) = signature.receiver.first()
            && !crate::analysis::scope::receiver_parameters(&header, &receiver.annotation)
                .is_empty()
        {
            return None;
        }
        let ty = arena.signature(
            decl.file,
            &header,
            Some(declaration.start),
            &signature,
            &[],
            0,
        )?;
        Some(Operand {
            mode: Mode::Value,
            ty,
            value: None,
        })
    }

    fn resolve_value(&mut self, arena: &mut TypeArena<'a>, decl: DeclId<'a>) -> Option<Operand> {
        let workspace = arena.workspace();
        let constant = decl.symbol.is_const();
        let group = constant
            .then(|| {
                workspace
                    .files
                    .get(decl.file)?
                    .constant_groups
                    .iter()
                    .find(|span| {
                        span[0] as usize <= decl.symbol.start
                            && decl.symbol.start < span[1] as usize
                    })
            })
            .flatten();
        let span = group
            .map(|span| span[0] as usize..span[1] as usize)
            .or_else(|| decl.symbol.declaration())?;
        if span.len() > MAX_BYTES {
            return None;
        }
        let source = workspace.source_slice(decl.file, span.clone())?;
        let parsed = if group.is_some() {
            crate::syntax::parse_declaration(&source)
        } else {
            let kind = if constant {
                DeclSpecKind::Const
            } else {
                DeclSpecKind::Var
            };
            crate::syntax::parse_decl_spec(&source, kind)
        };
        if !parsed.diagnostics.is_empty() || parsed.statements.len() > 4096 {
            return None;
        }
        let declarations = match &parsed.statement(parsed.root())?.kind {
            StmtKind::DeclarationGroup { declarations } => declarations.as_slice(),
            StmtKind::Declaration { .. } => std::slice::from_ref(&parsed.root),
            _ => return None,
        };
        let mut previous: Option<(&Option<Range<usize>>, &Vec<ExprId>)> = None;
        for (ordinal, declaration) in declarations.iter().enumerate() {
            let StmtKind::Declaration {
                names,
                annotation,
                values,
                ..
            } = &parsed.statement(*declaration)?.kind
            else {
                return None;
            };
            let inherited = if constant && values.is_empty() {
                if annotation.is_some() {
                    return None;
                }
                previous?
            } else {
                (annotation, values)
            };
            if !values.is_empty() {
                previous = Some((annotation, values));
            }
            let Some(position) = names.iter().position(|name| {
                source.get(name.clone()) == Some(decl.symbol.name.as_str())
                    && span.start.checked_add(name.start) == Some(decl.symbol.start)
            }) else {
                continue;
            };
            if names.len() > 256 {
                return None;
            }
            let (annotation, values) = inherited;
            let scopes = ScopeTree::from_function(&source, 0, None, 0, &parsed, None);
            let mut region = RegionInfo::new(
                Some(decl),
                [0, u32::try_from(source.len()).unwrap_or(u32::MAX)],
                scopes,
            );
            let mut checker = RegionChecker::new(
                arena,
                self,
                decl.file,
                &source,
                0,
                &parsed,
                &mut region,
                constant.then(|| u32::try_from(ordinal).ok()).flatten(),
                None,
            );
            checker.set_source_base(span.start);
            let target = match annotation {
                Some(ann) => {
                    let ty = checker.resolve_type_syntax_span(ann)?;
                    checker.arena().constructor_map_keys(ty)?;
                    Some(ty)
                }
                None => None,
            };
            // A package variable's explicit type is independent of the value
            // expression. Region checking diagnoses that initializer separately.
            if !constant && let Some(ty) = target {
                return Some(Operand {
                    mode: Mode::Variable,
                    ty,
                    value: None,
                });
            }
            let input = if values.is_empty() {
                if constant {
                    return None;
                }
                return Some(Operand {
                    mode: Mode::Variable,
                    ty: target?,
                    value: None,
                });
            } else if values.len() == names.len() {
                let outcome = checker.check_expr_outcome(values[position], target);
                if !constant && matches!(outcome, ExprOutcome::MissingName) {
                    // A directly missing initializer or callee proves the
                    // inferred variable type invalid, independent of the arena.
                    return Some(Operand::non_value(Mode::Invalid));
                }
                outcome.operand()?
            } else if values.len() == 1 && !constant && names.len() > 1 {
                checker
                    .check_multi_expr(values[0], names.len())?
                    .into_iter()
                    .nth(position)?
            } else {
                return None;
            };
            if !input.mode.is_value() {
                return None;
            }
            let int_bits = checker.arena().integer_bits()?;
            if constant {
                if input.mode != Mode::Constant {
                    return None;
                }
                return match target {
                    Some(target) => checker.arena().assign_basic(input, target, int_bits),
                    None => Some(input),
                };
            }
            let ty = checker.arena().default_operand(input, int_bits)?;
            checker.arena().constructor_map_keys(ty)?;
            return Some(Operand {
                mode: Mode::Variable,
                ty,
                value: None,
            });
        }
        None
    }
}
