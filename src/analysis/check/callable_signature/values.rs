//! Callable values are followed through query-local type and initializer spans.
use super::CallableSignature;
use crate::analysis::check::{Inference, Selection};
use crate::analysis::scope::{Binding, Locals};
use crate::analysis::types::TypeArena;
use crate::syntax::{Expr, ExprId, ExprKind, Lexer, TokenKind};
use crate::text::SourceMap;
use crate::workspace::Workspace;
use std::ops::Range;
use std::path::Path;

type Information = (CallableSignature, bool);

impl Workspace {
    pub(crate) fn callable_annotation(
        &self,
        file: &Path,
        source: &str,
        locals: &Locals,
        span: Range<usize>,
    ) -> bool {
        Callable {
            workspace: self,
            file,
            source,
            locals,
        }
        .type_information(
            TypeSource {
                file,
                source,
                span,
                locals: Some(locals),
            },
            "func",
            None,
            0,
        )
        .is_some()
    }

    pub(crate) fn binding_signature(
        &self,
        file: &Path,
        source: &str,
        locals: &Locals,
        binding: &Binding,
    ) -> Option<Information> {
        Callable {
            workspace: self,
            file,
            source,
            locals,
        }
        .binding(binding, None, 0)
    }

    pub(crate) fn callable_type(&self, file: &Path, source: &str) -> bool {
        let mut context = crate::analysis::query::Context::borrowed(self, file, source);
        if let Some(callable) = context.callable_type(source) {
            return callable;
        }
        let locals = Locals::empty();
        Callable {
            workspace: self,
            file,
            source,
            locals: &locals,
        }
        .type_information(
            TypeSource {
                file,
                source,
                span: 0..source.len(),
                locals: None,
            },
            "func",
            None,
            0,
        )
        .is_some()
    }
    pub(crate) fn callable_information(
        &self,
        file: &Path,
        source: &str,
        locals: &Locals,
        body: (&[Expr], usize),
        function: ExprId,
    ) -> Option<Information> {
        Callable {
            workspace: self,
            file,
            source,
            locals,
        }
        .expression(body.0, body.1, function, 0)
    }
}

struct Callable<'a> {
    workspace: &'a Workspace,
    file: &'a Path,
    source: &'a str,
    locals: &'a Locals,
}

struct TypeSource<'a> {
    file: &'a Path,
    source: &'a str,
    span: Range<usize>,
    locals: Option<&'a Locals>,
}

impl Callable<'_> {
    fn expression(
        &self,
        expressions: &[Expr],
        base: usize,
        id: ExprId,
        depth: usize,
    ) -> Option<Information> {
        if depth >= 32 {
            return None;
        }
        let expression = &expressions[id.0];
        match &expression.kind {
            ExprKind::Call { function, .. } => {
                let inference = Inference::new(
                    self.workspace,
                    self.file,
                    self.source,
                    self.locals,
                    expressions,
                    base,
                );
                let value = inference.scoped_expression(id, 0, 1)?;
                // Structural function nodes intentionally omit parameter names.
                // Retain them from the already qualified callee result label
                // after proving the result through the native expression path.
                if let Some((callee, _)) = self.expression(expressions, base, *function, depth + 1)
                {
                    let header = format!("func {}", callee.label);
                    let signature = crate::syntax::parse_signature(&header);
                    if signature.diagnostics.is_empty() && signature.results.len() == 1 {
                        let result = &signature.results[0];
                        if let Some(information) = self.type_information(
                            TypeSource {
                                file: self.file,
                                source: &header,
                                span: result.annotation.clone(),
                                locals: None,
                            },
                            "func",
                            None,
                            depth + 1,
                        ) {
                            return Some(information);
                        }
                    }
                }
                return self.type_information(
                    TypeSource {
                        file: value.scope,
                        source: &value.text,
                        span: 0..value.text.len(),
                        locals: None,
                    },
                    "func",
                    None,
                    depth + 1,
                );
            }
            ExprKind::Index { .. } => {
                let inference = Inference::new(
                    self.workspace,
                    self.file,
                    self.source,
                    self.locals,
                    expressions,
                    base,
                );
                if inference
                    .callee(id)
                    .is_some_and(|(_, symbol)| symbol.generic)
                {
                    return inference.explicit_function_signature(id);
                }
            }
            ExprKind::Unary { operator, .. }
                if &self.source[base + operator.start..base + operator.end] == "<-" =>
            {
                let inference = Inference::new(
                    self.workspace,
                    self.file,
                    self.source,
                    self.locals,
                    expressions,
                    base,
                );
                let mut arena = TypeArena::new(self.workspace);
                let value = inference.receive_operand(&mut arena, id, 0, 1)?;
                arena.is_callable(value.ty)?.then_some(())?;
                let scoped = inference.scoped_native_type(
                    &mut arena,
                    value.ty,
                    self.file,
                    base + expression.span.start,
                )?;
                return self.type_information(
                    TypeSource {
                        file: scoped.scope,
                        source: &scoped.text,
                        span: 0..scoped.text.len(),
                        locals: None,
                    },
                    "func",
                    None,
                    depth + 1,
                );
            }
            ExprKind::TypeAssertion { asserted_type, .. } => {
                let inference = Inference::new(
                    self.workspace,
                    self.file,
                    self.source,
                    self.locals,
                    expressions,
                    base,
                );
                let mut arena = TypeArena::new(self.workspace);
                let value = inference.assertion_operand(&mut arena, id, 0, 1)?;
                arena.is_callable(value.ty)?.then_some(())?;
                return self.type_information(
                    TypeSource {
                        file: self.file,
                        source: self.source,
                        span: base + asserted_type.start..base + asserted_type.end,
                        locals: Some(self.locals),
                    },
                    "func",
                    None,
                    depth + 1,
                );
            }
            ExprKind::Parenthesized { expression } => {
                return self.expression(expressions, base, *expression, depth + 1);
            }
            ExprKind::Name => {
                let name = &self.source[base + expression.span.start..base + expression.span.end];
                if let Some(binding) = self.locals.resolve_at(base + expression.span.start, name) {
                    return self.binding(binding, None, depth + 1);
                }
            }
            ExprKind::FunctionLiteral { signature, .. } => {
                return self.type_information(
                    TypeSource {
                        file: self.file,
                        source: self.source,
                        span: base + signature.start..base + signature.end,
                        locals: Some(self.locals),
                    },
                    "func",
                    None,
                    depth + 1,
                );
            }
            ExprKind::Selector { operand, field } => {
                let inference = Inference::new(
                    self.workspace,
                    self.file,
                    self.source,
                    self.locals,
                    expressions,
                    base,
                );
                if let Some(receiver) = inference.scoped_receiver_type(id) {
                    match inference.selection(id) {
                        Some(Selection::Global(origin, symbol)) if symbol.is_method() => {
                            return self.workspace.scoped_method_signature(
                                self.file,
                                origin,
                                symbol,
                                (receiver.scope, &receiver.text),
                            );
                        }
                        Some(Selection::Member(member)) => {
                            let data = self.workspace.member_data(member)?;
                            let annotation = data.annotation.as_deref()?;
                            let header = if member.kind == crate::syntax::MemberKind::Method {
                                format!("func {annotation}")
                            } else {
                                annotation.to_owned()
                            };
                            // A field written as T may become callable only
                            // after substituting the receiver's type arguments.
                            let types = self.workspace.scoped_member_types(
                                self.file,
                                receiver.scope,
                                &receiver.text,
                                member,
                            )?;
                            let header = types.render(&header)?;
                            return self.type_information(
                                TypeSource {
                                    file: self.file,
                                    source: &header,
                                    span: 0..header.len(),
                                    locals: None,
                                },
                                member.name,
                                None,
                                depth + 1,
                            );
                        }
                        _ => {}
                    }
                }
                let mut operand = &expressions[operand.0];
                while let ExprKind::Parenthesized { expression } = operand.kind {
                    operand = &expressions[expression.0];
                }
                if matches!(operand.kind, ExprKind::Name) {
                    let name = &self.source[base + operand.span.start..base + operand.span.end];
                    if let Some(binding) = self.locals.resolve_at(base + operand.span.start, name)
                        && let Some(information) = self.binding(
                            binding,
                            Some(&self.source[base + field.start..base + field.end]),
                            depth + 1,
                        )
                    {
                        return Some(information);
                    }
                }
            }
            _ => {}
        }
        let (origin, symbol) = self.workspace.resolve_call(
            self.file,
            expressions,
            id,
            self.source,
            base,
            self.locals,
        )?;
        if symbol.is_value() {
            return self.global(origin, symbol, depth + 1);
        }
        symbol
            .is_callable()
            .then(|| {
                self.workspace
                    .signature_information(self.file, origin, symbol)
            })
            .flatten()
    }

    /// Follow a package value through the same bounded binding presentation as
    /// a local. The wrapper is query-local and keeps the declaring import scope.
    fn global(
        &self,
        origin: &Path,
        symbol: &crate::index::IndexedSymbol,
        depth: usize,
    ) -> Option<Information> {
        if depth >= 32 || symbol.is_const() {
            return None;
        }
        let declaration = symbol.declaration()?;
        if declaration.len() > super::render::MAX_LABEL_BYTES {
            return None;
        }
        let spec = self.workspace.source_slice(origin, declaration.clone())?;
        let wrapped = format!("{{var {spec}\n}}");
        let parsed = crate::syntax::parse_block(&wrapped);
        if !parsed.diagnostics.is_empty() {
            return None;
        }
        let locals = Locals::from_body(&wrapped, 0, &parsed);
        let binding = locals.visible_bindings(wrapped.len() - 1).find(|binding| {
            binding.name == symbol.name
                && binding
                    .selection
                    .start
                    .checked_sub(5)
                    .and_then(|offset| declaration.start.checked_add(offset))
                    == Some(symbol.start)
        })?;
        let (signature, _) = Callable {
            workspace: self.workspace,
            file: origin,
            source: &wrapped,
            locals: &locals,
        }
        .binding(binding, None, depth + 1)?;
        let parameters = signature.label.strip_prefix(&symbol.name)?;
        let ty = format!("func{parameters}");
        self.type_information(
            TypeSource {
                file: origin,
                source: &ty,
                span: 0..ty.len(),
                locals: None,
            },
            &symbol.name,
            None,
            depth + 1,
        )
    }

    fn binding(&self, binding: &Binding, field: Option<&str>, depth: usize) -> Option<Information> {
        if depth >= 32 || binding.object_kind.is_type() {
            return None;
        }
        let name = field.unwrap_or(&binding.name);
        if let Some(case) = self.locals.switch_case(binding) {
            let inference =
                Inference::new(self.workspace, self.file, self.source, self.locals, &[], 0);
            let mut arena = TypeArena::new(self.workspace);
            let value = inference.switch_operand(&mut arena, binding)?;
            arena.is_callable(value.ty)?.then_some(())?;
            let [span] = case.types.as_slice() else {
                return None;
            };
            return self.type_information(
                TypeSource {
                    file: self.file,
                    source: self.source,
                    span: span.clone(),
                    locals: Some(self.locals),
                },
                name,
                field,
                depth + 1,
            );
        }
        if let Some(annotation) = binding.annotation() {
            return self.type_information(
                TypeSource {
                    file: self.file,
                    source: self.source,
                    span: annotation.clone(),
                    locals: Some(self.locals),
                },
                name,
                field,
                depth + 1,
            );
        }
        if field.is_some()
            && let Some(ty) =
                self.workspace
                    .binding_type(self.file, self.source, self.locals, binding)
        {
            return self.type_information(
                TypeSource {
                    file: self.file,
                    source: &ty,
                    span: 0..ty.len(),
                    locals: None,
                },
                name,
                field,
                depth + 1,
            );
        }
        let (span, result, count) = binding.initializer()?;
        if result != 0 || !matches!(count, 1 | 2) {
            return None;
        }
        let parsed = crate::syntax::parse_expression(self.source.get(span.clone())?);
        if !parsed.diagnostics.is_empty() {
            return None;
        }
        if count == 2 {
            let inference = Inference::new(
                self.workspace,
                self.file,
                self.source,
                self.locals,
                &parsed.nodes,
                span.start,
            );
            let ExprKind::Unary { operator, .. } = &inference.unparenthesized(parsed.root).kind
            else {
                return None;
            };
            if inference.text(operator) != "<-" {
                return None;
            }
        }
        if field.is_some() {
            let ExprKind::Composite {
                literal_type: Some(ty),
                ..
            } = parsed.nodes[parsed.root.0].kind
            else {
                return None;
            };
            let ty = &parsed.nodes[ty.0].span;
            return self.type_information(
                TypeSource {
                    file: self.file,
                    source: self.source,
                    span: span.start + ty.start..span.start + ty.end,
                    locals: Some(self.locals),
                },
                name,
                field,
                depth + 1,
            );
        }
        let mut information = self.expression(&parsed.nodes, span.start, parsed.root, depth + 1)?;
        let opening = information.0.label.find('(')?;
        information.0.label.replace_range(..opening, name);
        Some(information)
    }

    fn type_information(
        &self,
        mut location: TypeSource<'_>,
        name: &str,
        field: Option<&str>,
        depth: usize,
    ) -> Option<Information> {
        if depth >= 32 {
            return None;
        }
        location.span = unwrapped(location.source, location.span, field.is_some())?;
        let source = location.source.get(location.span.clone())?;
        let parsed = crate::syntax::parse_type(source);
        if !parsed.diagnostics.is_empty() {
            return None;
        }
        let mut tokens = Lexer::new(source);
        let first = tokens.next()?;
        if first.kind == TokenKind::Func && field.is_none() {
            // Local type parameters/types must not be requalified as unrelated
            // package globals by the current declaration-only label formatter.
            if parsed.references.iter().any(|reference| {
                reference.qualifier.is_none()
                    && location
                        .locals
                        .and_then(|locals| {
                            locals.resolve_at(
                                location.span.start + reference.name.start,
                                &source[reference.name.clone()],
                            )
                        })
                        .is_some_and(|binding| binding.object_kind.is_type())
            }) {
                return None;
            }
            let signature = crate::syntax::parse_literal_signature(source);
            return self.workspace.signature_from_parsed(
                self.file,
                location.file,
                source,
                &signature,
                name,
            );
        }
        if first.kind == TokenKind::Struct {
            let field = field?;
            let same = self.file.parent() == location.file.parent()
                && self.workspace.files[self.file].package
                    == self.workspace.files[location.file].package;
            if !same && !crate::analysis::resolve::exported(field) {
                return None;
            }
            let container = tokens.next()?.span.start;
            let mut fields = parsed.fields.iter().filter(|member| {
                !member.embedded
                    && member.container == container
                    && &source[member.name.clone()] == field
            });
            let member = fields.next()?;
            if fields.next().is_some() {
                return None;
            }
            return self.type_information(
                TypeSource {
                    span: location.span.start + member.annotation.start
                        ..location.span.start + member.annotation.end,
                    ..location
                },
                name,
                None,
                depth + 1,
            );
        }
        if first.kind != TokenKind::Ident {
            return None;
        }
        let ty = crate::analysis::scope::named_type(source)?;
        if let Some(binding) = location.locals.and_then(|locals| {
            locals.resolve_at(
                location.span.start + first.span.start,
                crate::analysis::scope::named_root(&ty)
                    .split('.')
                    .next()
                    .expect("named type"),
            )
        }) {
            if !binding.object_kind.is_type()
                || crate::analysis::scope::named_root(&ty).contains('.')
            {
                return None;
            }
            return self.type_information(
                TypeSource {
                    span: binding.annotation()?.clone(),
                    ..location
                },
                name,
                field,
                depth + 1,
            );
        }
        let (origin, symbol) = self.workspace.embedded_target(location.file, &ty)?;
        let source = self.workspace.source(origin)?;
        let map = SourceMap::new(&source);
        let mut start = symbol.end;
        let end = map.offset(symbol.range.end)?;
        if let Some(token) = Lexer::new(&source[start..end]).next()
            && token.kind == TokenKind::Assign
        {
            start += token.span.end;
        }
        self.type_information(
            TypeSource {
                file: origin,
                source: &source,
                span: start..end,
                locals: None,
            },
            name,
            field,
            depth + 1,
        )
    }
}

fn unwrapped(source: &str, mut span: Range<usize>, pointer: bool) -> Option<Range<usize>> {
    for _ in 0..128 {
        let mut tokens = Lexer::new(source.get(span.clone())?)
            .filter(|token| token.kind != TokenKind::Semicolon);
        let first = tokens.next()?;
        if first.kind == TokenKind::Mul && pointer {
            span.start += first.span.end;
            continue;
        }
        if first.kind != TokenKind::LParen {
            return Some(span);
        }
        let mut depth = 1;
        let mut closing = None;
        for token in tokens.by_ref() {
            match token.kind {
                TokenKind::LParen => depth += 1,
                TokenKind::RParen => {
                    depth -= 1;
                    if depth == 0 {
                        closing = Some(token.span.start);
                        break;
                    }
                }
                _ => {}
            }
        }
        let closing = closing?;
        if tokens.next().is_some() {
            return None;
        }
        let start = span.start;
        span = start + first.span.end..start + closing;
    }
    None
}
