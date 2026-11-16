//! Query-local signature selection and expected callable argument semantics.
mod builtin;
mod documentation;
pub(in crate::analysis::query) mod instantiated;
pub(in crate::analysis::query) mod label;
use self::instantiated::format_instantiated_signature;
pub(in crate::analysis::query) use self::label::{
    CallableSignature, ParameterLabel, function_types, signature_label,
};
use super::Context;
use crate::analysis::budget::{MAX_BYTES, MAX_DEPTH};
use crate::analysis::check::RegionChecker;
use crate::analysis::check::decls::DeclCache;
use crate::analysis::check::region::RegionInfo;
use crate::analysis::obj::{DeclId, LocalId, MethodOrigin, ObjectRef, SymbolOrigin};
use crate::analysis::resolve::resolve_type_declaration;
use crate::analysis::scope::tree::{LocalBindingKind, ScopeTree};
use crate::analysis::types::{Qualifier, TypeArena, TypeId, declaration_body};
use crate::syntax::{self, DeclSpecKind, Expr, ExprId, ExprKind, Lexer, Signature, TokenKind};
use crate::text::SourceMap;
use crate::workspace::Workspace;
use std::borrow::Cow;
use std::path::Path;

const MAX_LABEL_BYTES: usize = 16 * 1024;

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
    pub(super) fn expects_callable_argument(&self, offset: usize, body: (&[Expr], usize)) -> bool {
        let source = self.source();
        let file = self.file();
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
        let active_raw = call
            .arguments
            .iter()
            .take_while(|end| offset > **end)
            .count();
        let cursor_offset = (offset == source.len()).then_some(offset);

        if let Some(expr) = expressions.get(call.function.0)
            && let (Ok(cs), Ok(ce)) = (
                u32::try_from(base + expr.span.start),
                u32::try_from(base + expr.span.end),
            )
            && let Some(decided) = self.with_sema(|sema| {
                let (region, arena) = sema.region_and_arena_at(file, source, cs, cursor_offset)?;
                let callee_span = [cs, ce];
                let call_info = region.call_by_callee(callee_span);
                if let Some(info) = call_info
                    && (info.is_conversion
                        || info.is_const_builtin
                        || matches!(info.callee_obj, Some(ObjectRef::BuiltinFunction(_))))
                {
                    return Some(false);
                }
                let sig_ty = call_info
                    .and_then(|c| c.signature)
                    .or_else(|| region.expr_type(callee_span).map(|op| op.ty))?;
                let (params, _, variadic) = arena.signature_parts(sig_ty)?;
                let active = if variadic {
                    active_raw.min(params.len().saturating_sub(1))
                } else {
                    active_raw
                };
                let &param_ty = params.get(active)?;
                Some(arena.is_callable(param_ty).unwrap_or(false))
            })
        {
            return decided;
        }

        let Some(info) =
            help_information(self, body, call.function, call.expression, cursor_offset)
        else {
            return false;
        };
        let (information, variadic) = info.callable;
        let mut active = active_raw;
        if variadic {
            active = active.min(information.parameters.len().saturating_sub(1));
        }
        let Some(parameter) = information.parameters.get(active) else {
            return false;
        };
        let header = format!("func({})", parameter.label);
        let parsed = syntax::parse_literal_signature(&header);
        parsed.diagnostics.is_empty()
            && parsed.parameters.first().is_some_and(|parameter| {
                self.with_sema(|sema| {
                    let arena = sema.arena_mut();
                    let Some(ty) = arena.parse(file, &header[parameter.annotation.clone()]) else {
                        return false;
                    };
                    arena.is_callable(ty).unwrap_or(false)
                })
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
            let parsed = syntax::parse_expression(&source[start..end]);
            return Ok(help_in_expressions(
                context,
                offset,
                trigger,
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
    let cursor_offset = (offset == source.len()).then_some(offset);
    let mut selected_information: Option<(ExprId, HelpInformation)> = None;
    let call = call_site(source, offset, trigger, expressions, base, |function| {
        let Some(information) = help_information(
            context,
            body,
            function,
            direct_call(expressions, function),
            cursor_offset,
        ) else {
            return false;
        };
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
        .or_else(|| {
            help_information(context, body, call.function, call.expression, cursor_offset)
        })?;
    let documentation = information
        .documentation
        .or_else(|| documentation::for_function(context, body, call.function, cursor_offset));
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
    expressions
        .iter()
        .position(|expression| matches!(expression.kind, ExprKind::Call { function, .. } if function == selected))
        .map(ExprId)
}

fn help_information(
    context: &Context<'_>,
    body: (&[Expr], usize),
    function: ExprId,
    call: Option<ExprId>,
    cursor_offset: Option<usize>,
) -> Option<HelpInformation> {
    help_information_depth(context, body, function, call, cursor_offset, 0)
}

fn help_information_depth(
    context: &Context<'_>,
    body: (&[Expr], usize),
    function: ExprId,
    call: Option<ExprId>,
    cursor_offset: Option<usize>,
    depth: usize,
) -> Option<HelpInformation> {
    if depth >= MAX_DEPTH {
        return None;
    }
    let expr = body.0.get(function.0)?;
    let source = context.source();
    let file = context.file();
    let workspace = context.workspace();
    let callee_span = [
        u32::try_from(body.1 + expr.span.start).ok()?,
        u32::try_from(body.1 + expr.span.end).ok()?,
    ];
    let ident_range = match &expr.kind {
        ExprKind::Name => Some(&expr.span),
        ExprKind::Selector { field, .. } => Some(field),
        _ => None,
    };
    let ident_span = ident_range.and_then(|r| {
        Some([
            u32::try_from(body.1 + r.start).ok()?,
            u32::try_from(body.1 + r.end).ok()?,
        ])
    });
    let callee_name = ident_range
        .and_then(|r| source.get(body.1 + r.start..body.1 + r.end))
        .unwrap_or("func");

    enum Outcome {
        None,
        Builtin(&'static str),
        Ready((CallableSignature, bool)),
        TryCallReturn {
            inner_fn: ExprId,
            fallback: Option<(CallableSignature, bool)>,
        },
    }

    let outcome = context.with_sema(|sema| {
        let Some((region, arena)) =
            sema.region_and_arena_at(file, source, callee_span[0], cursor_offset)
        else {
            return Outcome::None;
        };
        let call_info = call.and_then(|_| region.call_by_callee(callee_span));
        if call_info.is_some_and(|c| c.is_conversion || c.is_const_builtin) {
            return Outcome::None;
        }
        let sel = ident_span.and_then(|sp| region.selection_by_span(sp).cloned());
        let obj = call_info
            .and_then(|c| c.callee_obj)
            .or_else(|| ident_span.and_then(|sp| region.use_by_span(sp)))
            .or_else(|| sel.as_ref().map(|s| s.obj));

        match obj {
            Some(
                ObjectRef::TypeName { .. }
                | ObjectRef::BuiltinType(_)
                | ObjectRef::PkgName { .. }
                | ObjectRef::Nil
                | ObjectRef::Label { .. }
                | ObjectRef::BuiltinConst(_)
                | ObjectRef::TypeParam { .. },
            ) => Outcome::None,
            Some(ObjectRef::BuiltinFunction(bf)) => {
                if !matches!(expr.kind, ExprKind::Name) {
                    Outcome::None
                } else {
                    Outcome::Builtin(bf.name())
                }
            }
            Some(ObjectRef::Func(decl)) => {
                match callable_from_func_decl(
                    workspace,
                    arena,
                    file,
                    source,
                    decl,
                    callee_name,
                    call.is_some(),
                    call_info.map(|c| c.type_args.as_ref()),
                ) {
                    Some(sig) => Outcome::Ready(sig),
                    None => Outcome::None,
                }
            }
            Some(ObjectRef::Method(MethodOrigin::Concrete(decl))) => {
                match callable_from_concrete_method(
                    workspace,
                    arena,
                    file,
                    source,
                    decl,
                    callee_name,
                    sel.as_ref(),
                ) {
                    Some(sig) => Outcome::Ready(sig),
                    None => Outcome::None,
                }
            }
            Some(ObjectRef::Method(MethodOrigin::Interface(member))) => {
                if member.builtin {
                    return Outcome::Ready((
                        CallableSignature {
                            label: format!("{callee_name}() string"),
                            parameters: Vec::new(),
                            active_parameter: None,
                        },
                        false,
                    ));
                }
                match callable_from_interface_method(
                    workspace,
                    arena,
                    file,
                    source,
                    member,
                    callee_name,
                    sel.as_ref(),
                ) {
                    Some(sig) => Outcome::Ready(sig),
                    None => Outcome::None,
                }
            }
            Some(ObjectRef::Field(member)) => {
                let Some(sel_val) = sel.as_ref() else {
                    return Outcome::None;
                };
                let local_env = visible_local_env(region, source, callee_span[0]);
                match callable_from_field_member(
                    workspace,
                    arena,
                    file,
                    source,
                    member,
                    callee_name,
                    sel_val,
                    &local_env,
                ) {
                    Some(sig) => Outcome::Ready(sig),
                    None => Outcome::None,
                }
            }
            Some(ObjectRef::Var {
                origin: SymbolOrigin::Local { id, .. },
                ..
            }) => {
                let local_env = visible_local_env(region, source, callee_span[0]);
                if let Some(sig) = callable_from_local_var(
                    workspace,
                    sema,
                    file,
                    source,
                    id,
                    callee_span[0],
                    cursor_offset,
                    callee_name,
                    &local_env,
                ) {
                    Outcome::Ready(sig)
                } else {
                    Outcome::None
                }
            }
            Some(ObjectRef::Var {
                origin: SymbolOrigin::Package(decl),
                ..
            }) => {
                let sig =
                    callable_from_package_var(workspace, sema, file, source, decl, callee_name);
                match sig {
                    Some(sig) => Outcome::Ready(sig),
                    None => Outcome::None,
                }
            }
            _ => {
                let local_env = visible_local_env(region, source, callee_span[0]);
                if let ExprKind::FunctionLiteral { signature, .. } = &expr.kind
                    && let Some(sig_text) =
                        source.get(body.1 + signature.start..body.1 + signature.end)
                {
                    let parsed = syntax::parse_literal_signature(sig_text);
                    if let Some(sig) = render_sig_in_env(
                        workspace,
                        arena,
                        file,
                        file,
                        sig_text,
                        &parsed,
                        "func",
                        &local_env,
                        &[],
                    ) {
                        return Outcome::Ready(sig);
                    }
                }
                if let ExprKind::TypeAssertion { asserted_type, .. } = &expr.kind
                    && let Some(sig_text) =
                        source.get(body.1 + asserted_type.start..body.1 + asserted_type.end)
                    && let Some(ty) = call_info
                        .and_then(|c| c.signature)
                        .or_else(|| region.expr_type(callee_span).map(|op| op.ty))
                    && arena.is_callable(ty) == Some(true)
                {
                    let trimmed = trim_outer_parens(sig_text);
                    if starts_with_func_lparen(trimmed) {
                        let parsed = syntax::parse_literal_signature(trimmed);
                        if let Some(sig) = render_sig_in_env(
                            workspace,
                            arena,
                            file,
                            file,
                            trimmed,
                            &parsed,
                            "func",
                            &local_env,
                            &[],
                        ) {
                            return Outcome::Ready(sig);
                        }
                    }
                }
                let ty = call_info
                    .and_then(|c| c.signature)
                    .or_else(|| region.expr_type(callee_span).map(|op| op.ty));
                let fallback = ty.and_then(|ty| {
                    callable_from_type(workspace, arena, ty, file, source, callee_name, &local_env)
                });
                if let ExprKind::Call {
                    function: inner_fn, ..
                } = &expr.kind
                    && ty.is_some_and(|t| arena.is_callable(t) == Some(true))
                {
                    return Outcome::TryCallReturn {
                        inner_fn: *inner_fn,
                        fallback,
                    };
                }
                match fallback {
                    Some(sig) => Outcome::Ready(sig),
                    None => Outcome::None,
                }
            }
        }
    });

    match outcome {
        Outcome::None => None,
        Outcome::Builtin(name) => builtin::for_name(context, name),
        Outcome::Ready(sig) => Some(HelpInformation::from(sig)),
        Outcome::TryCallReturn { inner_fn, fallback } => {
            if let Some(inner_help) = help_information_depth(
                context,
                body,
                inner_fn,
                Some(function),
                cursor_offset,
                depth + 1,
            ) {
                let header = format!("func {}", inner_help.callable.0.label);
                let parsed = syntax::parse_signature(&header);
                if parsed.diagnostics.is_empty()
                    && parsed.results.len() == 1
                    && let Some(raw_ann) = header.get(parsed.results[0].annotation.clone())
                {
                    let res_ann = trim_outer_parens(raw_ann);
                    if starts_with_func_lparen(res_ann) {
                        let lit_sig = syntax::parse_literal_signature(res_ann);
                        if lit_sig.diagnostics.is_empty()
                            && let Some(sig) =
                                signature_label(res_ann, &lit_sig, "func", |a| Some(a.to_owned()))
                        {
                            return Some(HelpInformation::from(sig));
                        }
                    }
                }
            }
            fallback.map(HelpInformation::from)
        }
    }
}

fn read_source_slice<'s>(
    workspace: &Workspace,
    current_file: &Path,
    current_source: &'s str,
    target_file: &Path,
    range: std::ops::Range<usize>,
) -> Option<Cow<'s, str>> {
    if target_file == current_file {
        current_source.get(range).map(Cow::Borrowed)
    } else {
        workspace.source_slice(target_file, range).map(Cow::Owned)
    }
}

fn read_file_source<'s>(
    workspace: &Workspace,
    current_file: &Path,
    current_source: &'s str,
    target_file: &Path,
) -> Option<Cow<'s, str>> {
    if target_file == current_file {
        Some(Cow::Borrowed(current_source))
    } else {
        workspace.source(target_file).map(Cow::Owned)
    }
}

pub(super) fn visible_local_env<'s>(
    region: &RegionInfo<'_>,
    source: &'s str,
    pos: u32,
) -> Vec<(&'s str, TypeId)> {
    let mut env: Vec<(&'s str, TypeId)> = Vec::new();
    let mut current = Some(region.scopes.innermost_scope(pos));
    while let Some(scope_id) = current {
        let Some(node) = region.scopes.nodes.get(scope_id.0 as usize) else {
            break;
        };
        let mut binding_id = node.last_binding;
        while let Some(id) = binding_id {
            let binding = &region.scopes.bindings[id.0 as usize];
            if binding.visible_from <= pos
                && let Some(name) =
                    source.get(binding.name_span[0] as usize..binding.name_span[1] as usize)
                && matches!(
                    binding.kind,
                    LocalBindingKind::TypeDecl { .. } | LocalBindingKind::TypeParam
                )
                && !env.iter().any(|&(seen, _)| seen == name)
                && let Some(op) = region.local_type(id)
            {
                env.push((name, op.ty));
            }
            binding_id = binding.prev_in_scope;
        }
        current = node.parent;
    }
    env
}

#[allow(clippy::too_many_arguments)]
fn callable_from_func_decl<'a>(
    workspace: &'a Workspace,
    arena: &mut TypeArena<'a>,
    consumer: &Path,
    current_source: &str,
    decl: DeclId<'a>,
    display_name: &str,
    is_call: bool,
    type_args: Option<&[TypeId]>,
) -> Option<(CallableSignature, bool)> {
    if decl.symbol.generic {
        if is_call {
            let args = type_args.filter(|a| !a.is_empty())?;
            return format_instantiated_signature(workspace, consumer, arena, decl, args);
        }
        let span = decl.symbol.declaration()?;
        let end = decl.symbol.body().map_or(span.end, |b| b.start);
        if end.checked_sub(span.start)? > MAX_BYTES {
            return None;
        }
        let header = read_source_slice(
            workspace,
            consumer,
            current_source,
            decl.file,
            span.start..end,
        )?;
        let parsed = syntax::parse_signature(&header);
        if !parsed.diagnostics.is_empty() {
            return None;
        }
        let formals: Vec<&str> = parsed
            .type_parameters
            .iter()
            .map(|p| p.name.as_ref().and_then(|r| header.get(r.clone())))
            .collect::<Option<_>>()?;
        return render_sig_in_env(
            workspace,
            arena,
            consumer,
            decl.file,
            &header,
            &parsed,
            display_name,
            &[],
            &formals,
        );
    }

    let span = decl.symbol.declaration()?;
    let end = decl.symbol.body().map_or(span.end, |b| b.start);
    if end.checked_sub(span.start)? > MAX_LABEL_BYTES {
        return None;
    }
    let header = read_source_slice(
        workspace,
        consumer,
        current_source,
        decl.file,
        span.start..end,
    )?;
    let parsed = syntax::parse_signature(&header);
    if !parsed.diagnostics.is_empty() {
        return None;
    }
    render_sig_in_env(
        workspace,
        arena,
        consumer,
        decl.file,
        &header,
        &parsed,
        display_name,
        &[],
        &[],
    )
}

fn callable_from_concrete_method<'a>(
    workspace: &'a Workspace,
    arena: &mut TypeArena<'a>,
    consumer: &Path,
    current_source: &str,
    decl: DeclId<'a>,
    display_name: &str,
    sel: Option<&crate::analysis::types::Selection<'a>>,
) -> Option<(CallableSignature, bool)> {
    let span = decl.symbol.declaration()?;
    let end = decl.symbol.body().map_or(span.end, |b| b.start);
    if end.checked_sub(span.start)? > MAX_LABEL_BYTES {
        return None;
    }
    let header = read_source_slice(
        workspace,
        consumer,
        current_source,
        decl.file,
        span.start..end,
    )?;
    let parsed = syntax::parse_signature(&header);
    if !parsed.diagnostics.is_empty() || !parsed.type_parameters.is_empty() {
        return None;
    }
    let mut env = Vec::new();
    if let Some(recv) = parsed.receiver.first() {
        let names = crate::analysis::scope::receiver_parameters(&header, &recv.annotation);
        if !names.is_empty() {
            let sel_val = sel?;
            let owner_name = decl.symbol.receiver.as_deref()?;
            let (owner_file, owner_sym) =
                resolve_type_declaration(workspace, decl.file, owner_name)?;
            let args = arena.receiver_type_arguments(
                sel_val.recv,
                sel_val.indices.as_slice(),
                owner_file.as_path(),
                owner_sym.start,
            )?;
            if args.len() != names.len() {
                return None;
            }
            for (name_span, ty) in names.into_iter().zip(args) {
                env.push((header.get(name_span)?, ty));
            }
        }
    }
    if let Some(sig) = render_sig_in_env(
        workspace,
        arena,
        consumer,
        decl.file,
        &header,
        &parsed,
        display_name,
        &env,
        &[],
    ) {
        return Some(sig);
    }
    sel.and_then(|s| {
        callable_from_type(
            workspace,
            arena,
            s.ty,
            consumer,
            current_source,
            display_name,
            &[],
        )
    })
}

fn resolve_member_owner<'a>(
    workspace: &'a Workspace,
    member: crate::analysis::obj::MemberDeclId<'a>,
) -> Option<&'a crate::index::IndexedSymbol> {
    let [start, end] = member.selection;
    let (start, end) = (start as usize, end as usize);
    if let Some(owner) = member.owner
        && owner
            .declaration()
            .is_some_and(|decl| decl.start <= start && end <= decl.end)
    {
        return Some(owner);
    }
    let stub = workspace.files.get(member.file)?;
    stub.symbols.iter().find(|sym| {
        sym.is_type()
            && sym
                .declaration()
                .is_some_and(|decl| decl.start <= start && end <= decl.end)
    })
}

fn callable_from_interface_method<'a>(
    workspace: &'a Workspace,
    arena: &mut TypeArena<'a>,
    consumer: &Path,
    current_source: &str,
    member: crate::analysis::obj::MemberDeclId<'a>,
    display_name: &str,
    sel: Option<&crate::analysis::types::Selection<'a>>,
) -> Option<(CallableSignature, bool)> {
    if let Some(owner) = resolve_member_owner(workspace, member) {
        let origin_source = read_file_source(workspace, consumer, current_source, member.file)?;
        let map = SourceMap::new(&origin_source);
        let owner_base = owner.end;
        if let Some(decl_end) = map.offset(owner.range.end)
            && let Some(decl_body) = origin_source.get(owner_base..decl_end)
            && decl_body.len() <= MAX_LABEL_BYTES
        {
            let entries = syntax::aggregate_members(decl_body);
            if let Some(entry) = entries
                .iter()
                .find(|e| owner_base + e.selection.start == member.selection[0] as usize)
                && let Some(ann) = decl_body.get(entry.annotation.clone())
            {
                let parsed = syntax::parse_method_signature(ann);
                if parsed.diagnostics.is_empty() {
                    let tp_names = owner_type_param_names(
                        workspace,
                        consumer,
                        current_source,
                        member.file,
                        owner,
                    );
                    let env = if tp_names.is_empty() {
                        Some(Vec::new())
                    } else if let Some(sel_val) = sel
                        && let Some(args) = arena.receiver_type_arguments(
                            sel_val.recv,
                            sel_val.indices.as_slice(),
                            member.file,
                            owner.start,
                        )
                        && args.len() == tp_names.len()
                    {
                        Some(tp_names.iter().map(String::as_str).zip(args).collect())
                    } else if sel.is_none() {
                        Some(Vec::new())
                    } else {
                        None
                    };
                    if let Some(env) = env {
                        if let Some(sig) = render_sig_in_env(
                            workspace,
                            arena,
                            consumer,
                            member.file,
                            ann,
                            &parsed,
                            display_name,
                            &env,
                            &[],
                        ) {
                            return Some(sig);
                        }
                    } else if let Some(sel_val) = sel
                        && let Some(sig) = callable_from_signature_with_names(
                            arena,
                            sel_val.ty,
                            consumer,
                            display_name,
                            ann,
                            &parsed,
                        )
                    {
                        return Some(sig);
                    }
                }
            }
        }
    }
    sel.and_then(|s| {
        callable_from_type(
            workspace,
            arena,
            s.ty,
            consumer,
            current_source,
            display_name,
            &[],
        )
    })
}

#[allow(clippy::too_many_arguments)]
fn callable_from_field_member<'a>(
    workspace: &'a Workspace,
    arena: &mut TypeArena<'a>,
    consumer: &Path,
    current_source: &str,
    member: crate::analysis::obj::MemberDeclId<'a>,
    display_name: &str,
    sel: &crate::analysis::types::Selection<'a>,
    local_env: &[(&str, TypeId)],
) -> Option<(CallableSignature, bool)> {
    if let Some(owner) = resolve_member_owner(workspace, member) {
        let origin_source = read_file_source(workspace, consumer, current_source, member.file)?;
        let map = SourceMap::new(&origin_source);
        let owner_base = owner.end;
        if let Some(decl_end) = map.offset(owner.range.end)
            && let Some(decl_body) = origin_source.get(owner_base..decl_end)
            && decl_body.len() <= MAX_LABEL_BYTES
        {
            let entries = syntax::aggregate_members(decl_body);
            if let Some(entry) = entries
                .iter()
                .find(|e| owner_base + e.selection.start == member.selection[0] as usize)
                && let Some(raw_ann) = decl_body.get(entry.annotation.clone())
            {
                let trimmed = trim_outer_parens(raw_ann);
                if starts_with_func_lparen(trimmed) {
                    let parsed = syntax::parse_literal_signature(trimmed);
                    if parsed.diagnostics.is_empty() {
                        let tp_names = owner_type_param_names(
                            workspace,
                            consumer,
                            current_source,
                            member.file,
                            owner,
                        );
                        let env = if tp_names.is_empty() {
                            Some(Vec::new())
                        } else if let Some(args) = arena.receiver_type_arguments(
                            sel.recv,
                            sel.indices.as_slice(),
                            member.file,
                            owner.start,
                        ) && args.len() == tp_names.len()
                        {
                            Some(tp_names.iter().map(String::as_str).zip(args).collect())
                        } else {
                            None
                        };
                        if let Some(env) = env {
                            if let Some(sig) = render_sig_in_env(
                                workspace,
                                arena,
                                consumer,
                                member.file,
                                trimmed,
                                &parsed,
                                display_name,
                                &env,
                                &[],
                            ) {
                                return Some(sig);
                            }
                        } else if let Some(sig) = callable_from_signature_with_names(
                            arena,
                            sel.ty,
                            consumer,
                            display_name,
                            trimmed,
                            &parsed,
                        ) {
                            return Some(sig);
                        }
                    }
                }
            }
        }
    }
    callable_from_type(
        workspace,
        arena,
        sel.ty,
        consumer,
        current_source,
        display_name,
        local_env,
    )
}

fn owner_type_param_names(
    workspace: &Workspace,
    consumer: &Path,
    current_source: &str,
    owner_file: &Path,
    owner: &crate::index::IndexedSymbol,
) -> Vec<String> {
    if !owner.generic {
        return Vec::new();
    }
    let Some(decl_range) = owner.declaration() else {
        return Vec::new();
    };
    let end = decl_range
        .end
        .min(owner.end.saturating_add(MAX_LABEL_BYTES));
    if owner.end >= end {
        return Vec::new();
    }
    let Some(header) = read_source_slice(
        workspace,
        consumer,
        current_source,
        owner_file,
        owner.end..end,
    ) else {
        return Vec::new();
    };
    crate::analysis::scope::type_parameters(header.trim_start()).unwrap_or_default()
}

#[allow(clippy::too_many_arguments)]
fn callable_from_local_var<'a>(
    workspace: &'a Workspace,
    sema: &mut crate::analysis::sema::Semantics<'a>,
    consumer: &Path,
    source: &str,
    start_id: LocalId,
    query_pos: u32,
    cursor_offset: Option<usize>,
    display_name: &str,
    local_env: &[(&str, TypeId)],
) -> Option<(CallableSignature, bool)> {
    let mut current = start_id;

    for _ in 0..16 {
        enum Hop<'a> {
            NextLocal(LocalId),
            FollowFunc(DeclId<'a>, Option<Vec<TypeId>>),
            FollowMethod(
                MethodOrigin<'a>,
                Option<crate::analysis::types::Selection<'a>>,
            ),
            FollowField(
                crate::analysis::obj::MemberDeclId<'a>,
                crate::analysis::types::Selection<'a>,
            ),
            Done,
        }

        let hop = {
            let (region, arena) =
                sema.region_and_arena_at(consumer, source, query_pos, cursor_offset)?;
            if let Some(ann_span) = region.local_annotation(current)
                && let Some(raw_ann) = source.get(ann_span[0] as usize..ann_span[1] as usize)
            {
                let trimmed = trim_outer_parens(raw_ann);
                if starts_with_func_lparen(trimmed) {
                    let parsed = syntax::parse_literal_signature(trimmed);
                    if parsed.diagnostics.is_empty()
                        && let Some(sig) = render_sig_in_env(
                            workspace,
                            arena,
                            consumer,
                            consumer,
                            trimmed,
                            &parsed,
                            display_name,
                            local_env,
                            &[],
                        )
                    {
                        return Some(sig);
                    }
                }
            }
            if let Some(init_span) = region.local_init(current)
                && let Some(init_text) = source.get(init_span[0] as usize..init_span[1] as usize)
            {
                let trimmed_span = trim_outer_paren_span(source, init_span);
                let trimmed = source
                    .get(trimmed_span[0] as usize..trimmed_span[1] as usize)
                    .unwrap_or(init_text);
                if starts_with_func_lparen(trimmed) {
                    let parsed_expr = syntax::parse_expression(trimmed);
                    if let Some(expr) = parsed_expr.nodes.get(parsed_expr.root.0)
                        && let ExprKind::FunctionLiteral { signature, .. } = &expr.kind
                        && let Some(sig_text) = trimmed.get(signature.clone())
                    {
                        let parsed = syntax::parse_literal_signature(sig_text);
                        if parsed.diagnostics.is_empty()
                            && let Some(sig) = render_sig_in_env(
                                workspace,
                                arena,
                                consumer,
                                consumer,
                                sig_text,
                                &parsed,
                                display_name,
                                local_env,
                                &[],
                            )
                        {
                            return Some(sig);
                        }
                    }
                }
                if let Some(ident_span) = callee_ident_in_expr_span(source, trimmed_span) {
                    let target_obj = region
                        .use_by_span(ident_span)
                        .or_else(|| region.selection_by_span(ident_span).map(|s| s.obj));
                    match target_obj {
                        Some(ObjectRef::Var {
                            origin: SymbolOrigin::Local { id: next_id, .. },
                            ..
                        }) if next_id != current && trimmed_span == ident_span => {
                            Hop::NextLocal(next_id)
                        }
                        Some(ObjectRef::Func(decl)) => {
                            let inst_args = region
                                .instance_by_span(ident_span)
                                .map(|inst| inst.type_args.to_vec());
                            Hop::FollowFunc(decl, inst_args)
                        }
                        Some(ObjectRef::Method(method)) => {
                            Hop::FollowMethod(method, region.selection_by_span(ident_span).cloned())
                        }
                        Some(ObjectRef::Field(member)) => {
                            if let Some(sel) = region.selection_by_span(ident_span).cloned() {
                                Hop::FollowField(member, sel)
                            } else {
                                Hop::Done
                            }
                        }
                        _ => Hop::Done,
                    }
                } else {
                    Hop::Done
                }
            } else {
                Hop::Done
            }
        };

        match hop {
            Hop::NextLocal(next_id) => current = next_id,
            Hop::FollowFunc(decl, inst_args) => {
                let (_, arena) =
                    sema.region_and_arena_at(consumer, source, query_pos, cursor_offset)?;
                if let Some(mut sig) = callable_from_func_decl(
                    workspace,
                    arena,
                    consumer,
                    source,
                    decl,
                    display_name,
                    inst_args.is_some(),
                    inst_args.as_deref(),
                ) {
                    if let Some(opening) = sig.0.label.find('(') {
                        sig.0.label.replace_range(..opening, display_name);
                    }
                    return Some(sig);
                }
                break;
            }
            Hop::FollowMethod(MethodOrigin::Concrete(decl), sel) => {
                let (_, arena) =
                    sema.region_and_arena_at(consumer, source, query_pos, cursor_offset)?;
                if let Some(sig) = callable_from_concrete_method(
                    workspace,
                    arena,
                    consumer,
                    source,
                    decl,
                    display_name,
                    sel.as_ref(),
                ) {
                    return Some(sig);
                }
                break;
            }
            Hop::FollowMethod(MethodOrigin::Interface(member), sel) => {
                if member.builtin {
                    return Some((
                        CallableSignature {
                            label: format!("{display_name}() string"),
                            parameters: Vec::new(),
                            active_parameter: None,
                        },
                        false,
                    ));
                }
                let (_, arena) =
                    sema.region_and_arena_at(consumer, source, query_pos, cursor_offset)?;
                if let Some(sig) = callable_from_interface_method(
                    workspace,
                    arena,
                    consumer,
                    source,
                    member,
                    display_name,
                    sel.as_ref(),
                ) {
                    return Some(sig);
                }
                break;
            }
            Hop::FollowField(member, sel) => {
                let (_, arena) =
                    sema.region_and_arena_at(consumer, source, query_pos, cursor_offset)?;
                if let Some(sig) = callable_from_field_member(
                    workspace,
                    arena,
                    consumer,
                    source,
                    member,
                    display_name,
                    &sel,
                    local_env,
                ) {
                    return Some(sig);
                }
                break;
            }
            Hop::Done => break,
        }
    }

    let (region, arena) = sema.region_and_arena_at(consumer, source, query_pos, cursor_offset)?;
    let op = region.local_type(start_id)?;
    callable_from_type(
        workspace,
        arena,
        op.ty,
        consumer,
        source,
        display_name,
        local_env,
    )
}

fn callable_from_package_var<'a>(
    workspace: &'a Workspace,
    sema: &mut crate::analysis::sema::Semantics<'a>,
    consumer: &Path,
    current_source: &str,
    decl: DeclId<'a>,
    display_name: &str,
) -> Option<(CallableSignature, bool)> {
    if let Some(spec_range) = decl.symbol.declaration()
        && spec_range.len() <= MAX_LABEL_BYTES
        && let Some(spec_slice) = read_source_slice(
            workspace,
            consumer,
            current_source,
            decl.file,
            spec_range.clone(),
        )
    {
        let kind = if decl.symbol.is_const() {
            DeclSpecKind::Const
        } else {
            DeclSpecKind::Var
        };
        let tree = syntax::parse_decl_spec(&spec_slice, kind);
        if let Some((ann_range, values)) = tree.statement(tree.root()).and_then(|s| match &s.kind {
            syntax::StmtKind::Declaration {
                annotation, values, ..
            } => Some((annotation.clone(), values.clone())),
            _ => None,
        }) {
            if let Some(ref ann) = ann_range
                && let Some(raw_ann) = spec_slice.get(ann.clone())
            {
                let trimmed = trim_outer_parens(raw_ann);
                if starts_with_func_lparen(trimmed) {
                    let parsed = syntax::parse_literal_signature(trimmed);
                    if parsed.diagnostics.is_empty()
                        && let Some(sig) = render_sig_in_env(
                            workspace,
                            sema.arena_mut(),
                            consumer,
                            decl.file,
                            trimmed,
                            &parsed,
                            display_name,
                            &[],
                            &[],
                        )
                    {
                        return Some(sig);
                    }
                }
            }
            if ann_range.is_none()
                && values.len() == 1
                && let Some(rhs_expr) = tree.expression(values[0])
            {
                if let ExprKind::FunctionLiteral { ref signature, .. } = rhs_expr.kind
                    && let Some(sig_text) = spec_slice.get(signature.clone())
                {
                    let parsed = syntax::parse_literal_signature(sig_text);
                    if parsed.diagnostics.is_empty()
                        && let Some(sig) = render_sig_in_env(
                            workspace,
                            sema.arena_mut(),
                            consumer,
                            decl.file,
                            sig_text,
                            &parsed,
                            display_name,
                            &[],
                            &[],
                        )
                    {
                        return Some(sig);
                    }
                } else if let (Ok(rs), Ok(re), Some((decl_file_interned, _))) = (
                    u32::try_from(rhs_expr.span.start),
                    u32::try_from(rhs_expr.span.end),
                    workspace.files.get_key_value(decl.file),
                ) && let Some(ident_span) =
                    callee_ident_in_expr_span(&spec_slice, [rs, re])
                    && let Ok(spec_end) = u32::try_from(spec_slice.len())
                {
                    let scopes = ScopeTree::from_top_level_decl(&spec_slice, 0, &tree, None);
                    let mut spec_region = RegionInfo::new(None, [0, spec_end], scopes);
                    let mut temp_arena = TypeArena::with_shared_budget(workspace);
                    let mut temp_decls = DeclCache::new();
                    {
                        let mut checker = RegionChecker::new(
                            &mut temp_arena,
                            &mut temp_decls,
                            decl_file_interned.as_path(),
                            &spec_slice,
                            0,
                            &tree,
                            &mut spec_region,
                            decl.symbol.is_const().then_some(0),
                            None,
                        );
                        checker.check_decl_spec(decl.symbol.is_const());
                    }
                    spec_region.finalize();
                    if let Some(ObjectRef::Func(target_decl)) = spec_region.use_by_span(ident_span)
                    {
                        let inst_args = spec_region
                            .instance_by_span(ident_span)
                            .map(|i| i.type_args.to_vec());
                        if let Some(mut sig) = callable_from_func_decl(
                            workspace,
                            &mut temp_arena,
                            consumer,
                            current_source,
                            target_decl,
                            display_name,
                            inst_args.is_some(),
                            inst_args.as_deref(),
                        ) {
                            if let Some(opening) = sig.0.label.find('(') {
                                sig.0.label.replace_range(..opening, display_name);
                            }
                            return Some(sig);
                        }
                    }
                }
            }
        }
    }
    let op = sema.operand_of_decl(decl)?;
    callable_from_type(
        workspace,
        sema.arena_mut(),
        op.ty,
        consumer,
        current_source,
        display_name,
        &[],
    )
}

fn callable_from_type<'a>(
    workspace: &'a Workspace,
    arena: &mut TypeArena<'a>,
    ty: TypeId,
    consumer: &Path,
    current_source: &str,
    display_name: &str,
    local_env: &[(&str, TypeId)],
) -> Option<(CallableSignature, bool)> {
    let unaliased = arena.unalias(ty)?;
    if let Some((decl, args, false)) = arena.named_declaration(unaliased)
        && let Some(spec_range) = decl.symbol.declaration()
        && spec_range.end.saturating_sub(decl.symbol.end) <= MAX_LABEL_BYTES
        && let Some(decl_slice) = read_source_slice(
            workspace,
            consumer,
            current_source,
            decl.file,
            decl.symbol.end..spec_range.end,
        )
        && let Some(raw_body) = declaration_body(&decl_slice)
    {
        let trimmed = trim_outer_parens(raw_body);
        if trimmed.len() <= MAX_LABEL_BYTES && starts_with_func_lparen(trimmed) {
            let parsed = syntax::parse_literal_signature(trimmed);
            if parsed.diagnostics.is_empty() {
                let tp_names = owner_type_param_names(
                    workspace,
                    consumer,
                    current_source,
                    decl.file,
                    decl.symbol,
                );
                if tp_names.len() == args.len() {
                    let env: Vec<(&str, TypeId)> =
                        tp_names.iter().map(String::as_str).zip(args).collect();
                    if let Some(sig) = render_sig_in_env(
                        workspace,
                        arena,
                        consumer,
                        decl.file,
                        trimmed,
                        &parsed,
                        display_name,
                        &env,
                        &[],
                    ) {
                        return Some(sig);
                    }
                }
            }
        }
    }
    if let Some((origin, raw_ann)) = arena
        .local_type_annotation(unaliased)
        .map(|(o, a)| (o, a.to_owned()))
    {
        let trimmed = trim_outer_parens(&raw_ann);
        if trimmed.len() <= MAX_LABEL_BYTES && starts_with_func_lparen(trimmed) {
            let parsed = syntax::parse_literal_signature(trimmed);
            if parsed.diagnostics.is_empty()
                && let Some(sig) = render_sig_in_env(
                    workspace,
                    arena,
                    consumer,
                    origin,
                    trimmed,
                    &parsed,
                    display_name,
                    local_env,
                    &[],
                )
            {
                return Some(sig);
            }
        }
    }
    let (params, results, variadic) = arena.signature_parts(unaliased)?;
    callable_from_signature_parts(
        arena,
        consumer,
        display_name,
        &params,
        &results,
        variadic,
        &[],
        &[],
    )
}

fn callable_from_signature_with_names(
    arena: &mut TypeArena<'_>,
    sig_ty: TypeId,
    consumer: &Path,
    display_name: &str,
    source: &str,
    parsed: &Signature,
) -> Option<(CallableSignature, bool)> {
    let unaliased = arena.unalias(sig_ty).unwrap_or(sig_ty);
    let (params, results, variadic) = arena.signature_parts(unaliased)?;
    if params.len() != parsed.parameters.len() || results.len() != parsed.results.len() {
        return None;
    }
    let param_names: Vec<Option<&str>> = parsed
        .parameters
        .iter()
        .map(|p| p.name.as_ref().and_then(|sp| source.get(sp.clone())))
        .collect();
    let result_names: Vec<Option<&str>> = parsed
        .results
        .iter()
        .map(|r| r.name.as_ref().and_then(|sp| source.get(sp.clone())))
        .collect();
    callable_from_signature_parts(
        arena,
        consumer,
        display_name,
        &params,
        &results,
        variadic,
        &param_names,
        &result_names,
    )
}

#[allow(clippy::too_many_arguments)]
fn callable_from_signature_parts(
    arena: &TypeArena<'_>,
    consumer: &Path,
    display_name: &str,
    params: &[TypeId],
    results: &[TypeId],
    variadic: bool,
    param_names: &[Option<&str>],
    result_names: &[Option<&str>],
) -> Option<(CallableSignature, bool)> {
    let mut raw_len = display_name.len().checked_add(8)?;
    let mut param_labels = Vec::with_capacity(params.len());
    for (idx, &p_ty) in params.iter().enumerate() {
        let mut formatted = arena.format_type(p_ty, Qualifier::Presentation(consumer))?;
        if let Some(trimmed) = function_types(&formatted) {
            formatted = trimmed;
        }
        if variadic
            && idx + 1 == params.len()
            && let Some(elem) = formatted.strip_prefix("[]")
        {
            formatted = format!("...{elem}");
        }
        if let Some(pname) = param_names.get(idx).copied().flatten() {
            formatted = format!("{pname} {formatted}");
        }
        raw_len = raw_len.checked_add(formatted.len() + 2)?;
        if raw_len > MAX_LABEL_BYTES {
            return None;
        }
        param_labels.push(ParameterLabel { label: formatted });
    }
    let mut has_named_result = false;
    let mut result_labels = Vec::with_capacity(results.len());
    for (idx, &r_ty) in results.iter().enumerate() {
        let mut formatted = arena.format_type(r_ty, Qualifier::Presentation(consumer))?;
        if let Some(trimmed) = function_types(&formatted) {
            formatted = trimmed;
        }
        if let Some(rname) = result_names.get(idx).copied().flatten() {
            has_named_result = true;
            formatted = format!("{rname} {formatted}");
        }
        raw_len = raw_len.checked_add(formatted.len() + 2)?;
        if raw_len > MAX_LABEL_BYTES {
            return None;
        }
        result_labels.push(formatted);
    }
    let mut label = format!(
        "{}({})",
        display_name,
        param_labels
            .iter()
            .map(|p| p.label.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    if !result_labels.is_empty() {
        label.push(' ');
        let parens = result_labels.len() > 1 || has_named_result;
        if parens {
            label.push('(');
        }
        label.push_str(&result_labels.join(", "));
        if parens {
            label.push(')');
        }
    }
    if label.len() > MAX_LABEL_BYTES {
        return None;
    }
    Some((
        CallableSignature {
            label,
            parameters: param_labels,
            active_parameter: None,
        },
        variadic,
    ))
}

#[allow(clippy::too_many_arguments)]
fn render_sig_in_env(
    workspace: &Workspace,
    arena: &mut TypeArena<'_>,
    consumer: &Path,
    origin: &Path,
    header: &str,
    signature: &Signature,
    display_name: &str,
    env: &[(&str, TypeId)],
    formals: &[&str],
) -> Option<(CallableSignature, bool)> {
    signature_label(header, signature, display_name, |ann| {
        render_annotation_in_env(workspace, arena, consumer, origin, ann, env, formals)
    })
}

fn render_annotation_in_env(
    workspace: &Workspace,
    arena: &mut TypeArena<'_>,
    consumer: &Path,
    origin: &Path,
    source: &str,
    env: &[(&str, TypeId)],
    formals: &[&str],
) -> Option<String> {
    if source.len() > MAX_LABEL_BYTES {
        return None;
    }
    if formals.is_empty()
        && !Lexer::new(source).any(|t| matches!(t.kind, TokenKind::Func | TokenKind::Interface))
        && let Some(native) = arena.parse_in(origin, source, env)
        && let Some(formatted) = arena.format_type(native, Qualifier::Presentation(consumer))
    {
        return Some(formatted);
    }
    let syntax = syntax::parse_type(source);
    if !syntax.diagnostics().is_empty()
        || syntax.references.len() + syntax.type_nodes().len() > crate::analysis::budget::MAX_NODES
    {
        return None;
    }
    let mut replacements = Vec::new();
    let mut bytes = 0usize;
    for reference in &syntax.references {
        let start = reference
            .qualifier
            .as_ref()
            .map_or(reference.name.start, |q| q.start);
        let span = start..reference.name.end;
        let name = source.get(reference.name.clone())?;
        if reference.qualifier.is_none() && formals.contains(&name) {
            continue;
        }
        let mut replacement = if let Some((_, actual)) = reference
            .qualifier
            .is_none()
            .then(|| env.iter().find(|(k, _)| *k == name))
            .flatten()
        {
            let formatted = arena.format_type(*actual, Qualifier::Presentation(consumer))?;
            if Lexer::new(&formatted)
                .next()
                .is_some_and(|token| token.kind == TokenKind::Arrow)
                && syntax.type_nodes().iter().any(|node| {
                    node.kind == syntax::TypeNodeKind::Chan && node.span.end == span.end
                })
            {
                format!("({formatted})")
            } else {
                formatted
            }
        } else {
            let ref_spelling = source.get(span.clone())?;
            if let Some((path, symbol)) = resolve_type_declaration(workspace, origin, ref_spelling)
            {
                qualify_symbol_for_consumer(workspace, consumer, path, symbol)?
            } else {
                continue;
            }
        };
        if let Some(trimmed) = function_types(&replacement) {
            replacement = trimmed;
        }
        bytes = bytes.checked_add(replacement.len())?;
        if bytes > MAX_BYTES {
            return None;
        }
        replacements.push((span, replacement));
    }
    replacements.sort_unstable_by_key(|(span, _)| span.start);
    let mut output = String::new();
    let mut cursor = 0;
    for (span, replacement) in replacements {
        if span.start < cursor {
            continue;
        }
        if output
            .len()
            .checked_add(span.start - cursor)
            .and_then(|n| n.checked_add(replacement.len()))?
            > MAX_BYTES
        {
            return None;
        }
        output.push_str(source.get(cursor..span.start)?);
        output.push_str(&replacement);
        cursor = span.end;
    }
    if output.len().checked_add(source.len() - cursor)? > MAX_BYTES {
        return None;
    }
    output.push_str(source.get(cursor..)?);
    Some(output.trim().to_owned())
}

fn qualify_symbol_for_consumer(
    workspace: &Workspace,
    consumer: &Path,
    origin: &Path,
    symbol: &crate::index::IndexedSymbol,
) -> Option<String> {
    let same = consumer.parent() == origin.parent()
        && workspace.files.get(consumer)?.package == workspace.files.get(origin)?.package;
    if same {
        return (symbol.name.len() <= MAX_LABEL_BYTES).then(|| symbol.name.clone());
    }
    let import = workspace.import_path(origin);
    let imported = workspace
        .files
        .get(consumer)?
        .imports
        .iter()
        .find(|c| !c.is_blank() && Some(c.path.as_str()) == import.as_deref());
    let dot = imported.is_some_and(|i| i.is_dot());
    let alias = imported
        .and_then(|c| workspace.import_name(c))
        .unwrap_or(&workspace.files.get(origin)?.package);
    if alias
        .len()
        .saturating_add(symbol.name.len())
        .saturating_add(1)
        > MAX_LABEL_BYTES
    {
        return None;
    }
    if dot {
        Some(symbol.name.clone())
    } else {
        Some(format!("{alias}.{}", symbol.name))
    }
}

fn trim_outer_parens(mut text: &str) -> &str {
    for _ in 0..MAX_DEPTH {
        let trimmed = text.trim();
        let Some(inner) = trimmed.strip_prefix('(').and_then(|s| s.strip_suffix(')')) else {
            return trimmed;
        };
        let mut depth = 0i32;
        let mut matched = true;
        for token in Lexer::new(trimmed) {
            match token.kind {
                TokenKind::LParen => depth += 1,
                TokenKind::RParen => {
                    depth -= 1;
                    if depth == 0 && token.span.end < trimmed.len() {
                        matched = false;
                        break;
                    }
                }
                _ => {}
            }
        }
        if !matched || depth != 0 {
            return trimmed;
        }
        text = inner;
    }
    text.trim()
}

fn trim_outer_paren_span(source: &str, mut span: [u32; 2]) -> [u32; 2] {
    for _ in 0..MAX_DEPTH {
        let Some(slice) = source.get(span[0] as usize..span[1] as usize) else {
            return span;
        };
        let mut tokens = Lexer::new(slice);
        let Some(first) = tokens.next() else {
            return span;
        };
        if first.kind != TokenKind::LParen {
            return span;
        }
        let mut depth = 1i32;
        let mut closing = None;
        for token in tokens {
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
        let Some(close_start) = closing else {
            return span;
        };
        let after = slice.get(close_start + 1..).unwrap_or("");
        if Lexer::new(after).next().is_some() {
            return span;
        }
        span = [
            span[0] + first.span.end as u32,
            span[0] + close_start as u32,
        ];
    }
    span
}

fn starts_with_func_lparen(text: &str) -> bool {
    let mut tokens = Lexer::new(text);
    tokens.next().is_some_and(|t| t.kind == TokenKind::Func)
        && tokens.next().is_some_and(|t| t.kind == TokenKind::LParen)
}

fn callee_ident_in_expr_span(source: &str, span: [u32; 2]) -> Option<[u32; 2]> {
    let slice = source.get(span[0] as usize..span[1] as usize)?;
    if slice.len() > MAX_LABEL_BYTES {
        return None;
    }
    let parsed = syntax::parse_expression(slice);
    if !parsed.diagnostics.is_empty() {
        return None;
    }
    let mut id = parsed.root;
    for _ in 0..MAX_DEPTH {
        let expr = parsed.nodes.get(id.0)?;
        match &expr.kind {
            ExprKind::Parenthesized { expression } => id = *expression,
            ExprKind::Index { operand, .. } => id = *operand,
            ExprKind::Name => {
                return Some([
                    span[0] + u32::try_from(expr.span.start).ok()?,
                    span[0] + u32::try_from(expr.span.end).ok()?,
                ]);
            }
            ExprKind::Selector { field, .. } => {
                return Some([
                    span[0] + u32::try_from(field.start).ok()?,
                    span[0] + u32::try_from(field.end).ok()?,
                ]);
            }
            _ => return None,
        }
    }
    None
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
        expression_file: &Path,
        text: &str,
        slot: usize,
        count: usize,
        package_level: bool,
        invalid_import: impl FnOnce(&str) -> bool,
    ) -> Option<String> {
        let parsed = syntax::parse_expression(text);
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
                crate::analysis::resolve::unqualified_symbol(workspace, expression_file, name)?
            }
            ExprKind::Selector { operand, field } => {
                let operand = parsed.nodes.get(operand.0)?;
                let ExprKind::Name = operand.kind else {
                    return None;
                };
                let alias = text.get(operand.span.clone())?;
                let field = text.get(field.clone())?;
                let mut found = crate::analysis::resolve::imported_symbol_iter(
                    workspace,
                    expression_file,
                    alias,
                )
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
            let (expr_file, _) = workspace.files.get_key_value(expression_file)?;
            let lhs = std::iter::repeat_n("_", count.max(1))
                .collect::<Vec<_>>()
                .join(", ");
            let prefix = format!("{lhs} = ");
            let wrapped = format!("{prefix}{text}");
            let tree = syntax::parse_decl_spec(&wrapped, DeclSpecKind::Var);
            if !tree.diagnostics().is_empty() {
                return None;
            }
            let span = [0u32, u32::try_from(wrapped.len()).ok()?];
            let scopes = ScopeTree::from_top_level_decl(&wrapped, 0, &tree, None);
            let mut region = RegionInfo::new(None, span, scopes);
            let mut arena = TypeArena::with_shared_budget(workspace);
            let mut decls = DeclCache::new();
            {
                let mut checker = RegionChecker::new(
                    &mut arena,
                    &mut decls,
                    expr_file.as_path(),
                    &wrapped,
                    0,
                    &tree,
                    &mut region,
                    None,
                    None,
                );
                checker.check_decl_spec(false);
            }
            region.finalize();
            let rhs_span = [u32::try_from(prefix.len()).ok()?, span[1]];
            let res_ty = if count <= 1 {
                region.expr_type(rhs_span)?.ty
            } else {
                let call = region.calls.iter().find(|c| c.span == rhs_span)?;
                let (_, results, _) = arena.signature_parts(call.signature?)?;
                *results.get(slot)?
            };
            return arena.format_type(res_ty, Qualifier::Presentation(self.file()));
        }
        let declaration = symbol.declaration()?;
        let end = symbol.body().map_or(declaration.end, |body| body.start);
        let header = workspace.source_slice(origin, declaration.start..end)?;
        if header.len() > MAX_BYTES {
            return None;
        }
        let signature = syntax::parse_signature(&header);
        if !signature.diagnostics.is_empty() || !signature.type_parameters.is_empty() {
            return None;
        }
        if count != signature.results.len() {
            return None;
        }
        let result = signature.results.get(slot)?;
        let annotation = header.get(result.annotation.clone())?;
        label::signature_type(workspace, self.file(), origin, annotation)
    }

    pub(in crate::analysis::query) fn local_binding_signature(
        &self,
        selection: &std::ops::Range<usize>,
        scope_start: usize,
        display_name: &str,
    ) -> Option<(CallableSignature, bool)> {
        let s0 = u32::try_from(selection.start).ok()?;
        let s1 = u32::try_from(selection.end).ok()?;
        let file = self.file();
        let source = self.source();
        let workspace = self.workspace();
        self.with_sema(|sema| {
            let (id, query_pos) = {
                let (region, _) = sema.region_and_arena_at(file, source, s0, None)?;
                resolve_local_binding_id(region, [s0, s1], scope_start)?
            };
            let local_env = {
                let (region, _) = sema.region_and_arena_at(file, source, s0, None)?;
                visible_local_env(region, source, query_pos)
            };
            callable_from_local_var(
                workspace,
                sema,
                file,
                source,
                id,
                query_pos,
                None,
                display_name,
                &local_env,
            )
        })
    }
}

pub(super) fn resolve_local_binding_id(
    region: &RegionInfo<'_>,
    selection: [u32; 2],
    scope_start: usize,
) -> Option<(LocalId, u32)> {
    if let Some(ObjectRef::Var {
        origin: SymbolOrigin::Local { id, .. },
        ..
    }) = region.def_by_span(selection)
    {
        return Some((id, selection[0]));
    }
    let case_pos = u32::try_from(scope_start).ok()?;
    region.implicits.iter().find_map(|&(cspan, obj)| {
        if cspan[0] <= case_pos
            && case_pos < cspan[1]
            && let ObjectRef::Var {
                origin: SymbolOrigin::Local { id, span, .. },
                ..
            } = obj
            && span == selection
        {
            Some((id, case_pos))
        } else {
            None
        }
    })
}
