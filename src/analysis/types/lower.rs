//! Single-pass AST-driven type lowering from [`crate::syntax::TypeSyntax`] to compacted [`Ty`] nodes.
use super::{
    BasicKind, CheckOutcome, Direction, MAX_BYTES, MAX_DEPTH, Mode, Operand, Ty, TypeArena, TypeId,
    UnknownReason,
};
use crate::analysis::engine::obj::DeclId;
use crate::analysis::engine::resolve::TypeNameLookup;
use crate::syntax::{
    ChanDir, ExprId, ExprKind, Lexer, TokenKind, TypeNodeId, TypeSyntax, TypeTreeKind,
};
use std::path::Path;

/// Callback for evaluating non-literal `[N]T` array-length expressions during
/// single-pass [`TypeSyntax`] lowering, backed by `DeclResolver` / `RegionChecker`.
pub(crate) type ConstExprEval<'e, 'a> =
    &'e mut dyn FnMut(&mut TypeArena<'a>, &Path, &str, &TypeSyntax, ExprId) -> Option<Operand>;

pub(super) fn default_const_eval<'a>(
    arena: &mut TypeArena<'a>,
    origin: &Path,
    source: &str,
    parsed: &TypeSyntax,
    expr_id: ExprId,
) -> Option<Operand> {
    arena.step()?;
    crate::analysis::engine::check::decls::eval_package_const_expr(
        arena, origin, source, parsed, expr_id,
    )
}

impl<'a> TypeArena<'a> {
    fn resolve_type_name(&mut self, origin: &Path, name: &str) -> TypeNameLookup<'a> {
        self.env_cache
            .resolve_type_name(self.workspace, origin, name)
    }

    pub(crate) fn parse_checked(&mut self, origin: &Path, source: &str) -> CheckOutcome<TypeId> {
        self.parse_in_checked(origin, source, None, &[])
    }

    pub(crate) fn parse(&mut self, origin: &Path, source: &str) -> Option<TypeId> {
        self.parse_in(origin, source, &[])
    }

    pub(crate) fn parse_in(
        &mut self,
        origin: &Path,
        source: &str,
        environment: &[(&str, TypeId)],
    ) -> Option<TypeId> {
        self.parse_in_internal(origin, source, None, environment)
    }

    pub(crate) fn parse_in_at(
        &mut self,
        origin: &Path,
        source: &str,
        file_offset: usize,
        environment: &[(&str, TypeId)],
    ) -> Option<TypeId> {
        self.parse_in_internal(origin, source, Some(file_offset), environment)
    }

    pub(crate) fn builtin_type(&mut self, origin: &Path, name: &str) -> Option<TypeId> {
        match name {
            "any" => self.builtin_interface(origin, false),
            "error" => self.builtin_interface(origin, true),
            _ => BasicKind::named(name).and_then(|kind| self.basic(kind)),
        }
    }

    pub(super) fn parse_in_internal(
        &mut self,
        origin: &Path,
        source: &str,
        file_offset: Option<usize>,
        environment: &[(&str, TypeId)],
    ) -> Option<TypeId> {
        self.parse_in_checked(origin, source, file_offset, environment)
            .ok()
    }

    pub(super) fn parse_in_checked(
        &mut self,
        origin: &Path,
        source: &str,
        file_offset: Option<usize>,
        environment: &[(&str, TypeId)],
    ) -> CheckOutcome<TypeId> {
        let mut eval = default_const_eval;
        self.parse_in_checked_with_eval(origin, source, file_offset, environment, &mut eval)
    }

    pub(super) fn parse_in_checked_with_eval(
        &mut self,
        origin: &Path,
        source: &str,
        file_offset: Option<usize>,
        environment: &[(&str, TypeId)],
        const_eval: ConstExprEval<'_, 'a>,
    ) -> CheckOutcome<TypeId> {
        if source.len() > MAX_BYTES {
            return CheckOutcome::Unknown(UnknownReason::BudgetExceeded);
        }
        let parsed = crate::syntax::parse_type(source);
        self.lower_type_syntax_with_eval(
            origin,
            source,
            &parsed,
            file_offset,
            environment,
            const_eval,
        )
    }

    pub(super) fn lower_type_syntax_checked(
        &mut self,
        origin: &Path,
        source: &str,
        parsed: &TypeSyntax,
        file_offset: Option<usize>,
        environment: &[(&str, TypeId)],
    ) -> CheckOutcome<TypeId> {
        let mut eval = default_const_eval;
        self.lower_type_syntax_with_eval(
            origin,
            source,
            parsed,
            file_offset,
            environment,
            &mut eval,
        )
    }

    pub(crate) fn lower_type_syntax_with_eval(
        &mut self,
        origin: &Path,
        source: &str,
        parsed: &TypeSyntax,
        file_offset: Option<usize>,
        environment: &[(&str, TypeId)],
        const_eval: ConstExprEval<'_, 'a>,
    ) -> CheckOutcome<TypeId> {
        if source.len() > MAX_BYTES || parsed.tree_overflowed() {
            return CheckOutcome::Unknown(UnknownReason::BudgetExceeded);
        }
        if !parsed.diagnostics.is_empty() {
            return CheckOutcome::Unknown(UnknownReason::SyntaxError);
        }
        let Some(root) = parsed.root() else {
            return CheckOutcome::Unknown(UnknownReason::BudgetExceeded);
        };
        self.lower_type_tree_node(
            origin,
            source,
            parsed,
            root,
            file_offset,
            environment,
            const_eval,
            0,
        )
    }

    /// Lower one specific node inside an already-parsed [`TypeSyntax`] tree
    /// (e.g. a struct field or signature parameter) without re-parsing source.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn lower_type_node_with_eval(
        &mut self,
        origin: &Path,
        source: &str,
        parsed: &TypeSyntax,
        node_id: TypeNodeId,
        file_offset: Option<usize>,
        environment: &[(&str, TypeId)],
        const_eval: ConstExprEval<'_, 'a>,
    ) -> CheckOutcome<TypeId> {
        if source.len() > MAX_BYTES || parsed.tree_overflowed() {
            return CheckOutcome::Unknown(UnknownReason::BudgetExceeded);
        }
        self.lower_type_tree_node(
            origin,
            source,
            parsed,
            node_id,
            file_offset,
            environment,
            const_eval,
            0,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn lower_type_tree_node(
        &mut self,
        origin: &Path,
        source: &str,
        parsed: &TypeSyntax,
        node_id: TypeNodeId,
        file_offset: Option<usize>,
        environment: &[(&str, TypeId)],
        const_eval: ConstExprEval<'_, 'a>,
        depth: usize,
    ) -> CheckOutcome<TypeId> {
        if depth >= MAX_DEPTH {
            return CheckOutcome::Unknown(UnknownReason::BudgetExceeded);
        }
        let Some(node) = parsed.tree_node(node_id) else {
            return CheckOutcome::Unknown(UnknownReason::SyntaxError);
        };

        let ty = match &node.kind {
            TypeTreeKind::Missing => {
                return CheckOutcome::Unknown(UnknownReason::SyntaxError);
            }
            TypeTreeKind::Parenthesized(inner) => {
                return self.lower_type_tree_node(
                    origin,
                    source,
                    parsed,
                    *inner,
                    file_offset,
                    environment,
                    const_eval,
                    depth + 1,
                );
            }
            TypeTreeKind::Pointer(inner) => match self.lower_type_tree_node(
                origin,
                source,
                parsed,
                *inner,
                file_offset,
                environment,
                const_eval,
                depth + 1,
            ) {
                CheckOutcome::Ok(element) => Ty::Pointer(element),
                other => return other,
            },
            TypeTreeKind::Slice(inner) => match self.lower_type_tree_node(
                origin,
                source,
                parsed,
                *inner,
                file_offset,
                environment,
                const_eval,
                depth + 1,
            ) {
                CheckOutcome::Ok(element) => Ty::Slice(element),
                other => return other,
            },
            TypeTreeKind::Array { len_expr, elem } => {
                let element = match self.lower_type_tree_node(
                    origin,
                    source,
                    parsed,
                    *elem,
                    file_offset,
                    environment,
                    const_eval,
                    depth + 1,
                ) {
                    CheckOutcome::Ok(ty) => ty,
                    other => return other,
                };
                let Some(expr_id) = *len_expr else {
                    return CheckOutcome::Unknown(UnknownReason::Unsupported);
                };
                let Some(length) =
                    self.eval_const_array_length(origin, source, parsed, expr_id, const_eval)
                else {
                    return CheckOutcome::Unknown(UnknownReason::Unsupported);
                };
                let Some(int_bits) = self.integer_bits() else {
                    return CheckOutcome::Unknown(UnknownReason::MissingMetadata);
                };
                let maximum = (1_u64 << (int_bits - 1)) - 1;
                if length > maximum {
                    return CheckOutcome::Invalid(());
                }
                Ty::Array { length, element }
            }
            TypeTreeKind::Map { key, value } => {
                let key = match self.lower_type_tree_node(
                    origin,
                    source,
                    parsed,
                    *key,
                    file_offset,
                    environment,
                    const_eval,
                    depth + 1,
                ) {
                    CheckOutcome::Ok(ty) => ty,
                    other => return other,
                };
                if !self.mode.is_symbolic() && self.comparable(key, depth + 1) == Some(false) {
                    return CheckOutcome::Invalid(());
                }
                let value = match self.lower_type_tree_node(
                    origin,
                    source,
                    parsed,
                    *value,
                    file_offset,
                    environment,
                    const_eval,
                    depth + 1,
                ) {
                    CheckOutcome::Ok(ty) => ty,
                    other => return other,
                };
                Ty::Map { key, value }
            }
            TypeTreeKind::Chan { dir, elem } => {
                let direction = match dir {
                    ChanDir::Send => Direction::Send,
                    ChanDir::Recv => Direction::Receive,
                    ChanDir::Both => Direction::Both,
                };
                let element = match self.lower_type_tree_node(
                    origin,
                    source,
                    parsed,
                    *elem,
                    file_offset,
                    environment,
                    const_eval,
                    depth + 1,
                ) {
                    CheckOutcome::Ok(ty) => ty,
                    other => return other,
                };
                Ty::Chan { direction, element }
            }
            TypeTreeKind::Func(sig_id) => {
                return self.lower_sig_node_checked(
                    origin,
                    source,
                    parsed,
                    *sig_id,
                    file_offset,
                    environment,
                    const_eval,
                    depth + 1,
                );
            }
            TypeTreeKind::Struct { fields } => {
                return self.lower_struct_fields_checked(
                    origin,
                    source,
                    parsed,
                    fields.clone(),
                    file_offset,
                    environment,
                    const_eval,
                    depth + 1,
                );
            }
            TypeTreeKind::Interface { members } => {
                let Some(origin_path) = self
                    .workspace
                    .files
                    .get_key_value(origin)
                    .map(|(p, _)| p.as_path())
                else {
                    return CheckOutcome::Unknown(UnknownReason::MissingMetadata);
                };
                return self.lower_interface_members_checked(
                    origin_path,
                    source,
                    parsed,
                    members.clone(),
                    file_offset,
                    environment,
                    const_eval,
                    depth + 1,
                );
            }
            TypeTreeKind::Name { .. } | TypeTreeKind::Qualified { .. } => {
                return self.lower_named_or_instantiated(
                    origin,
                    source,
                    parsed,
                    node_id,
                    &[],
                    file_offset,
                    environment,
                    const_eval,
                    depth,
                );
            }
            TypeTreeKind::Instantiation { base, args } => {
                let arg_ids = parsed.type_node_list(args.clone());
                if arg_ids.is_empty() {
                    return CheckOutcome::Unknown(UnknownReason::SyntaxError);
                }
                return self.lower_named_or_instantiated(
                    origin,
                    source,
                    parsed,
                    *base,
                    arg_ids,
                    file_offset,
                    environment,
                    const_eval,
                    depth,
                );
            }
            TypeTreeKind::Union { .. } | TypeTreeKind::Approx(_) => {
                return CheckOutcome::Unknown(UnknownReason::SyntaxError);
            }
        };
        match self.push(ty) {
            Some(id) => CheckOutcome::Ok(id),
            None => CheckOutcome::Unknown(UnknownReason::BudgetExceeded),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn lower_named_or_instantiated(
        &mut self,
        origin: &Path,
        source: &str,
        parsed: &TypeSyntax,
        base_id: TypeNodeId,
        arg_ids: &[TypeNodeId],
        file_offset: Option<usize>,
        environment: &[(&str, TypeId)],
        const_eval: ConstExprEval<'_, 'a>,
        depth: usize,
    ) -> CheckOutcome<TypeId> {
        let Some(base_node) = parsed.tree_node(base_id) else {
            return CheckOutcome::Unknown(UnknownReason::SyntaxError);
        };
        let is_unqualified = match base_node.kind {
            TypeTreeKind::Name { .. } => true,
            TypeTreeKind::Qualified { .. } => false,
            _ => return CheckOutcome::Unknown(UnknownReason::SyntaxError),
        };
        let Some(name) = source.get(base_node.byte_range()) else {
            return CheckOutcome::Unknown(UnknownReason::SyntaxError);
        };
        if name == "_" {
            return CheckOutcome::Unknown(UnknownReason::SyntaxError);
        }
        let has_args = !arg_ids.is_empty();
        if is_unqualified
            && let Some((_, ty)) = environment.iter().find(|(param, _)| *param == name)
        {
            if has_args {
                return CheckOutcome::Invalid(());
            }
            return CheckOutcome::Ok(*ty);
        }
        let Some(declaration) = self.resolve_type_name(origin, name) else {
            return CheckOutcome::Unknown(UnknownReason::MissingMetadata);
        };
        if declaration.is_none() && matches!(name, "any" | "error") {
            if has_args {
                return CheckOutcome::Invalid(());
            }
            return match self.builtin_interface(origin, name == "error") {
                Some(ty) => CheckOutcome::Ok(ty),
                None => CheckOutcome::Unknown(UnknownReason::BudgetExceeded),
            };
        }
        if declaration.is_none()
            && let Some(kind) = BasicKind::named(name)
        {
            if has_args {
                return CheckOutcome::Invalid(());
            }
            return match self.push(Ty::Basic(kind)) {
                Some(id) => CheckOutcome::Ok(id),
                None => CheckOutcome::Unknown(UnknownReason::BudgetExceeded),
            };
        }
        let Some(DeclId { file, symbol }) = declaration else {
            return CheckOutcome::Unknown(UnknownReason::MissingMetadata);
        };
        if symbol.name == "Pointer" && self.workspace.import_path(file).as_deref() == Some("unsafe")
        {
            if has_args {
                return CheckOutcome::Invalid(());
            }
            return match self.basic(BasicKind::UnsafePointer) {
                Some(ty) => CheckOutcome::Ok(ty),
                None => CheckOutcome::Unknown(UnknownReason::BudgetExceeded),
            };
        }
        if !self.mode.is_symbolic()
            && matches!(symbol.name.as_str(), "ArbitraryType" | "IntegerType")
            && self.workspace.import_path(file).as_deref() == Some("unsafe")
        {
            return CheckOutcome::Unknown(UnknownReason::Unsupported);
        }
        let mut arguments = Vec::with_capacity(arg_ids.len());
        for &arg_id in arg_ids {
            if arguments.len() >= self.active_node_limit() {
                return CheckOutcome::Unknown(UnknownReason::BudgetExceeded);
            }
            let arg = match self.lower_type_tree_node(
                origin,
                source,
                parsed,
                arg_id,
                file_offset,
                environment,
                const_eval,
                depth + 1,
            ) {
                CheckOutcome::Ok(ty) => ty,
                other => return other,
            };
            arguments.push(arg);
        }
        match self.push_named(DeclId { file, symbol }, &arguments, symbol.alias) {
            Some(ty) => CheckOutcome::Ok(ty),
            None => CheckOutcome::Unknown(UnknownReason::BudgetExceeded),
        }
    }

    fn eval_const_array_length(
        &mut self,
        origin: &Path,
        source: &str,
        parsed: &TypeSyntax,
        expr_id: ExprId,
        const_eval: ConstExprEval<'_, 'a>,
    ) -> Option<u64> {
        let expr = parsed.expression(expr_id)?;
        let expr_text = source.get(expr.span.clone())?.trim();
        if expr_text.is_empty() || expr_text.len() > 1024 {
            return None;
        }
        if matches!(expr.kind, ExprKind::Literal(TokenKind::Int))
            && let Some(length) = parse_symbolic_integer(expr_text)
        {
            return Some(length);
        }
        let int_bits = self.integer_bits()?;
        let op = const_eval(self, origin, source, parsed, expr_id)?;
        if op.mode != Mode::Constant {
            return None;
        }
        let kind = self.basic_kind(op.ty)?;
        if !kind.untyped() && !kind.integer() {
            return None;
        }
        super::constant::representable(op.value?, BasicKind::Int, int_bits).then_some(())?;
        let value = match op.value? {
            super::Constant::Int(value)
            | super::Constant::Float {
                integral: Some(value),
                ..
            } => value,
            _ => return None,
        };
        u64::try_from(value).ok()
    }
}

fn parse_symbolic_integer(text: &str) -> Option<u64> {
    if Lexer::new(text)
        .filter(|token| token.kind != TokenKind::Semicolon)
        .count()
        != 1
    {
        return None;
    }
    let digits = text.replace('_', "");
    let (radix, body) = if digits.starts_with("0x") || digits.starts_with("0X") {
        (16, &digits[2..])
    } else if digits.starts_with("0b") || digits.starts_with("0B") {
        (2, &digits[2..])
    } else if digits.starts_with("0o") || digits.starts_with("0O") {
        (8, &digits[2..])
    } else if digits.starts_with('0') && digits.len() > 1 {
        (8, &digits[1..])
    } else {
        (10, digits.as_str())
    };
    u64::from_str_radix(body, radix).ok()
}

pub(crate) fn declaration_body(declaration: &str) -> Option<&str> {
    let mut tokens = Lexer::new(declaration).peekable();
    if let Some(end) = crate::syntax::type_parameter_end(declaration) {
        while tokens.peek().is_some_and(|token| token.span.start < end) {
            tokens.next();
        }
    }
    if tokens.peek()?.kind == TokenKind::Assign {
        tokens.next();
    }
    let start = tokens.next()?.span.start;
    Some(&declaration[start..])
}

/// Return the body span of one outer parenthesized type, or `None` inside the
/// outer option when the type has no outer parentheses.
pub(crate) fn type_parentheses(source: &str) -> Option<Option<std::ops::Range<usize>>> {
    let first = Lexer::new(source).next()?;
    if first.kind != TokenKind::LParen {
        return Some(None);
    }
    let last = Lexer::new(source)
        .filter(|token| token.kind != TokenKind::Semicolon)
        .last()?;
    if last.kind != TokenKind::RParen {
        return None;
    }
    Some(Some(first.span.end..last.span.start))
}

/// Remove at most `MAX_DEPTH` outer pairs; a deeper remainder stays visible to
/// the caller instead of being silently accepted or rejected by this helper.
pub(crate) fn unparenthesized_type(mut source: &str) -> Option<&str> {
    for _ in 0..MAX_DEPTH {
        let Some(span) = type_parentheses(source)? else {
            return Some(source);
        };
        source = source.get(span)?;
    }
    Some(source)
}

#[cfg(test)]
mod tests {
    use super::*;
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

    #[test]
    fn single_pass_lowering_handles_nested_struct_interface_func_and_const_arrays() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = TempDir(std::env::temp_dir().join(format!(
            "goquery-lower-single-pass-{}-{stamp}",
            std::process::id()
        )));
        std::fs::create_dir_all(&root.0).expect("create temp dir");
        std::fs::write(root.0.join("go.mod"), "module example.com/m\n\ngo 1.27\n")
            .expect("write go.mod");
        let file_src = concat!(
            "package p\n",
            "const (\n",
            "    Base = 2\n",
            "    Step = iota + 1\n",
            "    Count\n",
            ")\n",
            "type Box[T any] struct { V T }\n",
        );
        std::fs::write(root.0.join("p.go"), file_src).expect("write p.go");

        let mut workspace = Workspace::default();
        workspace
            .initialize(&json!({
                "rootUri": workspace::uri(&root.0),
                "initializationOptions": { "goroot": "", "fileWatch": false },
            }))
            .expect("initialize workspace");
        let interned = workspace
            .files
            .keys()
            .find(|p| p.ends_with("p.go"))
            .expect("indexed p.go")
            .as_path();

        let mut store = TypeArena::with_shared_budget(&workspace, workspace.build_view(None));
        let type_src = concat!(
            "struct {",
            " buf [Count + Base]byte;",
            " cb func(x int, rest ...string) (res error);",
            " handler interface { Apply(Box[int]) ([Step]int, bool) };",
            " m map[string]chan<- int",
            " }",
        );
        let parsed = crate::syntax::parse_type(type_src);
        assert!(parsed.diagnostics.is_empty());
        assert!(!parsed.tree_overflowed());
        assert!(parsed.root().is_some());

        let CheckOutcome::Ok(struct_ty) =
            store.lower_type_syntax_checked(interned, type_src, &parsed, Some(0), &[])
        else {
            panic!("expected nested composite type to lower in a single pass");
        };

        // `Base = 2`, `Step = 1 + 1 = 2` (ordinal 1), `Count = 2 + 1 = 3` (ordinal 2) => 3 + 2 = 5.
        let buf_sel = store
            .literal_field(struct_ty, interned, "buf")
            .expect("buf field");
        assert_eq!(store.array_length(buf_sel.ty), Some(5));

        let cb_sel = store
            .literal_field(struct_ty, interned, "cb")
            .expect("cb field");
        let (cb_params, cb_results, cb_variadic) =
            store.signature_parts(cb_sel.ty).expect("cb signature");
        assert_eq!(cb_params.len(), 2);
        assert_eq!(cb_results.len(), 1);
        assert!(cb_variadic);
        let cb_names = store.callable_names(cb_sel.ty).expect("cb names");
        assert_eq!(cb_names.parameters, vec!["x", "rest"]);
        assert_eq!(cb_names.results, vec!["res"]);

        let handler_sel = store
            .literal_field(struct_ty, interned, "handler")
            .expect("handler field");
        assert_eq!(store.is_interface(handler_sel.ty), Some(true));
        let apply_sel = store
            .lookup_field_or_method(handler_sel.ty, false, interned, "Apply")
            .expect("lookup Apply")
            .expect("Apply method");
        let (apply_params, apply_results, apply_variadic) = store
            .signature_parts(apply_sel.ty)
            .expect("Apply signature");
        assert_eq!(apply_params.len(), 1);
        assert_eq!(apply_results.len(), 2);
        assert!(!apply_variadic);
        assert_eq!(store.array_length(apply_results[0]), Some(2));

        let map_sel = store
            .literal_field(struct_ty, interned, "m")
            .expect("m field");
        let (key_ty, val_ty) = store
            .composite_container_types(map_sel.ty)
            .expect("map container");
        assert!(matches!(
            store.basic_kind(key_ty.expect("map key")),
            Some(BasicKind::String)
        ));
        assert!(matches!(
            store.node(val_ty),
            Some(Ty::Chan {
                direction: Direction::Send,
                ..
            })
        ));

        // Tree overflow falls back cleanly to `BudgetExceeded`.
        let mut overflowed = crate::syntax::parse_type("int");
        overflowed.tree_root = None;
        overflowed.tree_overflowed = true;
        assert!(matches!(
            store.lower_type_syntax_checked(interned, "int", &overflowed, None, &[]),
            CheckOutcome::Unknown(UnknownReason::BudgetExceeded)
        ));
    }
}
