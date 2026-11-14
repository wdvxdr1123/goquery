//! Names remain source presentation; instantiated type identity stays native.
use super::SignaturePresentation;
use crate::analysis::query::budget::{MAX_BYTES, MAX_NODES};
use crate::analysis::types::{TypeArena, TypeId};
use crate::syntax::{Lexer, TokenKind, TypeNodeKind};
use crate::workspace::Workspace;
use std::path::Path;

pub(in crate::analysis::check) fn annotation(
    workspace: &Workspace,
    consumer: &Path,
    arena: &mut TypeArena<'_>,
    origin: &Path,
    source: &str,
    environment: &[(&str, TypeId)],
    presentation: SignaturePresentation,
) -> Option<String> {
    let native = arena.parse_in(origin, source, environment)?;
    let fallback = match presentation {
        SignaturePresentation::Accessible => arena.render_resolved(native, consumer),
        SignaturePresentation::Display => arena.render_presentation(native, consumer),
    }?;
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
            match presentation {
                SignaturePresentation::Accessible => arena.render_resolved(*actual, consumer),
                SignaturePresentation::Display => arena.render_presentation(*actual, consumer),
            }?
        } else {
            workspace.signature_type(consumer, origin, source.get(span.clone())?)?
        };
        // A bidirectional channel of receive-only channels requires parentheses.
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
    // Display qualification may name an unimported package, exactly as go/types'
    // FileQualifier does. The source schema and substituted identities are known;
    // this label is deliberately unavailable to semantic text consumers.
    if matches!(presentation, SignaturePresentation::Display) {
        return Some(output);
    }
    // Requalification and substitution must describe the exact proven type.
    // Names are never stored in the arena or recovered from matching spellings.
    if arena
        .parse(consumer, &output)
        .and_then(|candidate| arena.identical(native, candidate))
        == Some(true)
    {
        Some(output)
    } else {
        Some(fallback)
    }
}
