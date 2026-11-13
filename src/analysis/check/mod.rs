//! Lazy named receiver types, derived from initializer spans without cached trees.
mod assertion;
mod binding;
mod builtin;
mod call;
mod callable;
pub(crate) mod callable_signature;
pub(crate) mod classify;
mod composite;
mod conversion;
mod generics;
mod globals;
mod index;
mod iteration;
pub(crate) mod navigation;
mod receive;
mod scalar;
mod scope;
mod scoped;
pub(crate) mod shapes;
mod signature;
pub(crate) mod target;
pub(crate) mod type_definition;
use crate::analysis::member::MemberDeclaration;
use crate::analysis::method_set::ReceiverMember;
use crate::analysis::resolve::NameIndex;
use crate::analysis::scope::named;
use crate::analysis::scope::{self as locals, Binding, Locals};
use crate::analysis::types::TypeArena;
use crate::analysis::types::TypeForm;
use crate::index::IndexedSymbol as Symbol;
use crate::syntax::{Expr, ExprId, ExprKind};
use crate::text::SourceMap;
use crate::workspace::Workspace;
pub(crate) use binding::binding_detail;
pub(crate) use composite::{inferred_array_header, inferred_array_literal};
pub(crate) use iteration::interface_selector;
use scope::AnnotationType;
pub(crate) use scoped::ScopedType;
mod native_types;
pub(crate) use native_types::{
    NativeTypeCache, binding_type as native_binding_type, selector_member as native_selector_member,
};
use std::path::{Path, PathBuf};

impl Workspace {
    pub(crate) fn binding_type(
        &self,
        file: &Path,
        source: &str,
        locals: &Locals,
        binding: &Binding,
    ) -> Option<String> {
        self.infer_binding(file, source, locals, binding, 0)
    }

    fn infer_binding(
        &self,
        file: &Path,
        source: &str,
        locals: &Locals,
        binding: &Binding,
        depth: usize,
    ) -> Option<String> {
        if depth >= 32 {
            return None;
        }
        if locals.switch_case(binding).is_some() {
            return Inference {
                workspace: self,
                file,
                source,
                locals,
                expressions: &[],
                base: 0,
                depth: depth + 1,
                name_index: None,
                scoped_bindings: None,
                iota: None,
            }
            .scoped_switch_binding(binding)?
            .render(self, file);
        }
        let inference = Inference {
            workspace: self,
            file,
            source,
            locals,
            expressions: &[],
            base: 0,
            depth: depth + 1,
            name_index: None,
            scoped_bindings: None,
            iota: None,
        };
        if let Some(value) = inference.scoped_binding(binding) {
            return value?.render(self, file);
        }
        // Native declarations share one scope: local formals use the parameterized
        // arena and every other written annotation parses in the same arena. Only a
        // round-trip-proved spelling crosses the legacy text boundary. Each branch
        // builds its arena only when it has an annotation to resolve. A type
        // declaration is a name, not a value of its RHS type; member queries must
        // never treat `type T U` as a spelling of U.
        if !binding.object_kind.is_type() {
            if let Some((text, offset)) = binding
                .annotation()
                .and_then(|span| source.get(span.clone()).map(|text| (text, span.start)))
            {
                let mut arena = TypeArena::new(self);
                if let AnnotationType::Resolved(ty) =
                    inference.annotation_type(&mut arena, text, offset)
                    && let Some(text) = arena.render_resolved(ty, file)
                {
                    return Some(text);
                }
            }
            if binding.declaration == locals::DeclarationKind::Variadic {
                let mut arena = TypeArena::new(self);
                if let Some(ty) = inference.variadic_type(&mut arena, binding)
                    && let Some(text) = arena.render_resolved(ty, file)
                {
                    return Some(text);
                }
            }
        }
        if let Some(ty) = &binding.declared_type {
            if let Some(annotation) = binding
                .annotation()
                .and_then(|span| source.get(span.clone()))
                && annotation.trim() != ty
                && self.receiver_in_scope(file, ty, locals, binding.selection.start)
                && let Some(ty) = self.inference_value_type(file, file, annotation)
            {
                return Some(ty);
            }
            return self
                .receiver_in_scope(file, ty, locals, binding.selection.start)
                .then(|| ty.clone());
        }
        if let Some(span) = binding.annotation()
            && let Some(annotation) = source.get(span.clone())
            && locals::named_type(annotation).is_none()
        {
            if !type_names_in_scope(annotation, locals, span.start) {
                return None;
            }
            return self.inference_value_type(file, file, annotation);
        }
        if binding.range_initializer().is_some() {
            return self.infer_range_binding(file, source, locals, binding, depth + 1);
        }
        let (span, result, count) = binding.initializer()?;
        let parsed = crate::syntax::parse_expression(source.get(span.clone())?);
        if !parsed.diagnostics.is_empty() {
            return None;
        }
        let inferred = Inference {
            workspace: self,
            name_index: None,
            scoped_bindings: None,
            file,
            source,
            locals,
            expressions: &parsed.nodes,
            base: span.start,
            depth: depth + 1,
            iota: None,
        }
        .expression(parsed.root, result, count)?;
        self.receiver_in_scope(file, &inferred, locals, binding.selection.start)
            .then_some(inferred)
    }

    fn inference_value_type(&self, file: &Path, origin: &Path, annotation: &str) -> Option<String> {
        // Plain named projections already resolve declaration identity. Avoid
        // parsing/resolving/rendering that same name again in this migration
        // adapter; the structural arena is needed for wrappers and containers.
        if !annotation.trim_start().starts_with('*')
            && let Some(name) = locals::named_type(annotation)
        {
            return self.result_type(file, origin, &name);
        }
        let mut arena = TypeArena::new(self);
        if let Some(ty) = arena.parse(origin, annotation) {
            return arena.render(ty, file);
        }
        if arena.has_type_error() {
            return None;
        }
        if let Some(name) = locals::named_type(annotation) {
            let name = self.result_type(file, origin, &name)?;
            return Some(
                if crate::syntax::Lexer::new(annotation).next()?.kind
                    == crate::syntax::TokenKind::Mul
                {
                    format!("*{name}")
                } else {
                    name
                },
            );
        }
        let presentation = self.declared_type_presentation(file, origin, annotation)?;
        matches!(
            presentation.form,
            TypeForm::Slice
                | TypeForm::Array
                | TypeForm::Map
                | TypeForm::Chan
                | TypeForm::Pointer
                | TypeForm::Signature
        )
        .then_some(presentation.text)
    }

    // Re-express a named type for consumers that require a local spelling.
    // Semantic receiver lookup keeps an independent scope in ScopedType.
    pub(crate) fn result_type(&self, file: &Path, origin: &Path, name: &str) -> Option<String> {
        if name != locals::named_root(name) {
            // Each argument belongs to the declaration file's import scope,
            // including files in the same package with different import aliases.
            return self.requalified_type(file, origin, name);
        }
        let (path, symbol) = self.type_declaration(origin, name)?;
        let same_package =
            path.parent() == file.parent() && self.files[path].package == self.files[file].package;
        let candidate = if same_package {
            symbol.name.clone()
        } else {
            let import = self.import_path(path)?;
            let imported = self.files[file]
                .imports
                .iter()
                .find(|candidate| !candidate.is_blank() && candidate.path == import)?;
            let alias = self.import_name(imported)?;
            if imported.is_dot() {
                symbol.name.clone()
            } else {
                format!("{alias}.{}", symbol.name)
            }
        };
        let (resolved_path, resolved) = self.type_declaration(file, &candidate)?;
        (resolved_path == path && resolved.start == symbol.start).then_some(candidate)
    }
}

pub(crate) enum Selection<'a> {
    Global(&'a PathBuf, &'a Symbol),
    Member(MemberDeclaration<'a>),
}

#[derive(Clone)]
/// Memoized native lexical binding type. `Unknown` records a represented
/// failure together with the inference depth that proved it: budgets shrink
/// with depth, so a failure may only be reused at equal or greater depth.
pub(crate) enum CachedBinding<'a> {
    Type(ScopedType<'a>),
    Unknown(usize),
}

#[derive(Clone, Copy)]
pub(crate) struct Inference<'workspace, 'query> {
    workspace: &'workspace Workspace,
    name_index: Option<&'query NameIndex<'workspace>>,
    /// Request-local memo shared by the resolver's inference instances.
    scoped_bindings: Option<
        &'query std::cell::RefCell<
            std::collections::HashMap<(usize, usize), CachedBinding<'workspace>>,
        >,
    >,
    file: &'query Path,
    source: &'query str,
    locals: &'query Locals,
    expressions: &'query [Expr],
    base: usize,
    depth: usize,
    iota: Option<u32>,
}

impl<'workspace, 'query> Inference<'workspace, 'query> {
    pub(crate) fn literal_field(&self, key: ExprId) -> Option<MemberDeclaration<'workspace>> {
        let keyed = self.expressions.iter().position(|expression| {
            matches!(expression.kind,
            ExprKind::KeyValue { key: other, .. } if other == key)
        })?;
        let literal_type =
            self.expressions
                .iter()
                .find_map(|expression| match &expression.kind {
                    ExprKind::Composite {
                        literal_type: Some(ty),
                        elements,
                    } if elements.contains(&ExprId(keyed)) => Some(*ty),
                    _ => None,
                })?;
        let ty = self.named(literal_type)?;
        let (path, owner) = self.workspace.composite_target(self.file, &ty)?;
        self.workspace
            .receiver_members(self.file, &ty)
            .into_iter()
            .find_map(|member| match member {
                ReceiverMember::Declaration(member)
                    if member.kind == crate::syntax::MemberKind::Field
                        && member.file == path
                        && std::ptr::eq(member.owner, owner)
                        && member.name == self.text(&self.expressions[key.0].span) =>
                {
                    Some(member)
                }
                _ => None,
            })
    }

    pub(crate) fn selection(&self, id: ExprId) -> Option<Selection<'workspace>> {
        if self.depth >= 32 {
            return None;
        }
        let expression = self.unparenthesized(id);
        let ExprKind::Selector { operand, field } = &expression.kind else {
            return self
                .callee(id)
                .map(|(path, symbol)| Selection::Global(path, symbol));
        };
        if let Some(selection) = self.method_expression_target(id) {
            return Some(selection);
        }
        let receiver = self.unparenthesized(*operand);
        if matches!(receiver.kind, ExprKind::Name)
            && self
                .locals
                .resolve_at(self.base + receiver.span.start, self.text(&receiver.span))
                .is_none()
            && self.global_symbol(*operand).is_none()
        {
            return self
                .callee(id)
                .map(|(path, symbol)| Selection::Global(path, symbol));
        }
        let ty = self.scoped_receiver_type(id)?;
        self.workspace
            .scoped_receiver_member(self.file, ty.scope, &ty.text, self.text(field))
            .map(|member| match member {
                ReceiverMember::Method(path, symbol) => Selection::Global(path, symbol),
                ReceiverMember::Declaration(member) => Selection::Member(member),
            })
    }

    pub(crate) fn new(
        workspace: &'workspace Workspace,
        file: &'query Path,
        source: &'query str,
        locals: &'query Locals,
        expressions: &'query [Expr],
        base: usize,
    ) -> Self {
        Self {
            workspace,
            name_index: None,
            scoped_bindings: None,
            file,
            source,
            locals,
            expressions,
            base,
            depth: 0,
            iota: None,
        }
    }
    pub(crate) fn with_name_index(mut self, index: Option<&'query NameIndex<'workspace>>) -> Self {
        self.name_index = index;
        self
    }
    /// Shared request-local binding memo, propagated into nested inferences
    /// created for receiver/range expressions inside classification.
    pub(super) fn shared_scoped_bindings(
        &self,
    ) -> Option<
        &'query std::cell::RefCell<
            std::collections::HashMap<(usize, usize), CachedBinding<'workspace>>,
        >,
    > {
        self.scoped_bindings
    }

    pub(crate) fn with_scoped_bindings(
        mut self,
        cache: &'query std::cell::RefCell<
            std::collections::HashMap<(usize, usize), CachedBinding<'workspace>>,
        >,
    ) -> Self {
        self.scoped_bindings = Some(cache);
        self
    }
    fn next(&self) -> Self {
        Self {
            depth: self.depth + 1,
            ..*self
        }
    }

    pub(crate) fn receiver_type(&self, id: ExprId) -> Option<String> {
        self.scoped_receiver_type(id)?
            .render(self.workspace, self.file)
    }

    fn unparenthesized(&self, mut id: ExprId) -> &Expr {
        loop {
            let expression = &self.expressions[id.0];
            if let ExprKind::Parenthesized { expression } = expression.kind {
                id = expression;
            } else {
                return expression;
            }
        }
    }

    pub(crate) fn callee(&self, id: ExprId) -> Option<(&'workspace PathBuf, &'workspace Symbol)> {
        if self.depth >= 32 {
            return None;
        }
        let expression = self.unparenthesized(id);
        match &expression.kind {
            ExprKind::Index { operand, .. } => {
                let (file, symbol) = self.next().callee(*operand)?;
                (symbol.is_function() && symbol.generic).then_some((file, symbol))
            }
            ExprKind::Name => {
                let name = self.text(&expression.span);
                if self
                    .locals
                    .resolve_at(self.base + expression.span.start, name)
                    .is_some()
                {
                    return None;
                }
                if let Some(index) = self.name_index.filter(|index| index.name() == name) {
                    index.unqualified(self.file)
                } else {
                    self.workspace.unqualified_symbol(self.file, name)
                }
            }
            ExprKind::Selector { operand, field } => {
                let receiver = self.unparenthesized(*operand);
                if matches!(receiver.kind, ExprKind::Name)
                    && self
                        .locals
                        .resolve_at(self.base + receiver.span.start, self.text(&receiver.span))
                        .is_none()
                    && self.global_symbol(*operand).is_none()
                {
                    if let Some(index) = self
                        .name_index
                        .filter(|index| index.name() == self.text(field))
                    {
                        return index.imported(self.file, self.text(&receiver.span));
                    }
                    return self
                        .workspace
                        .imported_symbol_iter(self.file, self.text(&receiver.span))
                        .find(|(_, symbol)| symbol.name == self.text(field));
                }
                match self.selection(id)? {
                    Selection::Global(path, method) => Some((path, method)),
                    Selection::Member(_) => None,
                }
            }
            _ => None,
        }
    }
    fn text(&self, span: &std::ops::Range<usize>) -> &str {
        &self.source[self.base + span.start..self.base + span.end]
    }

    fn named(&self, id: ExprId) -> Option<String> {
        let expression = &self.expressions[id.0];
        self.named_span(&expression.span)
    }

    fn named_span(&self, span: &std::ops::Range<usize>) -> Option<String> {
        let name = locals::named_type(self.text(span))?;
        if self
            .locals
            .resolve_at(
                self.base + span.start,
                locals::named_root(&name).split('.').next()?,
            )
            .is_some()
        {
            return None;
        }
        self.workspace.result_type(self.file, self.file, &name)
    }

    pub(crate) fn expression(&self, id: ExprId, result: usize, count: usize) -> Option<String> {
        if self.depth >= 32 {
            return None;
        }
        let expression = &self.expressions[id.0];
        if self.global_symbol(id).is_some() && result == 0 && count == 1 {
            return self.scoped_global(id)?.render(self.workspace, self.file);
        }
        if let ExprKind::Selector { operand, .. } = &expression.kind
            && result == 0
            && count == 1
            && self.global_root(*operand)
        {
            return self
                .scoped_expression(id, result, count)?
                .render(self.workspace, self.file);
        }
        if let ExprKind::Unary { operator, .. } = &expression.kind
            && self.text(operator) == "<-"
        {
            return self
                .scoped_receive(id, result, count)?
                .render(self.workspace, self.file);
        }
        if let ExprKind::Parenthesized { expression } = expression.kind {
            return self.next().expression(expression, result, count);
        }
        if let ExprKind::Call {
            function,
            arguments,
            variadic,
        } = &expression.kind
        {
            return self
                .call_result(*function, arguments, *variadic, result, count)?
                .render(self.workspace, self.file);
        }
        if matches!(expression.kind, ExprKind::TypeAssertion { .. }) {
            return self
                .scoped_assertion(id, result, count)?
                .render(self.workspace, self.file);
        }
        if matches!(expression.kind, ExprKind::Slice { .. }) {
            return self
                .scoped_slice(id, result, count)?
                .render(self.workspace, self.file);
        }
        if matches!(expression.kind, ExprKind::Index { .. }) {
            return self
                .scoped_index(id, result, count)?
                .render(self.workspace, self.file);
        }

        if result != 0 || count != 1 {
            return None;
        }
        match &expression.kind {
            ExprKind::Literal | ExprKind::Binary { .. } => self.scalar_type(id),
            ExprKind::Selector { operand, .. } => {
                let mut arena = TypeArena::new(self.workspace);
                if let Some(method) = self.selector_operand(&mut arena, id) {
                    return arena.render_resolved(method?.ty, self.file);
                }
                let Selection::Member(member) = self.next().selection(id)? else {
                    let mut arena = TypeArena::new(self.workspace);
                    let operand = self.function_operand(&mut arena, id)?;
                    return arena.render_resolved(operand.ty, self.file);
                };
                if member.kind != crate::syntax::MemberKind::Field {
                    return None;
                }
                let storage = (member.file != self.file)
                    .then(|| self.workspace.source(member.file))
                    .flatten();
                let source = if member.file == self.file {
                    self.source
                } else {
                    storage.as_deref()?
                };
                let map = SourceMap::new(source);
                let data = Workspace::member_data_in_source(member, source, &map)?;
                let annotation = data.annotation.as_deref()?;
                let declaration =
                    source.get(member.owner.end..map.offset(member.owner.range.end)?)?;
                let parameters = named::parameters(declaration)?;
                if !parameters.is_empty() {
                    let receiver = locals::named_type(&self.next().expression(*operand, 0, 1)?)?;
                    let substituted = self
                        .workspace
                        .instantiated_member_type(self.file, &receiver, member, annotation)?;
                    if locals::named_type(&substituted).is_none() {
                        return self.workspace.inference_value_type(
                            self.file,
                            self.file,
                            &substituted,
                        );
                    }
                    let name = locals::named_type(&substituted)?;
                    let qualifier = locals::named_root(&name).split('.').next()?;
                    if self
                        .locals
                        .resolve_at(self.base + expression.span.start, qualifier)
                        .is_some()
                    {
                        return None;
                    }
                    return if crate::syntax::Lexer::new(&substituted).next()?.kind
                        == crate::syntax::TokenKind::Mul
                    {
                        self.workspace
                            .inference_value_type(self.file, self.file, &substituted)
                    } else {
                        Some(name)
                    };
                }
                self.workspace
                    .inference_value_type(self.file, member.file, annotation)
            }
            ExprKind::Name => {
                if matches!(self.text(&expression.span), "true" | "false") {
                    return self.scalar_type(id);
                }
                if self
                    .locals
                    .resolve_at(
                        self.base + expression.span.start,
                        self.text(&expression.span),
                    )
                    .is_none()
                {
                    let mut arena = TypeArena::new(self.workspace);
                    let operand = self.function_operand(&mut arena, id)?;
                    return arena.render_resolved(operand.ty, self.file);
                }
                let binding = self.locals.resolve_at(
                    self.base + expression.span.start,
                    self.text(&expression.span),
                )?;
                self.workspace.infer_binding(
                    self.file,
                    self.source,
                    self.locals,
                    binding,
                    self.depth + 1,
                )
            }
            ExprKind::Composite {
                literal_type: Some(ty),
                ..
            } => {
                if self.inferred_array_element(*ty).is_some() {
                    let mut arena = TypeArena::new(self.workspace);
                    let (operand, _) = self.inferred_array_operand(&mut arena, id)?;
                    return arena.render_resolved(operand.ty, self.file);
                }
                let span = &self.expressions[ty.0].span;
                let annotation = self.text(span);
                if !type_names_in_scope(annotation, self.locals, self.base + span.start) {
                    return None;
                }
                self.workspace
                    .inference_value_type(self.file, self.file, annotation)
            }
            ExprKind::FunctionLiteral { .. } => {
                let mut arena = TypeArena::new(self.workspace);
                let operand = self.function_operand(&mut arena, id)?;
                arena.render_resolved(operand.ty, self.file)
            }
            ExprKind::Unary { operator, .. } if matches!(self.text(operator), "&" | "*") => {
                let mut arena = TypeArena::new(self.workspace);
                let operand = self.value_operand(&mut arena, id)?;
                arena.render_resolved(operand.ty, self.file)
            }
            ExprKind::Unary { operator, .. }
                if matches!(self.text(operator), "+" | "-" | "^" | "!") =>
            {
                self.scalar_type(id)
            }
            _ => None,
        }
    }
}

// Local types/formals need distinct identities before this checker can support
// them. Never resolve one through a same-spelled package declaration.
fn type_names_in_scope(annotation: &str, locals: &Locals, base: usize) -> bool {
    if annotation.len() > 16 * 1024 {
        return false;
    }
    // Existing generic receiver projection owns formal-argument handling. Keep
    // that compatibility path while rejecting a shadowed declaration root.
    if let Some(name) = locals::named_type(annotation) {
        return locals
            .resolve_at(
                base,
                locals::named_root(&name)
                    .split('.')
                    .next()
                    .expect("named root"),
            )
            .is_none();
    }
    let syntax = crate::syntax::parse_type(annotation);
    syntax.diagnostics.is_empty()
        && syntax.references.iter().all(|reference| {
            let span = reference.qualifier.as_ref().unwrap_or(&reference.name);
            locals
                .resolve_at(base + span.start, &annotation[span.clone()])
                .is_none()
        })
}
