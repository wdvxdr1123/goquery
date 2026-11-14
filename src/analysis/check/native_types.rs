//! Query-local native type projection for range variables and call-chain
//! intermediate bindings.
//!
//! A written chain such as `q.Exec(...).Vector()` is resolved from declaration
//! headers, so a range variable over its slice result never enters the legacy
//! recursive binding checker. Only bindings whose initializer is a call/selector
//! or whose range operand is written are projected; other bindings keep the
//! existing resolver behavior. The cache lives for one query only.
use crate::analysis::method_set::ReceiverMember;
use crate::analysis::scope::{Binding, Locals};
use crate::index::IndexedSymbol as Symbol;
use crate::syntax::ExprKind;
use crate::workspace::Workspace;
use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use std::path::Path;

const MAX_ENTRIES: usize = 4_096;
const MAX_DEPTH: usize = 16;

#[derive(Default)]
pub(crate) struct NativeTypeCache {
    bindings: RefCell<HashMap<(usize, usize), Option<String>>>,
    expressions: RefCell<HashMap<(usize, usize), Option<String>>>,
}

impl NativeTypeCache {
    pub(crate) fn new() -> Self {
        Self::default()
    }
}

struct NativeTypeContext<'a> {
    workspace: &'a Workspace,
    file: &'a Path,
    source: &'a str,
    locals: &'a Locals,
    cache: &'a NativeTypeCache,
}

/// Whether the written initializer is a call/selector chain (checked after
/// parsing by `binding_type`; range operands are always admitted).
pub(crate) fn binding_type(
    workspace: &Workspace,
    file: &Path,
    source: &str,
    locals: &Locals,
    cache: &NativeTypeCache,
    binding: &Binding,
) -> Option<String> {
    NativeTypeContext {
        workspace,
        file,
        source,
        locals,
        cache,
    }
    .binding_type(binding)
}

impl NativeTypeContext<'_> {
    fn binding_type(&self, binding: &Binding) -> Option<String> {
        let key = (binding.selection.start, binding.selection.end);
        if let Some(value) = self.cache.bindings.borrow().get(&key) {
            return value.clone();
        }
        let value = self.binding_type_uncached(binding);
        if self.workspace.trace.queries {
            eprintln!(
                "native binding {} -> {}",
                binding.name,
                value.as_deref().unwrap_or("none")
            );
        }
        if self.cache.bindings.borrow().len() < MAX_ENTRIES {
            self.cache.bindings.borrow_mut().insert(key, value.clone());
        }
        value
    }

    fn binding_type_uncached(&self, binding: &Binding) -> Option<String> {
        // Written annotations and declared types are already consumer spellings.
        if let Some(span) = binding.annotation() {
            return self
                .source
                .get(span.clone())
                .map(|text| text.trim().to_owned());
        }
        if let Some(declared) = &binding.declared_type {
            return Some(declared.clone());
        }
        if let Some((span, result, count)) = binding.range_initializer() {
            let container = self.expression_type(span, 0)?;
            return range_element(self.workspace, self.file, &container, result, count);
        }
        let (span, result, count) = binding.initializer()?;
        if !initializer_is_chain(self.source, span) {
            return None;
        }
        self.expression_result_type(span, result, count, 0)
    }

    fn expression_result_type(
        &self,
        span: &Range<usize>,
        result: usize,
        count: usize,
        depth: usize,
    ) -> Option<String> {
        let text = self.source.get(span.clone())?;
        let parsed = crate::syntax::parse_expression(text);
        if !parsed.diagnostics.is_empty() {
            return None;
        }
        let root = &parsed.nodes[parsed.root.0];
        match &root.kind {
            ExprKind::Call { function, .. } => {
                self.call_result_type(&parsed, text, span.start, *function, result, depth + 1)
            }
            ExprKind::TypeAssertion { asserted_type, .. }
                if result == 0 && (count == 1 || count == 2) =>
            {
                text.get(asserted_type.clone()).map(str::to_owned)
            }
            _ => {
                let ty = self.expression_type(span, depth + 1)?;
                if count > 1 {
                    range_element(self.workspace, self.file, &ty, result, count)
                } else {
                    Some(ty)
                }
            }
        }
    }

    fn expression_type(&self, span: &Range<usize>, depth: usize) -> Option<String> {
        if depth > MAX_DEPTH {
            return None;
        }
        let key = (span.start, span.end);
        if let Some(value) = self.cache.expressions.borrow().get(&key) {
            return value.clone();
        }
        let value = self.expression_type_uncached(span, depth);
        if self.cache.expressions.borrow().len() < MAX_ENTRIES {
            self.cache
                .expressions
                .borrow_mut()
                .insert(key, value.clone());
        }
        value
    }
}

fn initializer_is_chain(source: &str, span: &Range<usize>) -> bool {
    let Some(text) = source.get(span.clone()) else {
        return false;
    };
    let parsed = crate::syntax::parse_expression(text);
    if !parsed.diagnostics.is_empty() {
        return false;
    }
    matches!(
        parsed.nodes[parsed.root.0].kind,
        ExprKind::Call { .. } | ExprKind::Selector { .. } | ExprKind::TypeAssertion { .. }
    )
}

impl NativeTypeContext<'_> {
    fn expression_type_uncached(&self, span: &Range<usize>, depth: usize) -> Option<String> {
        let text = self.source.get(span.clone())?;
        let parsed = crate::syntax::parse_expression(text);
        if !parsed.diagnostics.is_empty() {
            return None;
        }
        let root = &parsed.nodes[parsed.root.0];
        match &root.kind {
            ExprKind::Name => {
                let name = text.get(root.span.clone())?;
                let binding = self.locals.resolve_at(span.start + root.span.start, name)?;
                if let Some(ty) = self.binding_type(binding) {
                    return Some(ty);
                }
                // Written annotations/declared types are already in consumer scope.
                if let Some(span) = binding.annotation() {
                    return self
                        .source
                        .get(span.clone())
                        .map(|text| text.trim().to_owned());
                }
                binding.declared_type.clone()
            }
            ExprKind::Index { operand, .. } => {
                let receiver = &parsed.nodes[operand.0];
                let ty = self.expression_type(&absolute(span.start, &receiver.span), depth + 1)?;
                index_element(self.workspace, self.file, &ty)
            }
            ExprKind::Slice { operand, .. } => {
                let receiver = &parsed.nodes[operand.0];
                self.expression_type(&absolute(span.start, &receiver.span), depth + 1)
            }
            ExprKind::Parenthesized { expression } => {
                let inner = &parsed.nodes[expression.0];
                self.expression_type(&absolute(span.start, &inner.span), depth + 1)
            }
            ExprKind::Unary { operator, operand } => {
                let inner = &parsed.nodes[operand.0];
                let ty = self.expression_type(&absolute(span.start, &inner.span), depth + 1)?;
                match text.get(operator.clone())? {
                    "&" => Some(format!("*{ty}")),
                    "*" => ty.strip_prefix('*').map(str::to_owned),
                    _ => None,
                }
            }
            ExprKind::Selector { operand, field } => {
                let receiver = &parsed.nodes[operand.0];
                let receiver_ty =
                    self.expression_type(&absolute(span.start, &receiver.span), depth + 1)?;
                let name = text.get(field.clone())?;
                match self
                    .workspace
                    .receiver_member(self.file, &receiver_ty, name)?
                {
                    ReceiverMember::Declaration(declaration) => {
                        let data = self.workspace.member_data(declaration)?;
                        self.workspace.requalified_type(
                            self.file,
                            declaration.file,
                            data.annotation?.trim(),
                        )
                    }
                    // A method selector as a value has the method's function type,
                    // which this projection intentionally does not represent.
                    ReceiverMember::Method(..) => None,
                }
            }
            ExprKind::TypeAssertion { asserted_type, .. } => {
                text.get(asserted_type.clone()).map(str::to_owned)
            }
            ExprKind::Call { function, .. } => {
                self.call_result_type(&parsed, text, span.start, *function, 0, depth + 1)
            }
            _ => None,
        }
    }
}

fn absolute(base: usize, span: &Range<usize>) -> Range<usize> {
    base + span.start..base + span.end
}

impl NativeTypeContext<'_> {
    fn call_result_type(
        &self,
        parsed: &crate::syntax::ExpressionTree,
        text: &str,
        base: usize,
        function: crate::syntax::ExprId,
        result: usize,
        depth: usize,
    ) -> Option<String> {
        if depth > MAX_DEPTH {
            return None;
        }
        let callee = &parsed.nodes[function.0];
        match &callee.kind {
            ExprKind::Name => {
                let name = text.get(callee.span.clone())?;
                if let Some(binding) = self.locals.resolve_at(base + callee.span.start, name) {
                    let span = binding.annotation()?;
                    let annotation = self.source.get(span.clone())?;
                    return signature_result(annotation, result);
                }
                let (path, symbol) = self.workspace.unqualified_symbol(self.file, name)?;
                self.declaration_result(path, symbol, result)
            }
            ExprKind::Selector { operand, field } => {
                let receiver = &parsed.nodes[operand.0];
                if matches!(receiver.kind, ExprKind::Name) {
                    let alias = text.get(receiver.span.clone())?;
                    if self
                        .locals
                        .resolve_at(base + receiver.span.start, alias)
                        .is_none()
                    {
                        let field = text.get(field.clone())?;
                        for (path, symbol) in self.workspace.imported_symbol_iter(self.file, alias)
                        {
                            if !symbol.is_function() || symbol.name != field {
                                continue;
                            }
                            return self.declaration_result(path, symbol, result);
                        }
                    }
                }
                let receiver_ty =
                    self.expression_type(&absolute(base, &receiver.span), depth + 1)?;
                let name = text.get(field.clone())?;
                match self
                    .workspace
                    .receiver_member(self.file, &receiver_ty, name)?
                {
                    ReceiverMember::Method(path, symbol) => {
                        self.declaration_result(path, symbol, result)
                    }
                    ReceiverMember::Declaration(declaration) => {
                        let data = self.workspace.member_data(declaration)?;
                        let annotation = data.annotation?;
                        let signature = function_type_text(&annotation);
                        let parsed = crate::syntax::parse_literal_signature(&signature);
                        if !parsed.diagnostics.is_empty() {
                            return None;
                        }
                        let result_type =
                            signature.get(parsed.results.get(result)?.annotation.clone())?;
                        self.workspace.requalified_type(
                            self.file,
                            declaration.file,
                            result_type.trim(),
                        )
                    }
                }
            }
            _ => None,
        }
    }

    fn declaration_result(&self, path: &Path, symbol: &Symbol, result: usize) -> Option<String> {
        let declaration = symbol.declaration()?;
        let end = symbol.body().map_or(declaration.end, |body| body.start);
        let header = self.workspace.source_slice(path, declaration.start..end)?;
        signature_result(&header, result).map(|annotation| {
            self.workspace
                .requalified_type(self.file, path, annotation.trim())
                .unwrap_or(annotation)
        })
    }
}

fn function_type_text(annotation: &str) -> String {
    if annotation.trim_start().starts_with("func") {
        annotation.to_owned()
    } else {
        format!("func{}", annotation.trim_start())
    }
}

fn signature_result(header: &str, result: usize) -> Option<String> {
    let parsed = crate::syntax::parse_signature(header);
    if !parsed.diagnostics.is_empty() {
        return None;
    }
    let annotation = header.get(parsed.results.get(result)?.annotation.clone())?;
    Some(annotation.trim().to_owned())
}

/// Resolve a selector's member identity from the native operand type. Used by
/// the highlighting resolver for expression-member occurrences without running
/// the legacy binding walk.
pub(crate) fn selector_member<'a>(
    workspace: &'a Workspace,
    file: &Path,
    source: &str,
    locals: &Locals,
    cache: &NativeTypeCache,
    span: &Range<usize>,
) -> Option<ReceiverMember<'a>> {
    let context = NativeTypeContext {
        workspace,
        file,
        source,
        locals,
        cache,
    };
    let text = source.get(span.clone())?;
    let parsed = crate::syntax::parse_expression(text);
    if !parsed.diagnostics.is_empty() {
        return None;
    }
    let root = &parsed.nodes[parsed.root.0];
    let ExprKind::Selector { operand, field } = &root.kind else {
        return None;
    };
    let receiver = &parsed.nodes[operand.0];
    let receiver_ty = context.expression_type(&absolute(span.start, &receiver.span), 0)?;
    let name = text.get(field.clone())?;
    workspace.receiver_member(file, &receiver_ty, name)
}

/// Element type of an index expression on a written container type.
fn index_element(workspace: &Workspace, file: &Path, container: &str) -> Option<String> {
    let text = container.trim();
    if let Some(rest) = text.strip_prefix("[]") {
        return Some(rest.trim().to_owned());
    }
    if let Some(rest) = text.strip_prefix('[').and_then(|text| text.split_once(']')) {
        return Some(rest.1.trim().to_owned());
    }
    if let Some(rest) = text
        .strip_prefix('*')
        .and_then(|text| text.strip_prefix('['))
    {
        return rest
            .split_once(']')
            .map(|(_, element)| element.trim().to_owned());
    }
    if let Some(rest) = text.strip_prefix("map[") {
        let close = matching_bracket(rest)?;
        return rest.get(close + 1..).map(|value| value.trim().to_owned());
    }
    if text.starts_with("struct")
        || text.starts_with("func")
        || text.starts_with("interface")
        || text == "string"
        || text.starts_with('*')
    {
        return None;
    }
    let underlying = workspace.instantiated_underlying_type(file, text)?;
    index_element(workspace, file, &underlying)
}

/// Project one range slot of a written container type. Named containers are
/// resolved through their rendered underlying declaration once.
fn range_element(
    workspace: &Workspace,
    file: &Path,
    container: &str,
    result: usize,
    count: usize,
) -> Option<String> {
    if count > 2 || result >= count {
        return None;
    }
    let text = container.trim();
    if let Some(rest) = text.strip_prefix("[]") {
        if count == 1 {
            return (result == 0).then(|| rest.trim().to_owned());
        }
        return (result == 1).then(|| rest.trim().to_owned());
    }
    if let Some(rest) = text.strip_prefix('[').and_then(|text| text.split_once(']')) {
        if count == 1 {
            return (result == 0).then(|| rest.1.trim().to_owned());
        }
        return (result == 1).then(|| rest.1.trim().to_owned());
    }
    if let Some(rest) = text.strip_prefix("map[") {
        let close = matching_bracket(rest)?;
        let key = rest.get(..close)?.trim();
        let value = rest.get(close + 1..)?.trim();
        return match (result, count) {
            (0, 1) => Some(key.to_owned()),
            (0, 2) => Some(key.to_owned()),
            (1, 2) => Some(value.to_owned()),
            _ => None,
        };
    }
    if let Some(rest) = text.strip_prefix("chan ") {
        return (result == 0).then(|| rest.trim().to_owned());
    }
    if let Some(rest) = text.strip_prefix("*[") {
        let element = rest.split_once(']')?.1.trim();
        return (result == 1).then(|| element.to_owned());
    }
    if text.starts_with("struct")
        || text.starts_with("func")
        || text.starts_with("interface")
        || text.starts_with('*')
        || text == "string"
    {
        return None;
    }
    let underlying = workspace.instantiated_underlying_type(file, text)?;
    if workspace.trace.queries {
        eprintln!("native range {text:?} underlying {underlying:?}");
    }
    range_element(workspace, file, &underlying, result, count)
}

fn matching_bracket(text: &str) -> Option<usize> {
    let mut depth = 1usize;
    for (index, byte) in text.bytes().enumerate() {
        match byte {
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}
