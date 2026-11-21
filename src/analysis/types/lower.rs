//! AST-driven type lowering from [`crate::syntax::TypeSyntax`] to compacted [`Ty`] nodes.
use super::{
    BasicKind, CheckOutcome, Direction, MAX_BYTES, MAX_DEPTH, Ty, TypeArena, TypeId, UnknownReason,
};
use crate::analysis::engine::obj::DeclId;
use crate::analysis::engine::resolve::TypeNameLookup;
use crate::syntax::{Lexer, TokenKind, TypeNode, TypeNodeKind, TypeSyntax};
use std::path::Path;

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
        if source.len() > MAX_BYTES {
            return CheckOutcome::Unknown(UnknownReason::BudgetExceeded);
        }
        let parsed = crate::syntax::parse_type(source);
        self.lower_type_syntax_checked(origin, source, &parsed, file_offset, environment)
    }

    pub(super) fn lower_type_syntax_checked(
        &mut self,
        origin: &Path,
        source: &str,
        parsed: &TypeSyntax,
        file_offset: Option<usize>,
        environment: &[(&str, TypeId)],
    ) -> CheckOutcome<TypeId> {
        if source.len() > MAX_BYTES {
            return CheckOutcome::Unknown(UnknownReason::BudgetExceeded);
        }
        if !parsed.diagnostics.is_empty() || parsed.type_nodes.is_empty() {
            return CheckOutcome::Unknown(UnknownReason::SyntaxError);
        }
        let mut cursor = 0;
        let ty = match self.lower_type_node(
            origin,
            source,
            file_offset,
            &parsed.type_nodes,
            &mut cursor,
            environment,
            0,
        ) {
            CheckOutcome::Ok(ty) => ty,
            other => return other,
        };
        if cursor != parsed.type_nodes.len() {
            return CheckOutcome::Unknown(UnknownReason::SyntaxError);
        }
        CheckOutcome::Ok(ty)
    }

    #[allow(clippy::too_many_arguments)]
    fn lower_type_node(
        &mut self,
        origin: &Path,
        source: &str,
        file_offset: Option<usize>,
        nodes: &[TypeNode],
        cursor: &mut usize,
        environment: &[(&str, TypeId)],
        depth: usize,
    ) -> CheckOutcome<TypeId> {
        if depth >= MAX_DEPTH {
            return CheckOutcome::Unknown(UnknownReason::BudgetExceeded);
        }
        let Some(node) = nodes.get(*cursor) else {
            return CheckOutcome::Unknown(UnknownReason::SyntaxError);
        };
        *cursor += 1;

        let ty = match node.kind {
            TypeNodeKind::Parenthesized => {
                return self.lower_type_node(
                    origin,
                    source,
                    file_offset,
                    nodes,
                    cursor,
                    environment,
                    depth + 1,
                );
            }
            TypeNodeKind::Pointer => match self.lower_type_node(
                origin,
                source,
                file_offset,
                nodes,
                cursor,
                environment,
                depth + 1,
            ) {
                CheckOutcome::Ok(element) => Ty::Pointer(element),
                other => return other,
            },
            TypeNodeKind::Slice => match self.lower_type_node(
                origin,
                source,
                file_offset,
                nodes,
                cursor,
                environment,
                depth + 1,
            ) {
                CheckOutcome::Ok(element) => Ty::Slice(element),
                other => return other,
            },
            TypeNodeKind::Array => {
                let Some(elem_node) = nodes.get(*cursor) else {
                    return CheckOutcome::Unknown(UnknownReason::SyntaxError);
                };
                let elem_start = elem_node.span.start;
                let Some(prefix) = source.get(node.span.start + 1..elem_start) else {
                    return CheckOutcome::Unknown(UnknownReason::SyntaxError);
                };
                let Some(length_str) = prefix.trim_end().strip_suffix(']').map(str::trim) else {
                    return CheckOutcome::Unknown(UnknownReason::SyntaxError);
                };
                let element = match self.lower_type_node(
                    origin,
                    source,
                    file_offset,
                    nodes,
                    cursor,
                    environment,
                    depth + 1,
                ) {
                    CheckOutcome::Ok(ty) => ty,
                    other => return other,
                };
                let Some(length) = parse_symbolic_integer(length_str)
                    .or_else(|| self.eval_const_array_length(origin, length_str))
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
            TypeNodeKind::Map => {
                let key = match self.lower_type_node(
                    origin,
                    source,
                    file_offset,
                    nodes,
                    cursor,
                    environment,
                    depth + 1,
                ) {
                    CheckOutcome::Ok(ty) => ty,
                    other => return other,
                };
                if !self.mode.is_symbolic() && self.comparable(key, depth + 1) == Some(false) {
                    return CheckOutcome::Invalid(());
                }
                let value = match self.lower_type_node(
                    origin,
                    source,
                    file_offset,
                    nodes,
                    cursor,
                    environment,
                    depth + 1,
                ) {
                    CheckOutcome::Ok(ty) => ty,
                    other => return other,
                };
                Ty::Map { key, value }
            }
            TypeNodeKind::Chan | TypeNodeKind::SendChan | TypeNodeKind::RecvChan => {
                let direction = match node.kind {
                    TypeNodeKind::SendChan => Direction::Send,
                    TypeNodeKind::RecvChan => Direction::Receive,
                    _ => Direction::Both,
                };
                let element = match self.lower_type_node(
                    origin,
                    source,
                    file_offset,
                    nodes,
                    cursor,
                    environment,
                    depth + 1,
                ) {
                    CheckOutcome::Ok(ty) => ty,
                    other => return other,
                };
                Ty::Chan { direction, element }
            }
            TypeNodeKind::Function => {
                while nodes
                    .get(*cursor)
                    .is_some_and(|n| n.span.start >= node.span.start && n.span.end <= node.span.end)
                {
                    *cursor += 1;
                }
                let Some(header) = source.get(node.span.clone()) else {
                    return CheckOutcome::Unknown(UnknownReason::SyntaxError);
                };
                let signature = crate::syntax::parse_literal_signature(header);
                return match self.signature(
                    origin,
                    header,
                    file_offset.map(|base| base + node.span.start),
                    &signature,
                    environment,
                    depth + 1,
                ) {
                    Some(ty) => CheckOutcome::Ok(ty),
                    None => CheckOutcome::Unknown(UnknownReason::Unsupported),
                };
            }
            TypeNodeKind::Struct => {
                while nodes
                    .get(*cursor)
                    .is_some_and(|n| n.span.start >= node.span.start && n.span.end <= node.span.end)
                {
                    *cursor += 1;
                }
                let Some(body) = source.get(node.span.clone()) else {
                    return CheckOutcome::Unknown(UnknownReason::SyntaxError);
                };
                return match self.read_struct_at(
                    origin,
                    body,
                    file_offset.map(|base| base + node.span.start),
                    environment,
                    depth,
                ) {
                    Some(ty) => CheckOutcome::Ok(ty),
                    None => CheckOutcome::Unknown(UnknownReason::Unsupported),
                };
            }
            TypeNodeKind::Interface => {
                while nodes
                    .get(*cursor)
                    .is_some_and(|n| n.span.start >= node.span.start && n.span.end <= node.span.end)
                {
                    *cursor += 1;
                }
                let Some(body) = source.get(node.span.clone()) else {
                    return CheckOutcome::Unknown(UnknownReason::SyntaxError);
                };
                return self.read_interface_at_checked(
                    origin,
                    body,
                    file_offset.map(|base| base + node.span.start),
                    environment,
                    depth + 1,
                );
            }
            TypeNodeKind::Name | TypeNodeKind::Qualified => {
                let inst_span = if let Some(inst) = nodes.get(*cursor).filter(|next| {
                    next.kind == TypeNodeKind::Instantiation && next.span.start == node.span.start
                }) {
                    let span = inst.span.clone();
                    *cursor += 1;
                    Some(span)
                } else {
                    None
                };
                let Some(name) = source.get(node.span.clone()) else {
                    return CheckOutcome::Unknown(UnknownReason::SyntaxError);
                };
                if name == "_" {
                    return CheckOutcome::Unknown(UnknownReason::SyntaxError);
                }
                if node.kind == TypeNodeKind::Name
                    && let Some((_, ty)) = environment.iter().find(|(param, _)| *param == name)
                {
                    if let Some(inst) = inst_span {
                        while nodes.get(*cursor).is_some_and(|n| n.span.end < inst.end) {
                            *cursor += 1;
                        }
                        return CheckOutcome::Invalid(());
                    }
                    return CheckOutcome::Ok(*ty);
                }
                let Some(declaration) = self.resolve_type_name(origin, name) else {
                    return CheckOutcome::Unknown(UnknownReason::MissingMetadata);
                };
                if declaration.is_none() && matches!(name, "any" | "error") {
                    if let Some(inst) = inst_span {
                        while nodes.get(*cursor).is_some_and(|n| n.span.end < inst.end) {
                            *cursor += 1;
                        }
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
                    if let Some(inst) = inst_span {
                        while nodes.get(*cursor).is_some_and(|n| n.span.end < inst.end) {
                            *cursor += 1;
                        }
                        return CheckOutcome::Invalid(());
                    }
                    Ty::Basic(kind)
                } else {
                    let Some(DeclId { file, symbol }) = declaration else {
                        return CheckOutcome::Unknown(UnknownReason::MissingMetadata);
                    };
                    if symbol.name == "Pointer"
                        && self.workspace.import_path(file).as_deref() == Some("unsafe")
                    {
                        if let Some(inst) = inst_span {
                            while nodes.get(*cursor).is_some_and(|n| n.span.end < inst.end) {
                                *cursor += 1;
                            }
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
                    let mut arguments = Vec::new();
                    if let Some(inst) = inst_span {
                        while nodes
                            .get(*cursor)
                            .is_some_and(|n| n.span.start >= node.span.end && n.span.end < inst.end)
                        {
                            if arguments.len() >= self.active_node_limit() {
                                return CheckOutcome::Unknown(UnknownReason::BudgetExceeded);
                            }
                            let arg = match self.lower_type_node(
                                origin,
                                source,
                                file_offset,
                                nodes,
                                cursor,
                                environment,
                                depth + 1,
                            ) {
                                CheckOutcome::Ok(ty) => ty,
                                other => return other,
                            };
                            arguments.push(arg);
                        }
                    }
                    return match self.push_named(DeclId { file, symbol }, &arguments, symbol.alias)
                    {
                        Some(ty) => CheckOutcome::Ok(ty),
                        None => CheckOutcome::Unknown(UnknownReason::BudgetExceeded),
                    };
                }
            }
            TypeNodeKind::Instantiation => {
                return CheckOutcome::Unknown(UnknownReason::SyntaxError);
            }
        };
        match self.push(ty) {
            Some(id) => CheckOutcome::Ok(id),
            None => CheckOutcome::Unknown(UnknownReason::BudgetExceeded),
        }
    }

    fn eval_const_array_length(&mut self, origin: &Path, expr: &str) -> Option<u64> {
        if expr.is_empty() || expr.len() > 1024 {
            return None;
        }
        let int_bits = self.integer_bits()?;
        let op = self.constant_expression(origin, expr)?;
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
