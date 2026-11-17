//! Bounded declaration-header constant evaluation, entirely inside the type core.
//! No checker callback, synthetic declaration text, or second arena is needed.
use super::{BasicKind, Constant, MAX_BYTES, MAX_DEPTH, Mode, Operand, TypeArena};
use crate::analysis::obj::DeclId;
use crate::syntax::{self, DeclSpecKind, Expr, ExprId, ExprKind, StmtKind};
use std::path::Path;

struct Evaluator<'a, 'arena> {
    arena: &'arena mut TypeArena<'a>,
    path: Vec<DeclId<'a>>,
    memo: Vec<(DeclId<'a>, Option<Operand>)>,
    bytes: usize,
}

impl TypeArena<'_> {
    pub(crate) fn constant_expression(&mut self, origin: &Path, source: &str) -> Option<Operand> {
        if source.len() > MAX_BYTES || self.constant_depth >= MAX_DEPTH {
            return None;
        }
        let tree = syntax::parse_expression(source);
        if !tree.diagnostics().is_empty() {
            return None;
        }
        self.constant_depth += 1;
        let result = Evaluator {
            arena: self,
            path: Vec::new(),
            memo: Vec::new(),
            bytes: source.len(),
        }
        .expression(origin, source, tree.nodes(), tree.root(), None, 0);
        self.constant_depth -= 1;
        result.filter(|op| op.mode == Mode::Constant)
    }
}

impl<'a> Evaluator<'a, '_> {
    fn expression(
        &mut self,
        origin: &Path,
        source: &str,
        nodes: &[Expr],
        id: ExprId,
        iota: Option<u32>,
        depth: usize,
    ) -> Option<Operand> {
        self.arena.step()?;
        if depth >= MAX_DEPTH {
            return None;
        }
        let expr = nodes.get(id.index())?;
        let bits = self.arena.integer_bits()?;
        match &expr.kind {
            ExprKind::Literal => self.arena.literal(source.get(expr.span.clone())?),
            ExprKind::Name => {
                let name = source.get(expr.span.clone())?;
                if let Some((file, symbol)) = self.arena.resolver().unqualified_symbol(origin, name)
                {
                    return self.declaration(DeclId { file, symbol }, depth + 1);
                }
                if self
                    .arena
                    .resolver()
                    .file_symbol_iter(origin)
                    .any(|(_, symbol)| symbol.name == name)
                {
                    return None;
                }
                match name {
                    "iota" => Some(Operand {
                        mode: Mode::Constant,
                        ty: self.arena.basic(BasicKind::UntypedInt)?,
                        value: Some(Constant::Int(i128::from(iota?))),
                    }),
                    "true" | "false" => Some(Operand {
                        mode: Mode::Constant,
                        ty: self.arena.basic(BasicKind::UntypedBool)?,
                        value: Some(Constant::Boolean),
                    }),
                    _ => None,
                }
            }
            ExprKind::Selector { operand, field } => {
                let qualifier = nodes.get(operand.index())?;
                if !matches!(qualifier.kind, ExprKind::Name) {
                    return None;
                }
                let qualifier = source.get(qualifier.span.clone())?;
                let name = source.get(field.clone())?;
                // A package-level binding shadows an import qualifier.
                if self
                    .arena
                    .resolver()
                    .unqualified_symbol(origin, qualifier)
                    .is_some()
                {
                    return None;
                }
                let mut candidates = self
                    .arena
                    .resolver()
                    .imported_symbol_iter(origin, qualifier)
                    .filter(|(_, symbol)| symbol.name == name);
                let (file, symbol) = candidates.next()?;
                if candidates.next().is_some() {
                    return None;
                }
                self.declaration(DeclId { file, symbol }, depth + 1)
            }
            ExprKind::Parenthesized { expression } => {
                self.expression(origin, source, nodes, *expression, iota, depth + 1)
            }
            ExprKind::Unary { operator, operand } => {
                let op = self.expression(origin, source, nodes, *operand, iota, depth + 1)?;
                if op.mode != Mode::Constant {
                    return None;
                }
                self.arena.unary(source.get(operator.clone())?, op, bits)
            }
            ExprKind::Binary {
                left,
                operator,
                right,
            } => {
                let left = self.expression(origin, source, nodes, *left, iota, depth + 1)?;
                let right = self.expression(origin, source, nodes, *right, iota, depth + 1)?;
                if left.mode != Mode::Constant || right.mode != Mode::Constant {
                    return None;
                }
                self.arena
                    .binary_expr(source.get(operator.clone())?, left, right, None, bits)
            }
            ExprKind::Call {
                function,
                arguments,
                variadic: false,
            } => {
                if arguments.len() > 256 {
                    return None;
                }
                let function = nodes.get(function.index())?;
                let name = source.get(function.span.clone())?;
                let mut args = Vec::with_capacity(arguments.len());
                for &argument in arguments {
                    args.push(self.expression(origin, source, nodes, argument, iota, depth + 1)?);
                }
                let builtin = matches!(function.kind, ExprKind::Name)
                    && !self
                        .arena
                        .resolver()
                        .file_symbol_iter(origin)
                        .any(|(_, symbol)| symbol.name == name);
                if builtin {
                    match (name, args.as_slice()) {
                        ("len" | "cap", [arg]) => {
                            return self.arena.builtin_length(*arg, name == "cap", false, bits);
                        }
                        ("min" | "max", args) => {
                            return self.arena.builtin_minmax(args, name == "max", bits);
                        }
                        ("complex", args) => return self.arena.builtin_complex(args, bits),
                        ("real" | "imag", [arg]) => {
                            return self.arena.builtin_component(*arg, name == "imag");
                        }
                        _ => {}
                    }
                }
                let [arg] = args.as_slice() else {
                    return None;
                };
                let target = self.arena.parse(origin, name)?;
                self.arena.convert(*arg, target, bits)
            }
            _ => None,
        }
    }

    fn declaration(&mut self, decl: DeclId<'a>, depth: usize) -> Option<Operand> {
        self.arena.step()?;
        if let Some((_, result)) = self.memo.iter().find(|(cached, _)| *cached == decl) {
            return *result;
        }
        if depth >= MAX_DEPTH || self.path.contains(&decl) || !decl.symbol.is_value() {
            return None;
        }
        self.path.push(decl);
        let result = self.declaration_inner(decl, depth);
        self.path.pop();
        if self.memo.len()
            < (16 * 1024 / std::mem::size_of::<(DeclId<'a>, Option<Operand>)>()).min(128)
        {
            self.memo.push((decl, result));
        }
        result
    }

    fn declaration_inner(&mut self, decl: DeclId<'a>, depth: usize) -> Option<Operand> {
        let constant = decl.symbol.is_const();
        let workspace = self.arena.workspace;
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
        self.bytes = self.bytes.checked_add(span.len())?;
        if self.bytes > MAX_BYTES * 4 {
            return None;
        }
        let source = workspace.source_slice(decl.file, span.clone())?;
        let tree = if group.is_some() {
            syntax::parse_declaration(&source)
        } else {
            syntax::parse_decl_spec(
                &source,
                if constant {
                    DeclSpecKind::Const
                } else {
                    DeclSpecKind::Var
                },
            )
        };
        if !tree.diagnostics().is_empty() {
            return None;
        }
        let declarations = match &tree.statement(tree.root())?.kind {
            StmtKind::DeclarationGroup { declarations } => declarations.as_slice(),
            StmtKind::Declaration { .. } => std::slice::from_ref(&tree.root),
            _ => return None,
        };
        let mut previous = None;
        for (ordinal, &id) in declarations.iter().enumerate() {
            let StmtKind::Declaration {
                names,
                annotation,
                values,
                ..
            } = &tree.statement(id)?.kind
            else {
                return None;
            };
            let (ann, exprs) = if constant && values.is_empty() {
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
            let Some(index) = names
                .iter()
                .position(|name| span.start + name.start == decl.symbol.start)
            else {
                continue;
            };
            let target = match ann {
                Some(ann) => Some(self.arena.parse(decl.file, source.get(ann.clone())?)?),
                None => None,
            };
            if !constant && let Some(ty) = target {
                self.arena.validate_type(ty)?;
                return Some(Operand {
                    mode: Mode::Variable,
                    ty,
                    value: None,
                });
            }
            if names.len() != exprs.len() {
                return None;
            }
            let op = self.expression(
                decl.file,
                &source,
                tree.expressions(),
                *exprs.get(index)?,
                constant.then(|| u32::try_from(ordinal).ok()).flatten(),
                depth + 1,
            )?;
            if constant {
                if op.mode != Mode::Constant {
                    return None;
                }
                return match target {
                    Some(target) => self
                        .arena
                        .assign_basic(op, target, self.arena.integer_bits()?),
                    None => Some(op),
                };
            }
            let ty = self.arena.default_operand(op, self.arena.integer_bits()?)?;
            return Some(Operand {
                mode: Mode::Variable,
                ty,
                value: None,
            });
        }
        None
    }
}
