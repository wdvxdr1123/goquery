//! Instantiated generic signature display formatting over `TypeArena`.
use super::label::{CallableSignature, signature_label, signature_type};
use crate::analysis::budget::{MAX_BYTES, MAX_NODES};
use crate::analysis::obj::DeclId;
use crate::analysis::types::{TypeArena, TypeId};
use crate::syntax::{Lexer, TokenKind, TypeNodeKind};
use crate::workspace::Workspace;
use std::path::Path;

pub(in crate::analysis::query) fn format_instantiated_signature(
    workspace: &Workspace,
    consumer: &Path,
    arena: &mut TypeArena<'_>,
    decl: DeclId<'_>,
    type_args: &[TypeId],
) -> Option<(CallableSignature, bool)> {
    let symbol = decl.symbol;
    if !symbol.is_function() || !symbol.generic {
        return None;
    }
    let span = symbol.declaration()?;
    let end = symbol.body().map_or(span.end, |body| body.start);
    if end.checked_sub(span.start)? > MAX_BYTES {
        return None;
    }
    let header = workspace.source_slice(decl.file, span.start..end)?;
    let signature = crate::syntax::parse_signature(&header);
    if !signature.diagnostics.is_empty() || signature.type_parameters.len() != type_args.len() {
        return None;
    }
    let environment: Vec<_> = signature
        .type_parameters
        .iter()
        .map(|parameter| header.get(parameter.name.as_ref()?.clone()))
        .collect::<Option<Vec<_>>>()?
        .into_iter()
        .zip(type_args.iter().copied())
        .collect();
    let (callable, _) = signature_label(&header, &signature, &symbol.name, |ann| {
        annotation(workspace, consumer, arena, decl.file, ann, &environment)
    })?;
    let variadic = signature.parameters.last().is_some_and(|p| p.variadic);
    Some((callable, variadic))
}

fn annotation(
    workspace: &Workspace,
    consumer: &Path,
    arena: &mut TypeArena<'_>,
    origin: &Path,
    source: &str,
    environment: &[(&str, TypeId)],
) -> Option<String> {
    let native = arena.parse_in(origin, source, environment)?;
    let fallback = arena.render_presentation(native, consumer)?;
    if !Lexer::new(source).any(|token| matches!(token.kind, TokenKind::Func | TokenKind::Interface))
    {
        return Some(fallback);
    }
    let syntax = crate::syntax::parse_type(source);
    if !syntax.diagnostics().is_empty()
        || syntax.references.len() + syntax.type_nodes().len() > MAX_NODES
    {
        return None;
    }
    let mut replacements = Vec::new();
    let mut bytes = 0usize;
    for reference in &syntax.references {
        let start = reference
            .qualifier
            .as_ref()
            .map_or(reference.name.start, |qualifier| qualifier.start);
        let span = start..reference.name.end;
        let name = source.get(reference.name.clone())?;
        let formal = reference
            .qualifier
            .is_none()
            .then(|| environment.iter().find(|(formal, _)| *formal == name))
            .flatten();
        let mut replacement = if let Some((_, actual)) = formal {
            arena.render_presentation(*actual, consumer)?
        } else {
            signature_type(workspace, consumer, origin, source.get(span.clone())?)?
        };
        if formal.is_some()
            && Lexer::new(&replacement)
                .next()
                .is_some_and(|token| token.kind == TokenKind::Arrow)
            && syntax
                .type_nodes()
                .iter()
                .any(|node| node.kind == TypeNodeKind::Chan && node.span.end == span.end)
        {
            replacement = format!("({replacement})");
        }
        bytes = bytes.checked_add(replacement.len())?;
        if bytes > MAX_BYTES {
            return None;
        }
        replacements.push((span, replacement));
    }
    replacements.sort_unstable_by_key(|(span, _)| span.start);
    let mut output = String::new();
    let mut start = 0;
    for (span, replacement) in replacements {
        if span.start < start {
            return None;
        }
        if output
            .len()
            .checked_add(span.start - start + replacement.len())?
            > MAX_BYTES
        {
            return None;
        }
        output.push_str(source.get(start..span.start)?);
        output.push_str(&replacement);
        start = span.end;
    }
    if output.len().checked_add(source.len() - start)? > MAX_BYTES {
        return None;
    }
    output.push_str(source.get(start..)?);
    Some(output)
}
