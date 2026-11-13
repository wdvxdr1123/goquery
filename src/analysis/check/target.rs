//! Expected syntactic type positions, including incomplete qualified names.
use crate::analysis::scope::CursorScope;
use crate::syntax::{Diagnostic, Lexer, TokenKind, TypeReference};
use crate::workspace::Workspace;
use std::path::Path;

pub(crate) fn type_position(
    workspace: &Workspace,
    file: &Path,
    source: &str,
    offset: usize,
    scope: &CursorScope,
) -> bool {
    if let Some(body) = &scope.body
        && type_context_at(
            &body.tree.type_references,
            &body.tree.diagnostics,
            &source[body.base..],
            offset.saturating_sub(body.base),
        )
    {
        return true;
    }
    for symbol in &workspace.files[file].symbols {
        let Some(declaration) = symbol.declaration() else {
            continue;
        };
        let (start, end) = (declaration.start, declaration.end);
        if !(start..=end).contains(&offset) {
            continue;
        }
        if symbol.is_callable() {
            let end = symbol.body().map_or(end, |body| body.start);
            if offset <= end && end - start <= 16 * 1024 {
                let parsed = crate::syntax::parse_signature(&source[start..end]);
                if type_context_at(
                    &parsed.type_references,
                    &parsed.diagnostics,
                    &source[start..end],
                    offset - start,
                ) {
                    return true;
                }
            }
        } else if symbol.is_type() && offset >= symbol.end && end - symbol.end <= 16 * 1024 {
            let mut start = symbol.end;
            if let Some(parameters) = crate::syntax::type_parameter_end(&source[start..end]) {
                if offset < start + parameters {
                    let header = format!("func Generic{}()", &source[start..start + parameters]);
                    let parsed = crate::syntax::parse_signature(&header);
                    if type_context_at(
                        &parsed.type_references,
                        &parsed.diagnostics,
                        &header,
                        offset - start + "func Generic".len(),
                    ) {
                        return true;
                    }
                }
                start += parameters;
            }
            if let Some(token) = Lexer::new(&source[start..end]).next()
                && token.kind == TokenKind::Assign
            {
                start += token.span.end;
            }
            let parsed = crate::syntax::parse_type(&source[start..end]);
            if offset >= start
                && type_context_at(
                    &parsed.references,
                    &parsed.diagnostics,
                    &source[start..end],
                    offset - start,
                )
            {
                return true;
            }
        } else if symbol.is_value() && end - start <= 16 * 1024 {
            let block = format!("{{var {}\n}}", &source[start..end]);
            let parsed = crate::syntax::parse_block(&block);
            if type_context_at(
                &parsed.type_references,
                &parsed.diagnostics,
                &block,
                offset - start + 5,
            ) {
                return true;
            }
        }
    }
    false
}

fn type_context_at(
    references: &[TypeReference],
    diagnostics: &[Diagnostic],
    source: &str,
    offset: usize,
) -> bool {
    if references
        .iter()
        .any(|reference| reference.name.start <= offset && offset <= reference.name.end)
    {
        return true;
    }
    // An unfinished qualifier is still a type position. The syntax parser's
    // specific diagnostic distinguishes it from an expression selector.
    diagnostics.iter().any(|error| {
        if error.message != "expected qualified type name" {
            return false;
        }
        let Some(tail) = source.get(error.span.end..offset) else {
            return false;
        };
        let mut tokens = Lexer::new(tail);
        tokens
            .next()
            .is_some_and(|token| token.kind == TokenKind::Period)
            && tokens.next().is_none()
    })
}
