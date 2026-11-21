//! Tier 1 demand-driven declaration resolver with cycle detection (`obj_path`),
//! whole `const (...)` group batch caching, generic origin/signature caching,
//! and unified constant-expression evaluation.

use crate::analysis::engine::budget::{MAX_BYTES, MAX_DECL_CACHE, MAX_DEPTH};
use crate::analysis::engine::check::region::{RecordMask, RegionInfo};
use crate::analysis::engine::check::{ExprOutcome, RegionChecker};
use crate::analysis::engine::obj::DeclId;
use crate::analysis::engine::scope::tree::ScopeTree;
use crate::analysis::engine::types::{DepthKind, Mode, Operand, TypeArena, TypeId};
use crate::syntax::{BlockTree, DeclSpecKind, ExprId, ExprKind, StmtId, StmtKind, TypeSyntax};
use std::collections::HashMap;
use std::ops::Range;
use std::path::Path;

/// Maximum sibling constants opportunistically seeded from a single `const (...)`
/// group lookup so very large constant tables stay bounded in step/cache budget.
const MAX_CONST_GROUP_SEED: usize = 256;

#[derive(Clone, Copy)]
enum DeclEntry {
    Checking,
    Ready(Operand),
    GenericType(TypeId),
    InvalidVar,
    Failed,
}

/// Request-scoped resolver and cache of cross-file / cross-package declarations.
/// Only declaration header slices are read on demand; bodies are never parsed here.
pub(crate) struct DeclResolver<'a> {
    slots: HashMap<DeclId<'a>, DeclEntry>,
    obj_path: Vec<DeclId<'a>>,
}

/// Compatibility alias preserved for `analysis::sema` and `analysis::check` callers.
pub(crate) type DeclTable<'a> = DeclResolver<'a>;

/// Evaluate a package-level constant expression node from a [`TypeSyntax`]
/// (e.g. `[N]T` array length) when lowering outside an active [`RegionChecker`].
pub(in crate::analysis::engine) fn eval_package_const_expr<'a>(
    arena: &mut TypeArena<'a>,
    origin: &Path,
    source: &str,
    parsed: &TypeSyntax,
    expr_id: ExprId,
) -> Option<Operand> {
    let mut decls = DeclResolver::new();
    decls.eval_const_expr(arena, origin, source, parsed, expr_id)
}

impl<'a> DeclResolver<'a> {
    pub(crate) fn new() -> Self {
        Self {
            slots: HashMap::new(),
            obj_path: Vec::new(),
        }
    }

    /// Seed a resolved package-level value declaration from an open-file
    /// `RegionInfo` check so subsequent references avoid re-checking its initializer.
    pub(crate) fn seed_value(&mut self, decl: DeclId<'a>, operand: Operand) {
        if self.slots.len() >= MAX_DECL_CACHE
            || self.slots.contains_key(&decl)
            || !operand.ty.is_decl_safe()
        {
            return;
        }
        if operand.mode == Mode::Invalid {
            self.slots.insert(decl, DeclEntry::InvalidVar);
        } else if operand.mode.is_value() {
            self.slots.insert(decl, DeclEntry::Ready(operand));
        }
    }

    /// Resolve and cache the `DeclArena` origin template `TypeId` for a
    /// package-level generic type declaration without changing `operand_of`'s
    /// `None` semantics for uninstantiated generic types.
    pub(crate) fn generic_type_origin(
        &mut self,
        arena: &mut TypeArena<'a>,
        decl: DeclId<'a>,
    ) -> Option<TypeId> {
        if !decl.symbol.is_type() || !decl.symbol.generic {
            return None;
        }
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
        if let Some(entry) = self.slots.get(&decl).copied() {
            return match entry {
                DeclEntry::GenericType(ty) => Some(ty),
                DeclEntry::Ready(op) if op.mode == Mode::TypeExpr => Some(op.ty),
                DeclEntry::Checking
                | DeclEntry::Ready(_)
                | DeclEntry::InvalidVar
                | DeclEntry::Failed => None,
            };
        }
        if self.slots.len() >= MAX_DECL_CACHE || self.obj_path.len() >= MAX_DEPTH {
            return None;
        }
        self.slots.insert(decl, DeclEntry::Checking);
        self.obj_path.push(decl);
        let resolved = arena.with_decl_alloc(|arena| arena.declaration_type(decl));
        self.obj_path.pop();
        match resolved {
            Some(ty) if ty.is_decl_safe() => {
                self.slots.insert(decl, DeclEntry::GenericType(ty));
                Some(ty)
            }
            Some(_) | None => {
                self.slots.insert(decl, DeclEntry::Failed);
                None
            }
        }
    }

    pub(crate) fn operand_of(
        &mut self,
        arena: &mut TypeArena<'a>,
        decl: DeclId<'a>,
    ) -> Option<Operand> {
        // Uninstantiated generic types do not produce a concrete `Mode::TypeExpr`
        // operand here; callers wanting their origin template use `generic_type_origin`.
        if decl.symbol.is_type() && decl.symbol.generic {
            return None;
        }
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
        if let Some(entry) = self.slots.get(&decl).copied() {
            return match entry {
                DeclEntry::Ready(operand) => Some(operand),
                DeclEntry::Checking
                | DeclEntry::GenericType(_)
                | DeclEntry::InvalidVar
                | DeclEntry::Failed => None,
            };
        }
        if self.slots.len() >= MAX_DECL_CACHE || self.obj_path.len() >= MAX_DEPTH {
            return None;
        }
        self.slots.insert(decl, DeclEntry::Checking);
        self.obj_path.push(decl);
        let resolved = arena.with_decl_alloc(|arena| self.resolve_uncached(arena, decl));
        self.obj_path.pop();
        match resolved {
            Some(operand) if operand.mode == Mode::Invalid => {
                self.slots.insert(decl, DeclEntry::InvalidVar);
                None
            }
            Some(operand) if operand.ty.is_decl_safe() => {
                self.slots.insert(decl, DeclEntry::Ready(operand));
                Some(operand)
            }
            Some(_) | None => {
                self.slots.insert(decl, DeclEntry::Failed);
                None
            }
        }
    }

    pub(super) fn invalid_var(&self, decl: DeclId<'a>) -> bool {
        matches!(self.slots.get(&decl), Some(DeclEntry::InvalidVar))
    }

    pub(crate) fn type_of(
        &mut self,
        arena: &mut TypeArena<'a>,
        decl: DeclId<'a>,
    ) -> Option<TypeId> {
        self.operand_of(arena, decl).map(|op| op.ty)
    }

    /// Evaluate a constant expression from a [`TypeSyntax`] (such as a `[N]T`
    /// array length) using this resolver's cache and cycle detection stack.
    pub(in crate::analysis::engine) fn eval_const_expr(
        &mut self,
        arena: &mut TypeArena<'a>,
        origin: &Path,
        source: &str,
        parsed: &TypeSyntax,
        expr_id: ExprId,
    ) -> Option<Operand> {
        let expr = parsed.expression(expr_id)?;
        let expr_text = source.get(expr.span.clone())?.trim();
        if expr_text.is_empty() || expr_text.len() > 1024 {
            return None;
        }
        let file = arena.workspace().files.get_key_value(origin)?.0.as_path();
        let mut depth_guard = arena.enter_depth(DepthKind::Constant)?;
        match &expr.kind {
            ExprKind::Name => {
                let (sym_file, symbol) =
                    depth_guard.resolver().unqualified_symbol(file, expr_text)?;
                if !symbol.is_const() {
                    return None;
                }
                return self
                    .operand_of(
                        &mut depth_guard,
                        DeclId {
                            file: sym_file,
                            symbol,
                        },
                    )
                    .filter(|op| op.mode == Mode::Constant);
            }
            ExprKind::Selector { operand, field } => {
                if let Some(qual_expr) = parsed.expression(*operand)
                    && matches!(qual_expr.kind, ExprKind::Name)
                    && let Some(qual) = source.get(qual_expr.span.clone())
                    && let Some(name) = source.get(field.clone())
                {
                    if depth_guard
                        .resolver()
                        .unqualified_symbol(file, qual)
                        .is_some()
                    {
                        return None;
                    }
                    let mut candidates = depth_guard
                        .resolver()
                        .imported_symbol_iter(file, qual)
                        .filter(|(_, symbol)| symbol.name == name);
                    let (sym_file, symbol) = candidates.next()?;
                    if candidates.next().is_some() || !symbol.is_const() {
                        return None;
                    }
                    return self
                        .operand_of(
                            &mut depth_guard,
                            DeclId {
                                file: sym_file,
                                symbol,
                            },
                        )
                        .filter(|op| op.mode == Mode::Constant);
                }
            }
            _ => {}
        }
        let tree = crate::syntax::parse_expression(expr_text);
        if !tree.diagnostics().is_empty() {
            return None;
        }
        let root = tree.root();
        let block = BlockTree {
            root: StmtId(0),
            statements: Vec::new(),
            expressions: tree.nodes,
            diagnostics: Vec::new(),
            type_references: Vec::new(),
            type_parameters: Vec::new(),
            delimiters: Vec::new(),
        };
        let end = u32::try_from(expr_text.len()).unwrap_or(u32::MAX);
        depth_guard.with_projection(false, |arena| {
            let scopes = ScopeTree::empty([0, end]);
            let mut region = RegionInfo::with_mask(None, [0, end], scopes, RecordMask::TYPES);
            let mut checker = RegionChecker::new(
                arena,
                self,
                file,
                expr_text,
                0,
                &block,
                &mut region,
                None,
                None,
            );
            let op = checker.check_expr_outcome(root, None).operand()?;
            if op.mode != Mode::Constant {
                return None;
            }
            if op.ty.is_decl_safe() {
                return Some(op);
            }
            let kind = checker.arena().basic_kind(op.ty)?;
            let ty = checker.arena().basic(kind)?;
            Some(Operand { ty, ..op })
        })
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
                    let ty =
                        arena.basic(crate::analysis::engine::types::BasicKind::UnsafePointer)?;
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

    fn resolve_signature(
        &mut self,
        arena: &mut TypeArena<'a>,
        decl: DeclId<'a>,
    ) -> Option<Operand> {
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
        if !signature.diagnostics.is_empty() {
            return None;
        }
        let mut const_eval = |arena: &mut TypeArena<'a>,
                              origin: &Path,
                              source: &str,
                              parsed: &TypeSyntax,
                              expr_id: ExprId| {
            self.eval_const_expr(arena, origin, source, parsed, expr_id)
        };
        let has_recv_params = signature.receiver.first().is_some_and(|receiver| {
            !crate::analysis::engine::scope::receiver_parameters(&header, &receiver.annotation)
                .is_empty()
        });
        let ty = if has_recv_params {
            arena.resolve_generic_receiver_signature_with_eval(
                decl.file,
                &header,
                Some(declaration.start),
                &signature,
                &mut const_eval,
            )?
        } else if signature.type_parameters.is_empty() {
            arena.signature_with_eval(
                decl.file,
                &header,
                Some(declaration.start),
                &signature,
                &[],
                &mut const_eval,
                0,
            )?
        } else if signature.receiver.is_empty() {
            arena.resolve_generic_signature_with_eval(
                decl.file,
                &header,
                Some(declaration.start),
                &signature,
                &mut const_eval,
            )?
        } else {
            return None;
        };
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
        if constant && group.is_some() {
            return self.resolve_const_group(arena, decl, span, &source, &parsed, declarations);
        }
        self.resolve_single_value_spec(arena, decl, constant, span, &source, &parsed, declarations)
    }

    #[allow(clippy::too_many_arguments)]
    fn resolve_const_group(
        &mut self,
        arena: &mut TypeArena<'a>,
        decl: DeclId<'a>,
        span: Range<usize>,
        source: &str,
        parsed: &BlockTree,
        declarations: &[StmtId],
    ) -> Option<Operand> {
        struct ConstSpec<'s> {
            ordinal: usize,
            names: &'s [Range<usize>],
            annotation: Option<&'s Range<usize>>,
            values: &'s [ExprId],
        }
        let mut specs = Vec::with_capacity(declarations.len());
        let mut previous: Option<(Option<&Range<usize>>, &[ExprId])> = None;
        let mut target_loc = None;
        for (ordinal, &stmt_id) in declarations.iter().enumerate() {
            let StmtKind::Declaration {
                names,
                annotation,
                values,
                ..
            } = &parsed.statement(stmt_id)?.kind
            else {
                return None;
            };
            let (ann, exprs) = if values.is_empty() {
                if annotation.is_some() {
                    break;
                }
                let Some(prev) = previous else {
                    break;
                };
                prev
            } else {
                previous = Some((annotation.as_ref(), values.as_slice()));
                (annotation.as_ref(), values.as_slice())
            };
            if target_loc.is_none()
                && let Some(position) = names.iter().position(|name| {
                    source.get(name.clone()) == Some(decl.symbol.name.as_str())
                        && span.start.checked_add(name.start) == Some(decl.symbol.start)
                })
            {
                target_loc = Some((specs.len(), position));
            }
            specs.push(ConstSpec {
                ordinal,
                names: names.as_slice(),
                annotation: ann,
                values: exprs,
            });
        }
        let (target_spec_idx, target_pos) = target_loc?;
        let target_spec = &specs[target_spec_idx];
        let target_op = self.eval_const_spec_item(
            arena,
            decl,
            span.start,
            source,
            parsed,
            target_spec.ordinal,
            target_spec.names.len(),
            target_spec.annotation,
            target_spec.values,
            target_pos,
        )?;
        if !target_op.ty.is_decl_safe() {
            return None;
        }
        // Mark the requested constant ready immediately before sweeping siblings
        // so forward/backward references within the group resolve to it in O(1).
        self.slots.insert(decl, DeclEntry::Ready(target_op));

        // Opportunistically seed evaluated sibling constants from the same group
        // without re-reading disk or re-parsing `const (...)`.
        if specs.len() > 1 || target_spec.names.len() > 1 {
            let Some(file_stub) = arena.workspace().files.get(decl.file) else {
                return Some(target_op);
            };
            let mut group_symbols: HashMap<usize, &'a crate::index::IndexedSymbol> = HashMap::new();
            for sym in &file_stub.symbols {
                if sym.is_const() && span.start <= sym.start && sym.start < span.end {
                    group_symbols.insert(sym.start, sym);
                }
            }
            let mut seeded = 0usize;
            'outer: for spec in &specs {
                if spec.names.len() > 256 || spec.values.len() != spec.names.len() {
                    continue;
                }
                for (pos, name_range) in spec.names.iter().enumerate() {
                    if seeded >= MAX_CONST_GROUP_SEED
                        || self.slots.len() >= MAX_DECL_CACHE
                        || self.obj_path.len() >= MAX_DEPTH
                    {
                        break 'outer;
                    }
                    let Some(abs_start) = span.start.checked_add(name_range.start) else {
                        continue;
                    };
                    let Some(&sym) = group_symbols.get(&abs_start) else {
                        continue;
                    };
                    if source.get(name_range.clone()) != Some(sym.name.as_str()) {
                        continue;
                    }
                    let sib_decl = DeclId {
                        file: decl.file,
                        symbol: sym,
                    };
                    if sib_decl == decl || self.slots.contains_key(&sib_decl) {
                        continue;
                    }
                    self.slots.insert(sib_decl, DeclEntry::Checking);
                    self.obj_path.push(sib_decl);
                    let sib_op = self.eval_const_spec_item(
                        arena,
                        sib_decl,
                        span.start,
                        source,
                        parsed,
                        spec.ordinal,
                        spec.names.len(),
                        spec.annotation,
                        spec.values,
                        pos,
                    );
                    self.obj_path.pop();
                    if let Some(op) = sib_op
                        && op.mode == Mode::Constant
                        && op.ty.is_decl_safe()
                    {
                        self.slots.insert(sib_decl, DeclEntry::Ready(op));
                        seeded += 1;
                    } else {
                        // Do not poison siblings with `Failed` during batch sweep.
                        self.slots.remove(&sib_decl);
                    }
                }
            }
        }
        Some(target_op)
    }

    #[allow(clippy::too_many_arguments)]
    fn eval_const_spec_item(
        &mut self,
        arena: &mut TypeArena<'a>,
        decl: DeclId<'a>,
        source_base: usize,
        source: &str,
        parsed: &BlockTree,
        ordinal: usize,
        name_count: usize,
        annotation: Option<&Range<usize>>,
        values: &[ExprId],
        position: usize,
    ) -> Option<Operand> {
        if name_count > 256 || values.len() != name_count {
            return None;
        }
        let end = u32::try_from(source.len()).unwrap_or(u32::MAX);
        let scopes = ScopeTree::empty([0, end]);
        let mut region = RegionInfo::with_mask(Some(decl), [0, end], scopes, RecordMask::TYPES);
        let mut checker = RegionChecker::new(
            arena,
            self,
            decl.file,
            source,
            0,
            parsed,
            &mut region,
            u32::try_from(ordinal).ok(),
            None,
        );
        checker.set_source_base(source_base);
        let target = match annotation {
            Some(ann) => {
                let ty = checker.resolve_type_syntax_span(ann)?;
                checker.arena().constructor_map_keys(ty)?;
                Some(ty)
            }
            None => None,
        };
        let input = checker
            .check_expr_outcome(values[position], target)
            .operand()?;
        if input.mode != Mode::Constant {
            return None;
        }
        let int_bits = checker.arena().integer_bits()?;
        match target {
            Some(target) => checker.arena().assign_basic(input, target, int_bits),
            None => Some(input),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn resolve_single_value_spec(
        &mut self,
        arena: &mut TypeArena<'a>,
        decl: DeclId<'a>,
        constant: bool,
        span: Range<usize>,
        source: &str,
        parsed: &BlockTree,
        declarations: &[StmtId],
    ) -> Option<Operand> {
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
            let end = u32::try_from(source.len()).unwrap_or(u32::MAX);
            let scopes = if constant {
                ScopeTree::empty([0, end])
            } else {
                ScopeTree::from_function(source, 0, None, 0, parsed, None)
            };
            let mask = if constant {
                RecordMask::TYPES
            } else {
                RecordMask::TYPES | RecordMask::DIAGNOSTICS
            };
            let mut region = RegionInfo::with_mask(Some(decl), [0, end], scopes, mask);
            let mut checker = RegionChecker::new(
                arena,
                self,
                decl.file,
                source,
                0,
                parsed,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::engine::types::{BasicKind, Constant};
    use crate::workspace::{self, Workspace};
    use serde_json::json;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TempDir(PathBuf);
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn unique_temp_dir(tag: &str) -> TempDir {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("goquery-{tag}-{}-{stamp}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        TempDir(dir)
    }

    fn init_temp_workspace(root: &Path) -> Workspace {
        let mut workspace = Workspace::default();
        workspace
            .initialize(&json!({
                "rootUri": workspace::uri(root),
                "initializationOptions": { "goroot": "", "fileWatch": false },
            }))
            .expect("initialize workspace");
        workspace
    }

    #[test]
    fn const_group_batch_caches_iota_siblings_without_rereading_disk() {
        let dir = unique_temp_dir("const-group-batch");
        std::fs::write(dir.0.join("go.mod"), "module example.com/m\n\ngo 1.27\n")
            .expect("write go.mod");
        let file = dir.0.join("consts.go");
        let src = concat!(
            "package p\n",
            "const (\n",
            "    First = iota + 10\n",
            "    Second\n",
            "    Third\n",
            "    Fourth = Third * 2\n",
            ")\n",
        );
        std::fs::write(&file, src).expect("write consts.go");

        let workspace = init_temp_workspace(&dir.0);
        let (interned, stub) = workspace
            .files
            .iter()
            .find(|(p, _)| p.ends_with("consts.go"))
            .expect("indexed consts.go");
        let find = |name: &str| DeclId {
            file: interned.as_path(),
            symbol: stub
                .symbols
                .iter()
                .find(|s| s.name == name)
                .expect("const symbol"),
        };
        let first = find("First");
        let second = find("Second");
        let third = find("Third");
        let fourth = find("Fourth");

        let mut store = TypeArena::with_shared_budget(&workspace, workspace.build_view(None));
        let mut decls = DeclResolver::new();

        // Resolving `Third` parses the `const (...)` group once and seeds its siblings.
        let third_op = decls.operand_of(&mut store, third).expect("resolve Third");
        assert!(matches!(third_op.value, Some(Constant::Int(12))));

        // Remove the closed file from disk: all sibling lookups must hit `DeclResolver.slots`.
        std::fs::remove_file(interned).expect("remove temp file");

        let first_op = decls.operand_of(&mut store, first).expect("cached First");
        let second_op = decls.operand_of(&mut store, second).expect("cached Second");
        let fourth_op = decls.operand_of(&mut store, fourth).expect("cached Fourth");
        assert!(matches!(first_op.value, Some(Constant::Int(10))));
        assert!(matches!(second_op.value, Some(Constant::Int(11))));
        assert!(matches!(fourth_op.value, Some(Constant::Int(24))));
    }

    #[test]
    fn decl_resolver_caches_generic_type_origin_and_generic_receiver_method() {
        let dir = unique_temp_dir("generic-decls");
        std::fs::write(dir.0.join("go.mod"), "module example.com/m\n\ngo 1.27\n")
            .expect("write go.mod");
        let src = concat!(
            "package p\n",
            "const N = 4\n",
            "type Box[T comparable] struct { Data [N]T }\n",
            "func (b *Box[T]) First(x T, extra ...int) T { return x }\n",
        );
        std::fs::write(dir.0.join("generic.go"), src).expect("write generic.go");

        let workspace = init_temp_workspace(&dir.0);
        let (interned, stub) = workspace
            .files
            .iter()
            .find(|(p, _)| p.ends_with("generic.go"))
            .expect("indexed generic.go");
        let box_decl = DeclId {
            file: interned.as_path(),
            symbol: stub
                .symbols
                .iter()
                .find(|s| s.name == "Box")
                .expect("Box symbol"),
        };
        let method_decl = DeclId {
            file: interned.as_path(),
            symbol: stub
                .symbols
                .iter()
                .find(|s| s.name == "First")
                .expect("First method"),
        };

        let mut store = TypeArena::with_shared_budget(&workspace, workspace.build_view(None));
        let mut decls = DeclResolver::new();

        // Uninstantiated generic types continue to return `None` from `operand_of`/`type_of`.
        assert!(decls.operand_of(&mut store, box_decl).is_none());
        assert!(decls.type_of(&mut store, box_decl).is_none());

        // `generic_type_origin` caches the `DeclArena` origin template cleanly.
        let origin_ty = decls
            .generic_type_origin(&mut store, box_decl)
            .expect("generic origin");
        assert!(origin_ty.is_decl_safe());
        assert_eq!(
            decls.generic_type_origin(&mut store, box_decl),
            Some(origin_ty)
        );

        // Generic-receiver method signatures resolve and cache in `DeclResolver`.
        let sig_ty = decls
            .type_of(&mut store, method_decl)
            .expect("generic receiver method signature");
        assert!(sig_ty.is_decl_safe());
        let (params, results, variadic) = store.signature_parts(sig_ty).expect("signature parts");
        assert_eq!(params.len(), 2);
        assert_eq!(results.len(), 1);
        assert!(variadic);
        assert_eq!(store.format_parameter(sig_ty), Some(0));

        // Instantiating `Box[int]` evaluates `[N]int` through `DeclResolver` without legacy `Evaluator`.
        let int_ty = store.basic(BasicKind::Int).expect("int");
        let box_int = store
            .push_named(box_decl, &[int_ty], false)
            .expect("Box[int]");
        let data_sel = store
            .literal_field(box_int, interned.as_path(), "Data")
            .expect("Data field");
        assert_eq!(store.array_length(data_sel.ty), Some(4));
    }
}
