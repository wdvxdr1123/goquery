//! Checked anonymous fields and universe methods, with source-only presentation.
use super::{Context, HoverContent, SourceRange, doc, render, size};
use crate::analysis::sema::{
    FieldLayout, MemberDeclId, MethodOrigin, Qualifier, RegionMode, SemanticObject,
};
use crate::syntax::{Lexer, TokenKind};
use std::ops::Range;

pub(super) fn checked(
    context: &mut Context<'_>,
    span: &Range<usize>,
    range: SourceRange,
) -> Option<HoverContent> {
    let selected = [
        u32::try_from(span.start).ok()?,
        u32::try_from(span.end).ok()?,
    ];
    let (identity, ty, checked_layout) = {
        let (doc, sema) = context.split();
        let workspace = doc.workspace();
        let file = doc.file();
        let mut view = sema.region_at(file, doc.source(), selected[0], RegionMode::Full)?;
        let (found, object) = view.object_at(selected[0])?;
        if found != selected {
            return None;
        }
        match object {
            SemanticObject::Method(MethodOrigin::Interface(member)) if member.builtin => {
                (member, None, None)
            }
            SemanticObject::Field(member) if member.indexed(workspace).is_none() => {
                let ty = view
                    .selection_by_span(selected)
                    .map(|selection| selection.ty)
                    .or_else(|| view.expr_type(selected).map(|operand| operand.ty))?;
                let rendered = view.types().format(ty, Qualifier::Presentation(file))?;
                let declaring = member.file == file && member.selection == selected;
                let layout = if declaring {
                    view.field_owner_struct(member, selected)
                        .and_then(|(owner_ty, ordinal)| {
                            view.types()
                                .struct_layout(owner_ty)?
                                .fields
                                .into_iter()
                                .nth(ordinal)
                        })
                } else {
                    None
                };
                (member, Some(rendered), layout)
            }
            _ => return None,
        }
    };
    let mut content = HoverContent::new(range);
    if identity.builtin {
        content.code = Some("func (error).Error() string".to_owned());
        return Some(content);
    }
    let declaration = identity.selection[0] as usize..identity.selection[1] as usize;
    let source_text = render::declaration_source(context, identity.file)?;
    let source_owned = source_text.text().to_owned();
    let source = source_owned.as_str();
    let name = source.get(declaration.clone())?;
    let mut code = format!("field {name} {}", render::normalize_type(&ty?));
    if let Some(field) = &checked_layout {
        append_field_layout(&mut code, field);
    }
    content.code = Some(code);
    attach(
        context,
        identity,
        source,
        span,
        checked_layout.is_some(),
        &mut content,
    );
    Some(content)
}

fn append_field_layout(code: &mut String, field: &FieldLayout) {
    code.push_str(&format!(" // size={}", size::number(field.size)));
    if let Some(class) = size::size_class(field.size).filter(|class| *class > field.size) {
        code.push_str(&format!(", class={}", size::number(class)));
    }
    code.push_str(&format!(", offset={}", size::number(field.offset)));
}

/// Locate presentation metadata by the already-proven declaration byte span.
/// Uses checked layout from [`TypeInspector::struct_layout`] when available and
/// avoids scanning source when neither fallback layout nor doc comments are needed.
fn attach(
    context: &mut Context<'_>,
    identity: MemberDeclId<'_>,
    source: &str,
    selected: &Range<usize>,
    has_checked_layout: bool,
    content: &mut HoverContent,
) -> Option<()> {
    const WINDOW: usize = crate::analysis::budget::MAX_BYTES;
    let declaration = identity.selection[0] as usize..identity.selection[1] as usize;
    let mut window_start = declaration.start.saturating_sub(WINDOW);
    while !source.is_char_boundary(window_start) {
        window_start += 1;
    }
    let mut window_end = source.len().min(declaration.end.saturating_add(WINDOW));
    while !source.is_char_boundary(window_end) {
        window_end -= 1;
    }
    let window = source.get(window_start..window_end)?;
    let declaring = identity.file == context.file() && declaration == *selected;
    let need_layout = declaring && !has_checked_layout;
    let has_comment = window.contains("//") || window.contains("/*") || window.contains("*/");
    if !need_layout && !has_comment {
        return Some(());
    }
    let mut candidate = None;
    let mut tokens = Lexer::new(window);
    while let Some(token) = tokens.next() {
        let start = window_start + token.span.start;
        if start >= declaration.start {
            break;
        }
        if token.kind != TokenKind::Struct {
            continue;
        }
        let mut depth = 0usize;
        let mut end = None;
        for close in tokens.clone() {
            if close.span.end.saturating_sub(token.span.start) > WINDOW {
                break;
            }
            match close.kind {
                TokenKind::LBrace => depth += 1,
                TokenKind::RBrace => {
                    depth = depth.checked_sub(1)?;
                    if depth == 0 {
                        end = Some(window_start + close.span.end);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(end) = end.filter(|end| declaration.end <= *end) else {
            continue;
        };
        let text = source.get(start..end)?;
        let entries = crate::syntax::aggregate_members(text);
        if let Some(entry) = entries.into_iter().find(|entry| {
            start + entry.selection.start == declaration.start
                && start + entry.selection.end == declaration.end
        }) {
            candidate = Some((start, end, entry));
        }
    }
    let (start, end, entry) = candidate?;
    let text = source.get(start..end)?;
    if need_layout
        && let Some(layout) = size::struct_text_layout(context, identity.file, text)
        && let Some(field) = layout
            .fields
            .iter()
            .find(|field| source.get(declaration.clone()) == Some(field.name.as_str()))
        && let Some(code) = content.code.as_mut()
    {
        append_field_layout(code, field);
    }
    if has_comment {
        let field_start = render::field_start(text, entry.selection.start).map_or(
            render::line_start_token(source, declaration.start),
            |offset| start + offset,
        );
        if let Some(documents) = doc::attached(
            context,
            identity.file,
            source,
            doc::Target {
                start: field_start,
                end: start + entry.annotation.end,
                declaration: None,
                trailing: true,
            },
        ) {
            content.doc = Some(documents.text);
            content.doc_links = documents.links;
        }
    }
    Some(())
}
