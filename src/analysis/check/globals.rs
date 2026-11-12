//! Package values use declaration slices and bounded facts in the caller's arena.
use super::{Inference, ScopedType};
use crate::analysis::scope::Locals;
use crate::analysis::types::{Mode, Operand, TypeArena};
use crate::index::IndexedSymbol as Symbol;
use crate::syntax::{ExprId, ExprKind, StmtKind};
use std::path::{Path, PathBuf};

impl<'workspace> Inference<'workspace, '_> {
    pub(super) fn global_symbol(
        &self,
        id: ExprId,
    ) -> Option<(&'workspace PathBuf, &'workspace Symbol)> {
        let expression = self.unparenthesized(id);
        match &expression.kind {
            ExprKind::Name => {
                if self.workspace.files[self.file]
                    .imports
                    .iter()
                    .any(|import| {
                        self.workspace.import_name(import) == Some(self.text(&expression.span))
                    })
                {
                    return None;
                }
            }
            ExprKind::Selector { operand, .. } => {
                let root = self.unparenthesized(*operand);
                if !matches!(root.kind, ExprKind::Name)
                    || !self.workspace.files[self.file]
                        .imports
                        .iter()
                        .any(|import| {
                            self.workspace.import_name(import) == Some(self.text(&root.span))
                        })
                    || self
                        .locals
                        .resolve_at(self.base + root.span.start, self.text(&root.span))
                        .is_some()
                {
                    return None;
                }
            }
            _ => return None,
        }
        let (file, symbol) = self.callee(id)?;
        symbol.is_value().then_some((file, symbol))
    }

    pub(super) fn global_root(&self, mut id: ExprId) -> bool {
        for _ in 0..32 {
            if self.global_symbol(id).is_some() {
                return true;
            }
            id = match self.unparenthesized(id).kind {
                ExprKind::Selector { operand, .. }
                | ExprKind::Index { operand, .. }
                | ExprKind::Slice { operand, .. }
                | ExprKind::Unary { operand, .. } => operand,
                _ => return false,
            };
        }
        false
    }

    pub(super) fn global_operand(&self, arena: &mut TypeArena<'_>, id: ExprId) -> Option<Operand> {
        if self.depth >= 32 {
            return None;
        }
        let (file, symbol) = self.global_symbol(id)?;
        if let Some(fact) = arena.cached_value(file, symbol.start) {
            return fact;
        }
        let index = arena.start_value(file, symbol.start)?;
        let fact = self.read_global(arena, file, symbol);
        arena.finish_value(index, fact);
        fact
    }

    fn read_global(
        &self,
        arena: &mut TypeArena<'_>,
        file: &Path,
        symbol: &Symbol,
    ) -> Option<Operand> {
        let constant = symbol.is_const();
        let group = constant
            .then(|| {
                self.workspace.files[file]
                    .constant_groups
                    .iter()
                    .find(|span| {
                        span[0] as usize <= symbol.start && symbol.start < span[1] as usize
                    })
            })
            .flatten();
        let span = group
            .map(|span| span[0] as usize..span[1] as usize)
            .or_else(|| symbol.declaration())?;
        let source = self.workspace.source_slice(file, span.clone())?;
        let prefix = if group.is_some() {
            "{"
        } else if constant {
            "{const "
        } else {
            "{var "
        };
        let wrapped = format!("{prefix}{source}\n}}");
        let parsed = crate::syntax::parse_block(&wrapped);
        if !parsed.diagnostics.is_empty() || parsed.statements.len() > 4096 {
            return None;
        }
        let StmtKind::Block { statements } = &parsed.statements[parsed.root.0].kind else {
            return None;
        };
        let first = *statements.first()?;
        let declarations = match &parsed.statements[first.0].kind {
            StmtKind::DeclarationGroup { declarations } => declarations.as_slice(),
            StmtKind::Declaration { .. } if statements.len() == 1 => statements.as_slice(),
            _ => return None,
        };
        let mut previous = None;
        for (ordinal, declaration) in declarations.iter().enumerate() {
            let StmtKind::Declaration {
                names,
                annotation,
                values,
                ..
            } = &parsed.statements[declaration.0].kind
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
                wrapped.get(name.clone()) == Some(symbol.name.as_str())
                    && name
                        .start
                        .checked_sub(prefix.len())
                        .and_then(|start| span.start.checked_add(start))
                        == Some(symbol.start)
            }) else {
                continue;
            };
            if names.len() > 256 {
                return None;
            }
            let (annotation, values) = inherited;
            let locals = Locals::empty();
            let inference = Inference {
                workspace: self.workspace,
                name_index: None,
                scoped_bindings: None,
                file,
                source: &wrapped,
                locals: &locals,
                expressions: &parsed.expressions,
                base: 0,
                depth: self.depth + 1,
                iota: constant.then(|| u32::try_from(ordinal).ok()).flatten(),
            };
            let target = match annotation {
                Some(span) => Some(arena.parse(file, &wrapped[span.clone()])?),
                None => None,
            };
            if let Some(target) = target {
                arena.constructor_map_keys(target)?;
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
                inference.value_operand(arena, values[position])?
            } else if values.len() == 1 {
                inference.tuple_operand(arena, values[0], position, names.len())?
            } else {
                return None;
            };
            let int_bits = self.workspace.integer_bits()?;
            if constant {
                if input.mode != Mode::Constant {
                    return None;
                }
                return match target {
                    Some(target) => arena.assign_basic(input, target, int_bits),
                    None => Some(input),
                };
            }
            let ty = if let Some(target) = target {
                arena.assign_value(input, target, int_bits)?.then_some(())?;
                target
            } else {
                let ty = arena.default_operand(input, int_bits)?;
                arena.constructor_map_keys(ty)?;
                ty
            };
            return Some(Operand {
                mode: Mode::Variable,
                ty,
                value: None,
            });
        }
        None
    }

    pub(super) fn global_field(&self, arena: &mut TypeArena<'_>, id: ExprId) -> Option<Operand> {
        let ExprKind::Selector { operand, field } = &self.unparenthesized(id).kind else {
            return None;
        };
        if !self.global_root(*operand) {
            return None;
        }
        let input = self.next().value_operand(arena, *operand)?;
        arena.field_operand(input, self.file, self.text(field))
    }

    pub(super) fn native_call(
        &self,
        arena: &mut TypeArena<'_>,
        function: ExprId,
        arguments: &[ExprId],
        spread: bool,
        result: usize,
        count: usize,
    ) -> Option<Option<Operand>> {
        let callable = if let Some(method) = self.selector_operand(arena, function) {
            let Some(method) = method else {
                return Some(None);
            };
            method
        } else if self.global_symbol(function).is_some() {
            let Some(callable) = self.global_operand(arena, function) else {
                return Some(None);
            };
            callable
        } else if self.global_root(function) {
            let ExprKind::Selector { operand, field } = &self.unparenthesized(function).kind else {
                return None;
            };
            let Some(input) = self.next().value_operand(arena, *operand) else {
                return Some(None);
            };
            if let Some(field) = arena.field_operand(input, self.file, self.text(field)) {
                field
            } else {
                self.function_operand(arena, function)?
            }
        } else {
            self.function_operand(arena, function)?
        };
        Some((|| {
            if arena.is_callable(callable.ty) != Some(true) {
                return None;
            }
            let actuals = self.next().call_operands(arena, arguments, spread)?;
            let ty = arena.call_signature(
                callable.ty,
                &actuals,
                spread,
                result,
                count,
                self.workspace.integer_bits()?,
            )?;
            Some(Operand {
                mode: Mode::Value,
                ty,
                value: None,
            })
        })())
    }

    pub(super) fn scoped_global(&self, id: ExprId) -> Option<ScopedType<'workspace>> {
        let (file, _) = self.global_symbol(id)?;
        let mut arena = TypeArena::new(self.workspace);
        let input = self.global_operand(&mut arena, id)?;
        let ty = arena.default_operand(input, self.workspace.integer_bits()?)?;
        self.scoped_native_type(
            &mut arena,
            ty,
            file,
            self.base + self.expressions[id.0].span.start,
        )
    }
}
