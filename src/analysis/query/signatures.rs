//! Query-local signature selection and expected callable argument semantics.
mod builtin;
mod documentation;
use super::Context;
use crate::analysis::check::callable_signature::CallableSignature;
use crate::analysis::scope::Locals;
use crate::syntax::{Expr, ExprId, ExprKind, Lexer, TokenKind};
use crate::text::SourceMap;

#[derive(Clone, Copy)]
pub(crate) struct SignatureTrigger {
    pub(crate) explicit: bool,
    pub(crate) retrigger: bool,
}

pub(crate) struct SignatureHelp {
    pub(crate) signature: CallableSignature,
    pub(crate) documentation: Option<String>,
}

struct HelpInformation {
    callable: (CallableSignature, bool),
    documentation: Option<String>,
}

impl From<(CallableSignature, bool)> for HelpInformation {
    fn from(callable: (CallableSignature, bool)) -> Self {
        Self {
            callable,
            documentation: None,
        }
    }
}

struct CallSite {
    function: ExprId,
    expression: Option<ExprId>,
    arguments: Vec<usize>,
    in_arguments: bool,
}

impl Context<'_> {
    pub(super) fn expects_callable_argument(
        &self,
        offset: usize,
        locals: &Locals,
        body: (&[Expr], usize),
    ) -> bool {
        let file = self.file();
        let source = self.source();
        let (expressions, base) = body;
        let trigger = SignatureTrigger {
            explicit: true,
            retrigger: false,
        };
        let Some(call) = call_site(source, offset, trigger, expressions, base, |_| false) else {
            return false;
        };
        if !call.in_arguments {
            return false;
        }
        let Some((information, variadic)) = self.callable_information(locals, body, call.function)
        else {
            return false;
        };
        let mut active = call
            .arguments
            .iter()
            .take_while(|end| offset > **end)
            .count();
        if variadic {
            active = active.min(information.parameters.len().saturating_sub(1));
        }
        let Some(parameter) = information.parameters.get(active) else {
            return false;
        };
        let header = format!("func({})", parameter.label);
        let parsed = crate::syntax::parse_literal_signature(&header);
        parsed.diagnostics.is_empty()
            && parsed.parameters.first().is_some_and(|parameter| {
                self.workspace
                    .callable_type(file, &header[parameter.annotation.clone()])
            })
    }
}

impl Context<'_> {
    pub(crate) fn signature_help(
        &self,
        offset: usize,
        trigger: SignatureTrigger,
    ) -> Result<Option<SignatureHelp>, (i32, String)> {
        let context = self;
        let workspace = context.workspace();
        let file = context.file();
        let source = context.source();
        let map = SourceMap::new(source);
        let scope = context.cursor_scope(offset)?;
        if let Some(body) = &scope.body {
            return Ok(help_in_expressions(
                context,
                offset,
                trigger,
                &scope.locals,
                (&body.tree.expressions, body.base),
            ));
        }
        // Outside a function, inspect only the containing value declaration.
        for symbol in &workspace.files[file].symbols {
            if !symbol.is_value() {
                continue;
            }
            let Some(end) = map.offset(symbol.range.end) else {
                continue;
            };
            if !(symbol.end..=end).contains(&offset) {
                continue;
            }
            let Some(assignment) =
                Lexer::new(&source[symbol.end..end]).find(|token| token.kind == TokenKind::Assign)
            else {
                continue;
            };
            let start = symbol.end + assignment.span.end;
            let parsed = crate::syntax::parse_expression(&source[start..end]);
            return Ok(help_in_expressions(
                context,
                offset,
                trigger,
                &scope.locals,
                (&parsed.nodes, start),
            ));
        }
        Ok(None)
    }
}

fn help_in_expressions(
    context: &Context<'_>,
    offset: usize,
    trigger: SignatureTrigger,
    locals: &Locals,
    body: (&[Expr], usize),
) -> Option<SignatureHelp> {
    let source = context.source();
    let (expressions, base) = body;
    if !trigger.explicit
        && !trigger.retrigger
        && expressions.iter().any(|expression| {
            matches!(expression.kind, ExprKind::Literal)
                && base + expression.span.start <= offset
                && offset <= base + expression.span.end
                && Lexer::new(&source[base + expression.span.start..base + expression.span.end])
                    .next()
                    .is_some_and(|token| token.kind == TokenKind::String)
        })
    {
        return None;
    }
    let mut selected_information: Option<(ExprId, HelpInformation)> = None;
    let call = call_site(source, offset, trigger, expressions, base, |function| {
        let Some(information) = help_information(
            context,
            locals,
            body,
            function,
            direct_call(expressions, function),
        ) else {
            return false;
        };
        // Keep the same smallest candidate as call_site. In particular, retain
        // a selected builtin's SDK declaration result rather than reading that
        // closed source again when constructing the final response.
        if selected_information
            .as_ref()
            .is_none_or(|(id, _)| expressions[function.0].span.len() < expressions[id.0].span.len())
        {
            selected_information = Some((function, information));
        }
        true
    })?;
    let selected_information = selected_information.filter(|(id, _)| *id == call.function);
    let information = selected_information
        .map(|(_, information)| information)
        .or_else(|| help_information(context, locals, body, call.function, call.expression))?;
    let documentation = information
        .documentation
        .or_else(|| documentation::for_function(context, locals, body, call.function));
    let mut information = information.callable;
    if !matches!(
        expressions[call.function.0].kind,
        ExprKind::Name | ExprKind::Selector { .. }
    ) {
        let opening = information.0.label.find('(')?;
        information.0.label.replace_range(..opening, "func");
    }
    if call.in_arguments && !information.0.parameters.is_empty() {
        let mut active = call
            .arguments
            .iter()
            .take_while(|end| offset > **end)
            .count();
        if information.1 {
            active = active.min(information.0.parameters.len() - 1);
        }
        information.0.active_parameter = Some(active as u32);
    }
    Some(SignatureHelp {
        signature: information.0,
        documentation,
    })
}

fn direct_call(expressions: &[Expr], selected: ExprId) -> Option<ExprId> {
    expressions.iter().position(|expression| {
        matches!(expression.kind, ExprKind::Call { function, .. } if function == selected)
    }).map(ExprId)
}

fn help_information(
    context: &Context<'_>,
    locals: &Locals,
    body: (&[Expr], usize),
    function: ExprId,
    call: Option<ExprId>,
) -> Option<HelpInformation> {
    if let Some(information) = builtin::for_function(context, locals, body, function) {
        return Some(information);
    }
    let (expressions, base) = body;
    let workspace = context.workspace();
    let inference = crate::analysis::check::Inference::new(
        workspace,
        context.file(),
        context.source(),
        locals,
        expressions,
        base,
    );
    if let Some((origin, symbol)) = inference.callee(function)
        && symbol.is_function()
        && symbol.generic
    {
        if let Some(call) = call {
            let ExprKind::Call {
                arguments,
                variadic,
                ..
            } = &expressions[call.0].kind
            else {
                return None;
            };
            return inference
                .instantiated_callee_signature(function, Some((arguments, *variadic)))
                .map(HelpInformation::from);
        }
        // Selecting the name within an explicit instance denotes the generic
        // declaration object, while its surrounding call denotes the instance.
        let declaration = symbol.declaration()?;
        let end = symbol.body().map_or(declaration.end, |body| body.start);
        if end.checked_sub(declaration.start)? > super::budget::MAX_BYTES {
            return None;
        }
        let header = workspace.source_slice(origin, declaration.start..end)?;
        let signature = crate::syntax::parse_signature(&header);
        return workspace
            .declared_signature(context.file(), origin, &header, &signature, &symbol.name)
            .map(HelpInformation::from);
    }
    context
        .callable_information(locals, body, function)
        .map(HelpInformation::from)
}

fn call_site(
    source: &str,
    offset: usize,
    trigger: SignatureTrigger,
    expressions: &[Expr],
    base: usize,
    mut callable: impl FnMut(ExprId) -> bool,
) -> Option<CallSite> {
    // Function-valued names under the cursor take precedence over an outer call.
    let selected = expressions
        .iter()
        .enumerate()
        .filter(|(index, expression)| {
            let span = match &expression.kind {
                ExprKind::Name => &expression.span,
                ExprKind::Selector { field, .. } => field,
                _ => return false,
            };
            base + span.start <= offset && offset <= base + span.end && callable(ExprId(*index))
        })
        .min_by_key(|(_, expression)| expression.span.len());
    if let Some((index, _)) = selected {
        return Some(CallSite {
            function: ExprId(index),
            expression: direct_call(expressions, ExprId(index)),
            arguments: Vec::new(),
            in_arguments: false,
        });
    }
    let mut best = None;
    let mut best_size = usize::MAX;
    for (index, expression) in expressions.iter().enumerate() {
        let ExprKind::Call {
            function,
            arguments,
            ..
        } = &expression.kind
        else {
            continue;
        };
        let start = base + expressions[function.0].span.end;
        let end = (base + expression.span.end).max(offset);
        if offset < start || expression.span.len() >= best_size {
            continue;
        }
        let mut tokens = Lexer::new(&source[start..end]);
        let Some(opening) = tokens
            .next()
            .filter(|token| token.kind == TokenKind::LParen)
        else {
            continue;
        };
        let mut depth = 1;
        let mut closing = None;
        let mut in_string = false;
        for token in tokens {
            in_string |= token.kind == TokenKind::String
                && start + token.span.start < offset
                && offset < start + token.span.end;
            match token.kind {
                TokenKind::LParen => depth += 1,
                TokenKind::RParen => {
                    depth -= 1;
                    if depth == 0 {
                        closing = Some(start + token.span.start);
                        break;
                    }
                }
                _ => {}
            }
        }
        if closing.is_some_and(|closing| offset > closing) {
            continue;
        }
        if !trigger.explicit && !trigger.retrigger && in_string {
            continue;
        }
        if expressions.iter().any(|child| {
            matches!(
                child.kind,
                ExprKind::Composite { .. } | ExprKind::FunctionLiteral { .. }
            ) && child.span.start > expression.span.start
                && base + child.span.start <= offset
                && offset < base + child.span.end
        }) {
            continue;
        }
        best_size = expression.span.len();
        best = Some(CallSite {
            function: *function,
            expression: Some(ExprId(index)),
            arguments: arguments
                .iter()
                .map(|argument| base + expressions[argument.0].span.end)
                .collect(),
            in_arguments: offset > start + opening.span.start,
        });
    }
    best
}

impl Context<'_> {
    /// Result type of a direct call to a known package-level function.
    pub(crate) fn direct_call_result_type(
        &self,
        expression_file: &std::path::Path,
        text: &str,
        slot: usize,
        count: usize,
        package_level: bool,
        invalid_import: impl FnOnce(&str) -> bool,
    ) -> Option<String> {
        let parsed = crate::syntax::parse_expression(text);
        if !parsed.diagnostics.is_empty() {
            return None;
        }
        let expression = parsed.nodes.get(parsed.root.0)?;
        let ExprKind::Call { function, .. } = &expression.kind else {
            return None;
        };
        let callee = parsed.nodes.get(function.0)?;
        let instantiated = matches!(callee.kind, ExprKind::Index { .. });
        let callee = if let ExprKind::Index { operand, .. } = &callee.kind {
            parsed.nodes.get(operand.0)?
        } else {
            callee
        };
        let workspace = self.workspace();
        let (origin, symbol) = match &callee.kind {
            ExprKind::Name => {
                let name = text.get(callee.span.clone())?;
                workspace.unqualified_symbol(expression_file, name)?
            }
            ExprKind::Selector { operand, field } => {
                let operand = parsed.nodes.get(operand.0)?;
                let ExprKind::Name = operand.kind else {
                    return None;
                };
                let alias = text.get(operand.span.clone())?;
                let field = text.get(field.clone())?;
                let mut found = workspace
                    .imported_symbol_iter(expression_file, alias)
                    .filter(|(_, symbol)| symbol.name == field);
                let Some(found) = found.next() else {
                    return invalid_import(alias).then(|| "invalid type".to_owned());
                };
                found
            }
            _ => return None,
        };
        if !symbol.is_function() || instantiated && !symbol.generic {
            return None;
        }
        if symbol.generic {
            // Without the caller's lexical scope, only a package-level
            // initializer can safely use an empty local environment.
            if !package_level {
                return None;
            }
            // Use the same typed argument unification and assignment proof as
            // document queries; do not manufacture types from generic spellings.
            // Inference owns the header read, avoiding a duplicate closed-file read.
            let locals = Locals::empty();
            let rendered = crate::analysis::check::Inference::new(
                workspace,
                expression_file,
                text,
                &locals,
                &parsed.nodes,
                0,
            )
            .expression(parsed.root, slot, count)?;
            return workspace.signature_type(self.file(), expression_file, &rendered);
        }
        let declaration = symbol.declaration()?;
        let end = symbol.body().map_or(declaration.end, |body| body.start);
        let header = workspace.source_slice(origin, declaration.start..end)?;
        if header.len() > super::budget::MAX_BYTES {
            return None;
        }
        let signature = crate::syntax::parse_signature(&header);
        if !signature.diagnostics.is_empty() || !signature.type_parameters.is_empty() {
            return None;
        }
        if count != signature.results.len() {
            return None;
        }
        let result = signature.results.get(slot)?;
        let annotation = header.get(result.annotation.clone())?;
        workspace.signature_type(self.file(), origin, annotation)
    }
}
