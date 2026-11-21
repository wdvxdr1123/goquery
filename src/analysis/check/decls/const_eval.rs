//! Package-level constant-expression evaluation (e.g. `[N]T` array lengths)
//! backed by [`DeclResolver`] and [`RegionChecker`].

use std::path::Path;

use crate::analysis::engine::check::RegionChecker;
use crate::analysis::engine::check::decls::DeclResolver;
use crate::analysis::engine::check::region::{RecordMask, RegionInfo};
use crate::analysis::engine::obj::DeclId;
use crate::analysis::engine::scope::tree::ScopeTree;
use crate::analysis::engine::types::{DepthKind, Mode, Operand, TypeArena};
use crate::syntax::{BlockTree, ExprId, ExprKind, StmtId, TypeSyntax};

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
}
