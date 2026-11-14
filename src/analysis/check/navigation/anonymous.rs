//! Inline (`struct{...}`) field targets, including elided composite literals.
//!
//! A composite literal whose element type is an anonymous struct has no named
//! owner symbol, so member lookup/receiver paths cannot see its fields. The
//! literal's own type syntax is the declaration, which keeps the identity
//! local: match the field name against that exact type expression only.
//!
//! Selector chains that end in an inline-struct field are resolved from the
//! binding/initializer/annotation syntax of the chain root, stepping named and
//! inline struct fields without retaining any cross-request state.
use crate::analysis::method_set::ReceiverMember;
use crate::analysis::scope::{Binding, CursorScope, Locals};
use crate::analysis::source::{Name, Role, SourceAnalysis};
use crate::index::IndexedSymbol as Symbol;
use crate::syntax::{ExprKind, Lexer, MemberKind, TokenKind};
use crate::workspace::Workspace;
use std::ops::Range;
use std::path::{Path, PathBuf};

/// One inline struct type expression and its field name selections.
struct InlineStruct {
    /// `struct{...}` span.
    type_span: Range<usize>,
    /// Absolute field-name selections of the root type expression.
    members: Vec<(String, Range<usize>)>,
}

/// Prepared inline-struct sites for one source snapshot. A document-wide
/// highlight request resolves many keys against the same syntax; building the
/// site list once per request keeps that work query-local.
pub(crate) struct InlineIndex {
    structs: Vec<InlineStruct>,
}

impl InlineIndex {
    pub(crate) fn new(source: &str) -> Self {
        Self {
            structs: inline_structs(source),
        }
    }
}

/// The declaration selected by a cursor on an inline-struct field.
pub(crate) struct InlineField {
    pub(crate) file: PathBuf,
    pub(crate) selection: Range<usize>,
    pub(crate) name: String,
}

/// Typed occurrence resolution shared by reference seeds and candidate scans.
pub(crate) fn occurrence_field(
    workspace: &Workspace,
    file: &Path,
    source: &str,
    analysis: &SourceAnalysis,
    occurrence: &Name,
    index: &InlineIndex,
) -> Option<InlineField> {
    match &occurrence.role {
        Role::LiteralKey(annotation) => {
            literal_field_declaration(workspace, file, source, &occurrence.span, annotation, &[])
        }
        Role::LiteralElementKey(context) => {
            let (annotation, projections) = analysis.literal_element_projection(*context)?;
            literal_field_declaration(
                workspace,
                file,
                source,
                &occurrence.span,
                &annotation,
                &projections,
            )
        }
        Role::Member(Some(operand)) | Role::ExpressionMember(operand) => {
            field_declaration_with_scope(
                workspace,
                file,
                source,
                &occurrence.span,
                &analysis.locals,
                Some(operand.clone()),
                index,
            )
        }
        Role::Field => field_declaration_with_scope(
            workspace,
            file,
            source,
            &occurrence.span,
            &analysis.locals,
            None,
            index,
        ),
        _ => None,
    }
}

/// Returns the declaration selection for a cursor on an inline-struct field, or
/// the field declaration selected by a keyed element of a composite literal
/// whose head type is that inline struct (including elided nested literals).
pub(crate) fn field_target(source: &str, selection: &Range<usize>) -> Option<Range<usize>> {
    let index = InlineIndex::new(source);
    field_target_in(source, &index, selection)
}

pub(crate) fn field_target_in(
    source: &str,
    index: &InlineIndex,
    selection: &Range<usize>,
) -> Option<Range<usize>> {
    // 1. Cursor on the field declaration inside `struct{...}`.
    for site in &index.structs {
        if let Some((_, span)) = site.members.iter().find(|(_, span)| span == selection) {
            return Some(span.clone());
        }
    }
    // 2. Keyed element (`name:`) of a composite literal headed by the type.
    if !Lexer::new(&source[selection.end..])
        .next()
        .is_some_and(|token| token.kind == TokenKind::Colon)
    {
        return None;
    }
    let name = &source[selection.clone()];
    let mut best: Option<(usize, Range<usize>)> = None;
    for site in &index.structs {
        let Some((_, member)) = site.members.iter().find(|(field, _)| field == name) else {
            continue;
        };
        let Some(literal) = literal_after(source, site.type_span.end) else {
            continue;
        };
        if !(literal.start < selection.start && selection.end <= literal.end) {
            continue;
        }
        let distance = selection.start - site.type_span.end;
        if best.as_ref().is_none_or(|(closest, _)| distance < *closest) {
            best = Some((distance, member.clone()));
        }
    }
    best.map(|(_, member)| member)
}

/// Full inline-field resolution using a prepared source-site index.
pub(crate) fn field_declaration_in(
    workspace: &Workspace,
    file: &Path,
    source: &str,
    selection: &Range<usize>,
    offset: usize,
    index: &InlineIndex,
) -> Option<InlineField> {
    if let Some(selection) = field_target_in(source, index, selection) {
        return Some(InlineField {
            name: source[selection.clone()].to_owned(),
            file: file.to_path_buf(),
            selection,
        });
    }
    let scope = cursor_scope(workspace, file, source, offset);
    if let Some(body) = scope.as_ref().and_then(|scope| scope.body.as_ref())
        && let Some(operand) = body.tree.expressions.iter().find_map(|expression| {
            let ExprKind::Selector { operand, field } = &expression.kind else {
                return None;
            };
            ((body.base + field.start..body.base + field.end) == *selection).then(|| {
                let span = &body.tree.expressions[operand.0].span;
                body.base + span.start..body.base + span.end
            })
        })
    {
        return field_declaration_with_scope(
            workspace,
            file,
            source,
            selection,
            &scope.as_ref()?.locals,
            Some(operand),
            index,
        );
    }
    let (base, fields) = selector_chain(source, selection)?;
    let locals = scope.as_ref().map(|scope| &scope.locals);
    let context = Context {
        workspace,
        file,
        source,
        locals,
    };
    let mut ty = expression_type(&context, &source[base.clone()], base.start, 0)?;
    for (index, name) in fields.iter().enumerate() {
        let inline_owner = matches!(classify(&ty), Some(TypeKind::Inline(_)));
        let member = field_member(workspace, &ty, name)?;
        if index + 1 == fields.len() {
            if !inline_owner {
                return None;
            }
            return Some(InlineField {
                file: member.file,
                selection: member.selection,
                name: name.clone(),
            });
        }
        ty = member.annotation;
    }
    None
}

/// Resolve a prepared occurrence without rebuilding its lexical body scope.
pub(crate) fn field_declaration_with_scope(
    workspace: &Workspace,
    file: &Path,
    source: &str,
    selection: &Range<usize>,
    locals: &Locals,
    operand: Option<Range<usize>>,
    index: &InlineIndex,
) -> Option<InlineField> {
    if let Some(selection) = field_target_in(source, index, selection) {
        return Some(InlineField {
            file: file.to_path_buf(),
            name: source[selection.clone()].to_owned(),
            selection,
        });
    }
    let operand = operand?;
    let context = Context {
        workspace,
        file,
        source,
        locals: Some(locals),
    };
    let ty = expression_type(&context, source.get(operand.clone())?, operand.start, 0)?;
    if !matches!(classify(&ty), Some(TypeKind::Inline(_))) {
        return None;
    }
    let name = source.get(selection.clone())?;
    let member = field_member(workspace, &ty, name)?;
    Some(InlineField {
        file: member.file,
        selection: member.selection,
        name: name.to_owned(),
    })
}

/// A direct field conflict in the same anonymous aggregate.
pub(crate) fn field_conflict(
    index: &InlineIndex,
    selection: &Range<usize>,
    new_name: &str,
) -> Option<Range<usize>> {
    let owner = index
        .structs
        .iter()
        .find(|site| site.members.iter().any(|(_, span)| span == selection))?;
    owner
        .members
        .iter()
        .find(|(name, span)| name == new_name && span != selection)
        .map(|(_, span)| span.clone())
}

/// Project a literal's expected type without losing its declaration byte origin.
pub(crate) fn literal_field_declaration(
    workspace: &Workspace,
    file: &Path,
    source: &str,
    selection: &Range<usize>,
    annotation: &Range<usize>,
    projections: &[bool],
) -> Option<InlineField> {
    if annotation.len() > 16 * 1024 || projections.len() > 64 {
        return None;
    }
    let mut ty = TypeSyntax {
        file: file.to_path_buf(),
        start: annotation.start,
        text: source.get(annotation.clone())?.to_owned(),
    };
    for key in projections.iter().rev() {
        ty = project(workspace, &ty, usize::from(!key), 2, 0)?;
    }
    let name = source.get(selection.clone())?;
    let member = field_member(workspace, &ty, name)?;
    Some(InlineField {
        file: member.file,
        selection: member.selection,
        name: name.to_owned(),
    })
}

/// Final field declaration of a selector chain, named or inline owner.
/// Returns the declaring file and the field-name selection there.
pub(crate) fn chain_location(
    workspace: &Workspace,
    file: &Path,
    source: &str,
    selection: &Range<usize>,
    offset: usize,
) -> Option<(PathBuf, Range<usize>)> {
    let (base, fields) = selector_chain(source, selection)?;
    let scope = cursor_scope(workspace, file, source, offset);
    let context = Context {
        workspace,
        file,
        source,
        locals: scope.as_ref().map(|scope| &scope.locals),
    };
    let mut ty = expression_type(&context, &source[base.clone()], base.start, 0)?;
    let mut declaration = None;
    for (index, name) in fields.iter().enumerate() {
        let member = field_member(workspace, &ty, name)?;
        if index + 1 == fields.len() {
            declaration = Some((member.file, member.selection));
            break;
        }
        ty = member.annotation;
    }
    declaration
}

fn inline_structs(source: &str) -> Vec<InlineStruct> {
    let mut result = Vec::new();
    let mut tokens = Lexer::new(source);
    while let Some(token) = tokens.next() {
        if token.kind != TokenKind::Struct {
            continue;
        }
        let start = token.span.start;
        let mut depth = 0usize;
        let mut end = None;
        for token in tokens.clone() {
            match token.kind {
                TokenKind::LBrace => depth += 1,
                TokenKind::RBrace => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        end = Some(token.span.end);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(end) = end else { continue };
        let text = &source[start..end];
        let Some(root) = crate::syntax::aggregate_offset(text) else {
            continue;
        };
        let members: Vec<(String, Range<usize>)> = crate::syntax::aggregate_members(text)
            .into_iter()
            .filter(|member| member.container == root && member.symbol.kind == MemberKind::Field)
            .map(|member| {
                (
                    member.symbol.name,
                    start + member.selection.start..start + member.selection.end,
                )
            })
            .collect();
        result.push(InlineStruct {
            type_span: start..end,
            members,
        });
    }
    result
}

/// Span from the composite literal's opening brace to its matching close, when
/// the type expression containing this inline struct is followed by `{`.
/// Enclosing slice/array/map brackets between the struct and the literal are
/// skipped (`[]struct{...}{...}`); any other token ends the search.
fn literal_after(source: &str, mut offset: usize) -> Option<Range<usize>> {
    let open = loop {
        skip_whitespace(source, &mut offset);
        let mut tokens = crate::syntax::Lexer::new(&source[offset..]).peekable();
        let token = tokens.next()?;
        match token.kind {
            TokenKind::LBrace => break offset,
            TokenKind::RBrack => offset += token.span.end,
            _ => return None,
        }
    };
    let mut tokens = crate::syntax::Lexer::new(&source[open..]);
    let mut depth = 0usize;
    let mut end = None;
    for token in tokens.by_ref() {
        match token.kind {
            TokenKind::LBrace => depth += 1,
            TokenKind::RBrace => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    end = Some(open + token.span.end);
                    break;
                }
            }
            _ => {}
        }
    }
    end.map(|end| open..end)
}

fn skip_whitespace(source: &str, offset: &mut usize) {
    while let Some(character) = source[*offset..].chars().next() {
        if !character.is_whitespace() {
            break;
        }
        *offset += character.len_utf8();
    }
}

// ---------------------------------------------------------------------------
// Selector-chain resolution
// ---------------------------------------------------------------------------

/// The query-local source/metadata view for one rename preparation.
struct Context<'a> {
    workspace: &'a Workspace,
    file: &'a Path,
    source: &'a str,
    locals: Option<&'a Locals>,
}

/// A type expression spelling together with the file whose import scope gives
/// meaning to its qualified names.
#[derive(Clone)]
struct TypeSyntax {
    file: PathBuf,
    start: usize,
    text: String,
}

enum TypeKind {
    Named(String),
    Inline(InlineSite),
    Array(TypeSyntax),
    Map { key: TypeSyntax, value: TypeSyntax },
    Chan(TypeSyntax),
}

struct InlineSite {
    file: PathBuf,
    members: Vec<InlineMember>,
}

struct InlineMember {
    name: String,
    selection: Range<usize>,
    annotation: TypeSyntax,
}

struct FieldMember {
    file: PathBuf,
    selection: Range<usize>,
    annotation: TypeSyntax,
}

fn cursor_scope<'a>(
    workspace: &'a Workspace,
    file: &'a Path,
    source: &'a str,
    offset: usize,
) -> Option<CursorScope> {
    let function = workspace
        .files
        .get(file)?
        .symbols
        .iter()
        .find_map(|symbol| {
            let body = symbol.body()?;
            (body.contains(&offset) || offset == source.len() && body.end == offset)
                .then(|| symbol.declaration().map(|span| (span.start, body.clone())))
                .flatten()
        });
    Locals::scope_at_cursor(source, offset, function).ok()
}

/// Splits `base.field1.field2` into the base expression span and field names.
fn selector_chain(source: &str, selection: &Range<usize>) -> Option<(Range<usize>, Vec<String>)> {
    let tokens: Vec<(TokenKind, Range<usize>)> = Lexer::new(source)
        .filter(|token| token.kind != TokenKind::Comment)
        .map(|token| (token.kind, token.span))
        .collect();
    let index = tokens.iter().position(|(_, span)| *span == *selection)?;
    if index == 0 || tokens[index].0 != TokenKind::Ident {
        return None;
    }
    let mut fields = Vec::new();
    let mut cursor = index;
    loop {
        if tokens[cursor].0 != TokenKind::Ident {
            return None;
        }
        fields.push(source[tokens[cursor].1.clone()].to_owned());
        if cursor < 2 || tokens[cursor - 1].0 != TokenKind::Period {
            return None;
        }
        let previous = tokens[cursor - 2].clone();
        if previous.0 == TokenKind::Ident {
            cursor -= 2;
            if cursor == 0 {
                fields.reverse();
                return Some((previous.1, fields));
            }
            if tokens[cursor - 1].0 == TokenKind::Period {
                continue;
            }
            fields.reverse();
            return Some((previous.1, fields));
        }
        return None;
    }
}

fn expression_type(
    context: &Context<'_>,
    text: &str,
    base: usize,
    depth: usize,
) -> Option<TypeSyntax> {
    if depth >= 16 || text.len() > 16 * 1024 {
        return None;
    }
    let parsed = crate::syntax::parse_expression(text);
    if !parsed.diagnostics.is_empty() {
        return None;
    }
    let expression = &parsed.nodes[parsed.root.0];
    match &expression.kind {
        ExprKind::Name => name_type(context, text, base, &expression.span, depth),
        ExprKind::Selector { operand, field } => {
            let operand = &parsed.nodes[operand.0];
            if matches!(operand.kind, ExprKind::Name)
                && context
                    .locals
                    .and_then(|locals| {
                        locals.resolve_at(base + operand.span.start, &text[operand.span.clone()])
                    })
                    .is_none()
                && let Some((file, symbol)) = context
                    .workspace
                    .imported_symbol_iter(context.file, &text[operand.span.clone()])
                    .find(|(_, symbol)| symbol.name == text[field.clone()] && symbol.is_value())
            {
                return symbol_type(context, file, symbol, depth + 1);
            }
            let receiver = expression_type(
                context,
                &text[operand.span.clone()],
                base + operand.span.start,
                depth + 1,
            )?;
            field_member(context.workspace, &receiver, &text[field.clone()])
                .map(|member| member.annotation)
        }
        ExprKind::Composite {
            literal_type: Some(ty),
            ..
        } => {
            let ty = &parsed.nodes[ty.0];
            syntax(context, &text[ty.span.clone()], base + ty.span.start)
        }
        ExprKind::Unary { operator, operand } => {
            if !matches!(&text[operator.clone()], "&" | "*") {
                return None;
            }
            let operand = &parsed.nodes[operand.0];
            expression_type(
                context,
                &text[operand.span.clone()],
                base + operand.span.start,
                depth + 1,
            )
        }
        ExprKind::Parenthesized { expression } => {
            let expression = &parsed.nodes[expression.0];
            expression_type(
                context,
                &text[expression.span.clone()],
                base + expression.span.start,
                depth + 1,
            )
        }
        ExprKind::Call { function, .. } => call_result(
            context.workspace,
            context,
            &parsed,
            text,
            base,
            *function,
            depth,
        ),
        ExprKind::Index { operand, .. } => {
            let operand = &parsed.nodes[operand.0];
            let ty = expression_type(
                context,
                &text[operand.span.clone()],
                base + operand.span.start,
                depth + 1,
            )?;
            indexed_element(context.workspace, &ty)
        }
        ExprKind::Slice { operand, .. } => {
            let operand = &parsed.nodes[operand.0];
            expression_type(
                context,
                &text[operand.span.clone()],
                base + operand.span.start,
                depth + 1,
            )
        }
        ExprKind::TypeAssertion { asserted_type, .. } => syntax(
            context,
            &text[asserted_type.clone()],
            base + asserted_type.start,
        ),
        _ => None,
    }
}

fn name_type(
    context: &Context<'_>,
    text: &str,
    base: usize,
    span: &Range<usize>,
    depth: usize,
) -> Option<TypeSyntax> {
    let name = &text[span.clone()];
    if let Some(locals) = context.locals
        && let Some(binding) = locals.resolve_at(base + span.start, name)
    {
        return binding_type(context, binding, depth + 1);
    }
    let (file, symbol) = context.workspace.unqualified_symbol(context.file, name)?;
    symbol_type(context, file, symbol, depth + 1)
}

fn symbol_type(
    context: &Context<'_>,
    file: &Path,
    symbol: &Symbol,
    depth: usize,
) -> Option<TypeSyntax> {
    if file == context.file {
        return declaration_type(context, symbol, depth);
    }
    let source = context.workspace.source(file)?;
    declaration_type(
        &Context {
            workspace: context.workspace,
            file,
            source: &source,
            locals: None,
        },
        symbol,
        depth,
    )
}

fn binding_type(context: &Context<'_>, binding: &Binding, depth: usize) -> Option<TypeSyntax> {
    if depth >= 16 {
        return None;
    }
    if let Some(annotation) = binding.annotation() {
        return syntax(
            context,
            &context.source[annotation.clone()],
            annotation.start,
        );
    }
    if let Some(declared) = &binding.declared_type {
        return Some(TypeSyntax {
            file: context.file.to_path_buf(),
            start: binding.selection.start,
            text: declared.clone(),
        });
    }
    if let Some((span, result, count)) = binding.range_initializer() {
        let ty = expression_type(context, &context.source[span.clone()], span.start, 0)?;
        return project(context.workspace, &ty, result, count, depth + 1);
    }
    if let Some((span, result, count)) = binding.initializer() {
        let ty = expression_type(context, &context.source[span.clone()], span.start, 0)?;
        if count > 1 {
            return project(context.workspace, &ty, result, count, depth + 1);
        }
        return Some(ty);
    }
    None
}

fn declaration_type(context: &Context<'_>, symbol: &Symbol, depth: usize) -> Option<TypeSyntax> {
    if depth >= 16 {
        return None;
    }
    let declaration = symbol.declaration()?;
    let tail = context.source.get(symbol.end..declaration.end)?;
    if tail.len() > 16 * 1024 {
        return None;
    }
    let base = symbol.end;
    if let Some(assign) = top_level_assign(tail) {
        let rhs = &tail[assign + 1..];
        let trimmed = rhs.len() - rhs.trim_start().len();
        return expression_type(
            context,
            &rhs[trimmed..],
            base + assign + 1 + trimmed,
            depth + 1,
        );
    }
    let trimmed = tail.len() - tail.trim_start().len();
    syntax(context, &tail[trimmed..], base + trimmed)
}

fn top_level_assign(text: &str) -> Option<usize> {
    let mut depth = 0usize;
    for token in Lexer::new(text) {
        match token.kind {
            TokenKind::Assign if depth == 0 => return Some(token.span.start),
            TokenKind::LParen | TokenKind::LBrack | TokenKind::LBrace => depth += 1,
            TokenKind::RParen | TokenKind::RBrack | TokenKind::RBrace => {
                depth = depth.saturating_sub(1);
            }
            _ => {}
        }
    }
    None
}

fn call_result(
    workspace: &Workspace,
    context: &Context<'_>,
    parsed: &crate::syntax::ExpressionTree,
    text: &str,
    base: usize,
    function: crate::syntax::ExprId,
    depth: usize,
) -> Option<TypeSyntax> {
    if depth >= 16 {
        return None;
    }
    let function = &parsed.nodes[function.0];
    match &function.kind {
        ExprKind::Name => {
            let name = &text[function.span.clone()];
            let (path, symbol) = workspace.unqualified_symbol(context.file, name)?;
            if !symbol.is_function() {
                return None;
            }
            signature_result(workspace, path, symbol)
        }
        ExprKind::Selector { operand, field } => {
            let operand = &parsed.nodes[operand.0];
            if matches!(operand.kind, ExprKind::Name)
                && context
                    .locals
                    .and_then(|locals| {
                        locals.resolve_at(base + operand.span.start, &text[operand.span.clone()])
                    })
                    .is_none()
                && let Some((file, symbol)) = workspace
                    .imported_symbol_iter(context.file, &text[operand.span.clone()])
                    .find(|(_, symbol)| symbol.name == text[field.clone()] && symbol.is_function())
            {
                return signature_result(workspace, file, symbol);
            }
            let receiver = expression_type(
                context,
                &text[operand.span.clone()],
                base + operand.span.start,
                depth + 1,
            )?;
            let TypeKind::Named(receiver_name) = classify(&receiver)? else {
                return None;
            };
            let field_name = &text[field.clone()];
            let member = workspace.receiver_member(&receiver.file, &receiver_name, field_name)?;
            let ReceiverMember::Method(path, symbol) = member else {
                return None;
            };
            signature_result(workspace, path, symbol)
        }
        _ => None,
    }
}

fn signature_result(workspace: &Workspace, file: &Path, symbol: &Symbol) -> Option<TypeSyntax> {
    let declaration = symbol.declaration()?;
    let end = symbol.body().map_or(declaration.end, |body| body.start);
    let header = workspace.source_slice(file, declaration.start..end)?;
    if header.len() > 16 * 1024 {
        return None;
    }
    let signature = crate::syntax::parse_signature(&header);
    if !signature.diagnostics.is_empty() {
        return None;
    }
    let result = signature.results.first()?;
    Some(TypeSyntax {
        file: file.to_path_buf(),
        start: declaration.start + result.annotation.start,
        text: header[result.annotation.clone()].to_owned(),
    })
}

fn syntax(context: &Context<'_>, text: &str, start: usize) -> Option<TypeSyntax> {
    if text.len() > 16 * 1024 {
        return None;
    }
    Some(TypeSyntax {
        file: context.file.to_path_buf(),
        start,
        text: text.to_owned(),
    })
}

fn classify(ty: &TypeSyntax) -> Option<TypeKind> {
    let mut tokens = Lexer::new(&ty.text).peekable();
    if tokens
        .peek()
        .is_some_and(|token| token.kind == TokenKind::Mul)
    {
        tokens.next();
        let offset = tokens.peek()?.span.start;
        let text = ty.text.get(offset..)?.to_owned();
        return classify(&TypeSyntax {
            file: ty.file.clone(),
            start: ty.start + offset,
            text,
        });
    }
    let first = tokens.next()?;
    match first.kind {
        TokenKind::LBrack => {
            let after = skip_bracket(&ty.text, first.span.end)?;
            let offset = skip_space(&ty.text, after);
            let text = ty.text.get(offset..)?;
            (!text.is_empty()).then(|| {
                TypeKind::Array(TypeSyntax {
                    file: ty.file.clone(),
                    start: ty.start + offset,
                    text: text.to_owned(),
                })
            })
        }
        TokenKind::Map => {
            let bracket = tokens.next()?;
            if bracket.kind != TokenKind::LBrack {
                return None;
            }
            let key_end = matching_bracket(&ty.text, bracket.span.end)?;
            let after = skip_bracket(&ty.text, bracket.span.end)?;
            let offset = skip_space(&ty.text, after);
            let key = &ty.text[bracket.span.end..key_end];
            let value = ty.text.get(offset..)?;
            if value.is_empty() || key.is_empty() {
                return None;
            }
            Some(TypeKind::Map {
                key: TypeSyntax {
                    file: ty.file.clone(),
                    start: ty.start + bracket.span.end,
                    text: key.to_owned(),
                },
                value: TypeSyntax {
                    file: ty.file.clone(),
                    start: ty.start + offset,
                    text: value.to_owned(),
                },
            })
        }
        TokenKind::Chan => {
            let offset = skip_space(&ty.text, first.span.end);
            let text = ty.text.get(offset..)?;
            (!text.is_empty()).then(|| {
                TypeKind::Chan(TypeSyntax {
                    file: ty.file.clone(),
                    start: ty.start + offset,
                    text: text.to_owned(),
                })
            })
        }
        TokenKind::Arrow => {
            let offset = skip_space(&ty.text, first.span.end);
            let mut inner = Lexer::new(ty.text.get(offset..)?);
            if inner.next()?.kind != TokenKind::Chan {
                return None;
            }
            let after = skip_space(&ty.text, offset + inner.next()?.span.end);
            let text = ty.text.get(after..)?;
            (!text.is_empty()).then(|| {
                TypeKind::Chan(TypeSyntax {
                    file: ty.file.clone(),
                    start: ty.start + after,
                    text: text.to_owned(),
                })
            })
        }
        TokenKind::Struct => inline_kind(ty, first.span.start),
        TokenKind::Ident => {
            let name = crate::analysis::scope::named_type(&ty.text)?;
            Some(TypeKind::Named(name))
        }
        _ => None,
    }
}

fn inline_kind(ty: &TypeSyntax, offset: usize) -> Option<TypeKind> {
    let text = ty.text.get(offset..)?;
    let root = crate::syntax::aggregate_offset(text)?;
    let members = crate::syntax::aggregate_members(text)
        .into_iter()
        .filter(|member| member.container == root && member.symbol.kind == MemberKind::Field)
        .map(|member| {
            let annotation = text.get(member.annotation.clone())?;
            Some(InlineMember {
                name: member.symbol.name,
                selection: ty.start + offset + member.selection.start
                    ..ty.start + offset + member.selection.end,
                annotation: TypeSyntax {
                    file: ty.file.clone(),
                    start: ty.start + offset + member.annotation.start,
                    text: annotation.to_owned(),
                },
            })
        })
        .collect::<Option<Vec<_>>>()?;
    Some(TypeKind::Inline(InlineSite {
        file: ty.file.clone(),
        members,
    }))
}

fn field_member(workspace: &Workspace, ty: &TypeSyntax, name: &str) -> Option<FieldMember> {
    match classify(ty)? {
        TypeKind::Inline(site) => site
            .members
            .into_iter()
            .find(|member| member.name == name)
            .map(|member| FieldMember {
                file: site.file,
                selection: member.selection,
                annotation: member.annotation,
            }),
        TypeKind::Named(receiver) => named_field(workspace, ty, &receiver, name),
        _ => None,
    }
}

fn named_field(
    workspace: &Workspace,
    ty: &TypeSyntax,
    receiver: &str,
    name: &str,
) -> Option<FieldMember> {
    let (file, owner) = workspace
        .underlying_target(&ty.file, receiver)
        .or_else(|| workspace.type_declaration(&ty.file, receiver))?;
    let declaration = owner.declaration()?;
    let text = workspace.source_slice(file, owner.end..declaration.end)?;
    if text.len() > 16 * 1024 {
        return None;
    }
    let root = crate::syntax::aggregate_offset(&text)?;
    let entry = crate::syntax::aggregate_members(&text)
        .into_iter()
        .find(|member| {
            member.container == root
                && member.symbol.kind == MemberKind::Field
                && member.symbol.name == name
        })?;
    let annotation = text.get(entry.annotation.clone())?;
    Some(FieldMember {
        file: file.clone(),
        selection: owner.end + entry.selection.start..owner.end + entry.selection.end,
        annotation: TypeSyntax {
            file: file.clone(),
            start: owner.end + entry.annotation.start,
            text: annotation.to_owned(),
        },
    })
}

fn project(
    workspace: &Workspace,
    ty: &TypeSyntax,
    result: usize,
    count: usize,
    depth: usize,
) -> Option<TypeSyntax> {
    if depth >= 16 {
        return None;
    }
    match classify(ty)? {
        TypeKind::Array(element) => {
            if count >= 2 && result >= 1 {
                Some(element)
            } else {
                None
            }
        }
        TypeKind::Map { key, value } => {
            if count == 1 || result == 0 {
                Some(key)
            } else {
                Some(value)
            }
        }
        TypeKind::Chan(element) => (count == 1 || result >= 1).then_some(element),
        TypeKind::Named(name) => {
            let underlying = underlying_type(workspace, ty, &name)?;
            project(workspace, &underlying, result, count, depth + 1)
        }
        TypeKind::Inline(_) => None,
    }
}

fn indexed_element(workspace: &Workspace, ty: &TypeSyntax) -> Option<TypeSyntax> {
    match classify(ty)? {
        TypeKind::Array(element) | TypeKind::Chan(element) => Some(element),
        TypeKind::Map { value, .. } => Some(value),
        TypeKind::Named(name) => {
            let underlying = underlying_type(workspace, ty, &name)?;
            indexed_element(workspace, &underlying)
        }
        TypeKind::Inline(_) => None,
    }
}

fn underlying_type(workspace: &Workspace, ty: &TypeSyntax, name: &str) -> Option<TypeSyntax> {
    let (file, owner) = workspace
        .underlying_target(&ty.file, name)
        .or_else(|| workspace.type_declaration(&ty.file, name))?;
    let declaration = owner.declaration()?;
    let text = workspace.source_slice(file, owner.end..declaration.end)?;
    if text.len() > 16 * 1024 {
        return None;
    }
    let mut start = 0usize;
    if let Some(parameters) = crate::syntax::type_parameter_end(&text) {
        start = parameters;
    }
    let tail = text.get(start..)?;
    let trimmed = tail.len() - tail.trim_start().len();
    let mut offset = start + trimmed;
    if Lexer::new(&text[offset..])
        .next()
        .is_some_and(|token| token.kind == TokenKind::Assign)
    {
        offset += text[offset..].find('=').map_or(0, |index| index + 1);
        let rest = &text[offset..];
        offset += rest.len() - rest.trim_start().len();
    }
    Some(TypeSyntax {
        file: file.clone(),
        start: owner.end + offset,
        text: text[offset..].to_owned(),
    })
}

fn skip_bracket(text: &str, from: usize) -> Option<usize> {
    let mut depth = 1usize;
    for token in Lexer::new(&text[from..]) {
        match token.kind {
            TokenKind::LBrack => depth += 1,
            TokenKind::RBrack => {
                depth -= 1;
                if depth == 0 {
                    return Some(from + token.span.end);
                }
            }
            _ => {}
        }
    }
    None
}

fn matching_bracket(text: &str, from: usize) -> Option<usize> {
    let mut depth = 1usize;
    for token in Lexer::new(&text[from..]) {
        match token.kind {
            TokenKind::LBrack => depth += 1,
            TokenKind::RBrack => {
                depth -= 1;
                if depth == 0 {
                    return Some(from + token.span.start);
                }
            }
            _ => {}
        }
    }
    None
}

fn skip_space(text: &str, mut offset: usize) -> usize {
    while let Some(character) = text[offset..].chars().next() {
        if !character.is_whitespace() {
            break;
        }
        offset += character.len_utf8();
    }
    offset
}
