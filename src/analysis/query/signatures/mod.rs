//! Query-local checked callable signatures and source-name presentation.
mod builtin;
mod documentation;
pub(in crate::analysis::query) mod instantiated;
pub(in crate::analysis::query) mod label;
pub(in crate::analysis::query) use self::label::CallableSignature;
use super::Context;
use crate::syntax::{self, Expr, ExprId, ExprKind, Lexer, TokenKind};
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
    pub(crate) fn signature_help(
        &mut self,
        offset: usize,
        trigger: SignatureTrigger,
    ) -> Result<Option<SignatureHelp>, (i32, String)> {
        let workspace = self.workspace();
        let file = self.file();
        let body = self.cursor_body(offset)?;
        if let Some(body) = &body {
            return Ok(help_in_expressions(
                self,
                offset,
                trigger,
                (&body.tree.expressions, body.base),
            ));
        }
        // Outside a function, inspect only the containing value declaration.
        let mut init_range = None;
        {
            let source = self.source();
            let map = SourceMap::new(source);
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
                let Some(assignment) = Lexer::new(&source[symbol.end..end])
                    .find(|token| token.kind == TokenKind::Assign)
                else {
                    continue;
                };
                init_range = Some(symbol.end + assignment.span.end..end);
                break;
            }
        }
        if let Some(range) = init_range {
            let start = range.start;
            let parsed = syntax::parse_expression(&self.source()[range]);
            return Ok(help_in_expressions(
                self,
                offset,
                trigger,
                (&parsed.nodes, start),
            ));
        }
        Ok(None)
    }
}

fn help_in_expressions(
    context: &mut Context<'_>,
    offset: usize,
    trigger: SignatureTrigger,
    body: (&[Expr], usize),
) -> Option<SignatureHelp> {
    let source_len = context.source().len();
    let (expressions, base) = body;
    if !trigger.explicit
        && !trigger.retrigger
        && expressions.iter().any(|expression| {
            matches!(expression.kind, ExprKind::Literal(TokenKind::String))
                && base + expression.span.start <= offset
                && offset <= base + expression.span.end
        })
    {
        return None;
    }
    let cursor_offset = (offset == source_len).then_some(offset);
    let mut selected_information: Option<(ExprId, HelpInformation)> = None;
    let selected_fn = {
        let (doc, sema) = context.split();
        call_site(
            doc.source(),
            offset,
            trigger,
            expressions,
            base,
            |function| {
                let Some(information) = help_information(
                    doc,
                    sema,
                    body,
                    function,
                    direct_call(expressions, function),
                    cursor_offset,
                ) else {
                    return false;
                };
                if selected_information.as_ref().is_none_or(|(id, _)| {
                    expressions[function.0].span.len() < expressions[id.0].span.len()
                }) {
                    selected_information = Some((function, information));
                }
                true
            },
        )
    }?;
    let selected_information = selected_information.filter(|(id, _)| *id == selected_fn.function);
    let information = match selected_information {
        Some((_, information)) => information,
        None => {
            let (doc, sema) = context.split();
            help_information(
                doc,
                sema,
                body,
                selected_fn.function,
                selected_fn.expression,
                cursor_offset,
            )?
        }
    };
    let documentation = information.documentation.or_else(|| {
        documentation::for_function(context, body, selected_fn.function, cursor_offset)
    });
    let mut information = information.callable;
    if !matches!(
        expressions[selected_fn.function.0].kind,
        ExprKind::Name | ExprKind::Selector { .. }
    ) {
        let opening = information.0.label.find('(')?;
        information.0.label.replace_range(..opening, "func");
    }
    if selected_fn.in_arguments && !information.0.parameters.is_empty() {
        let mut active = selected_fn
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
    expressions
        .iter()
        .position(|expression| matches!(expression.kind, ExprKind::Call { function, .. } if function == selected))
        .map(ExprId)
}

fn help_information(
    doc: super::DocumentView<'_, '_>,
    sema: &mut crate::analysis::sema::SemanticSession<'_>,
    body: (&[Expr], usize),
    function: ExprId,
    call: Option<ExprId>,
    cursor_offset: Option<usize>,
) -> Option<HelpInformation> {
    let expression = body.0.get(function.0)?;
    let span = [
        u32::try_from(body.1 + expression.span.start).ok()?,
        u32::try_from(body.1 + expression.span.end).ok()?,
    ];
    let ident = match &expression.kind {
        ExprKind::Name => Some(&expression.span),
        ExprKind::Selector { field, .. } => Some(field),
        _ => None,
    };
    let name = ident
        .and_then(|span| doc.source().get(body.1 + span.start..body.1 + span.end))
        .unwrap_or("func");
    let ident = ident.and_then(|span| {
        Some([
            u32::try_from(body.1 + span.start).ok()?,
            u32::try_from(body.1 + span.end).ok()?,
        ])
    });
    let callable = sema.checked_callable(
        doc.file(),
        doc.source(),
        span,
        ident,
        cursor_offset,
        call.is_some(),
    )?;
    match callable {
        crate::analysis::sema::CheckedCallable::Builtin(function)
            if matches!(expression.kind, ExprKind::Name) =>
        {
            builtin::for_name(doc, function.name())
        }
        crate::analysis::sema::CheckedCallable::Builtin(_) => None,
        crate::analysis::sema::CheckedCallable::Signature(view) => {
            label::signature_from_native(name, &view).map(HelpInformation::from)
        }
    }
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

pub(in crate::analysis::query) fn local_binding_signature_with_sema(
    doc: super::DocumentView<'_, '_>,
    sema: &mut crate::analysis::sema::SemanticSession<'_>,
    selection: &std::ops::Range<usize>,
    scope_start: usize,
    display_name: &str,
) -> Option<(CallableSignature, bool)> {
    let span = [
        u32::try_from(selection.start).ok()?,
        u32::try_from(selection.end).ok()?,
    ];
    let view = sema.binding_callable(
        doc.file(),
        doc.source(),
        span,
        u32::try_from(scope_start).ok()?,
    )?;
    label::signature_from_native(display_name, &view)
}
