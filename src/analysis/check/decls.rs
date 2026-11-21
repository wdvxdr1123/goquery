//! Tier 1 demand-driven declaration resolution with cycle detection (`obj_path`).

use crate::analysis::engine::budget::{MAX_BYTES, MAX_DECL_CACHE, MAX_DEPTH};
use crate::analysis::engine::check::region::RegionInfo;
use crate::analysis::engine::check::{ExprOutcome, RegionChecker};
use crate::analysis::engine::obj::DeclId;
use crate::analysis::engine::scope::tree::ScopeTree;
use crate::analysis::engine::types::{Mode, Operand, TypeArena, TypeId};
use crate::syntax::{DeclSpecKind, ExprId, StmtKind};
use std::collections::HashMap;
use std::ops::Range;

#[derive(Clone, Copy)]
enum DeclSlot {
    Checking,
    Ready(Operand),
    InvalidVar,
    Failed,
}

/// Request-scoped cache of cross-file / cross-package declaration operands.
/// Only declaration header slices are read on demand; bodies are never parsed here.
pub(crate) struct DeclTable<'a> {
    slots: HashMap<DeclId<'a>, DeclSlot>,
    obj_path: Vec<DeclId<'a>>,
}

impl<'a> DeclTable<'a> {
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
            self.slots.insert(decl, DeclSlot::InvalidVar);
        } else if operand.mode.is_value() {
            self.slots.insert(decl, DeclSlot::Ready(operand));
        }
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
                DeclSlot::Ready(operand) => Some(operand),
                DeclSlot::Checking | DeclSlot::InvalidVar | DeclSlot::Failed => None,
            };
        }
        if self.slots.len() >= MAX_DECL_CACHE || self.obj_path.len() >= MAX_DEPTH {
            return None;
        }
        self.slots.insert(decl, DeclSlot::Checking);
        self.obj_path.push(decl);
        let resolved = arena.with_decl_alloc(|arena| self.resolve_uncached(arena, decl));
        self.obj_path.pop();
        match resolved {
            Some(operand) if operand.mode == Mode::Invalid => {
                self.slots.insert(decl, DeclSlot::InvalidVar);
                None
            }
            Some(operand) if operand.ty.is_decl_safe() => {
                self.slots.insert(decl, DeclSlot::Ready(operand));
                Some(operand)
            }
            Some(_) | None => {
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
        if !signature.diagnostics.is_empty() {
            return None;
        }
        if let Some(receiver) = signature.receiver.first()
            && !crate::analysis::engine::scope::receiver_parameters(&header, &receiver.annotation)
                .is_empty()
        {
            return None;
        }
        let ty = if signature.type_parameters.is_empty() {
            arena.signature(
                decl.file,
                &header,
                Some(declaration.start),
                &signature,
                &[],
                0,
            )?
        } else if signature.receiver.is_empty() {
            arena.resolve_generic_signature(
                decl.file,
                &header,
                Some(declaration.start),
                &signature,
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
