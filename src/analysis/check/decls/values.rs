//! Package-level `const` and `var` declaration resolution, including
//! whole `const (...)` group batch caching for [`DeclResolver`].

use std::collections::HashMap;
use std::ops::Range;

use crate::analysis::engine::budget::{MAX_BYTES, MAX_DECL_CACHE, MAX_DEPTH};
use crate::analysis::engine::check::decls::{DeclEntry, DeclResolver};
use crate::analysis::engine::check::region::{RecordMask, RegionInfo};
use crate::analysis::engine::check::{ExprOutcome, RegionChecker};
use crate::analysis::engine::obj::DeclId;
use crate::analysis::engine::scope::tree::ScopeTree;
use crate::analysis::engine::types::{Mode, Operand, TypeArena};
use crate::syntax::{BlockTree, DeclSpecKind, ExprId, StmtId, StmtKind};

/// Maximum sibling constants opportunistically seeded from a single `const (...)`
/// group lookup so very large constant tables stay bounded in step/cache budget.
const MAX_CONST_GROUP_SEED: usize = 256;

impl<'a> DeclResolver<'a> {
    pub(super) fn resolve_value(
        &mut self,
        arena: &mut TypeArena<'a>,
        decl: DeclId<'a>,
    ) -> Option<Operand> {
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
