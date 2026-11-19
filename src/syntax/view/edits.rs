//! Token boundaries for conservative source edits.
use crate::syntax::{Lexer, Token, TokenKind};
use std::ops::Range;

/// Deletion range for the `Remove variable` quickfix gopls offers for a local
/// declaration that occupies exactly one line. Both `name := value` and
/// `var name type` shapes qualify; multi-name declarations and statements that
/// continue past the line are left alone because gopls rewrites those
/// differently.
pub(crate) fn variable_removal(source: &str, span: &Range<usize>) -> Option<Range<usize>> {
    let mut tokens = Lexer::new(source);
    let mut previous: Option<Token> = None;
    let mut before_previous: Option<Token> = None;
    while let Some(token) = tokens.next() {
        if token.kind != TokenKind::Ident || token.span != *span {
            before_previous = previous.replace(token);
            continue;
        }
        let following = tokens.next();
        let starts_short_declaration = following
            .as_ref()
            .is_some_and(|next| next.kind == TokenKind::Define);
        let starts_var_declaration = previous
            .as_ref()
            .is_some_and(|keyword| keyword.kind == TokenKind::Var)
            && before_previous
                .as_ref()
                .is_some_and(|start| statement_boundary(start.kind));
        let mut single_name = true;
        let keyword_start = if starts_short_declaration {
            span.start
        } else if starts_var_declaration {
            if following
                .as_ref()
                .is_some_and(|next| next.kind == TokenKind::Comma)
            {
                single_name = false;
            }
            previous
                .as_ref()
                .map_or(span.start, |keyword| keyword.span.start)
        } else {
            return None;
        };
        if !single_name {
            return None;
        }
        let start_of_line = line_start(source, keyword_start);
        if !source
            .get(start_of_line..keyword_start)
            .is_some_and(|prefix| prefix.trim().is_empty())
        {
            return None;
        }
        let mut statement_end = None;
        for inner in tokens.by_ref() {
            match inner.kind {
                TokenKind::Semicolon => {
                    statement_end = Some(inner.span.start);
                    break;
                }
                TokenKind::Comma => single_name = false,
                _ => {}
            }
        }
        if !single_name {
            return None;
        }
        let statement_end = statement_end?;
        if source[span.start..statement_end].contains('\n') {
            return None;
        }
        let end_of_line = source
            .get(statement_end..)
            .and_then(|rest| rest.find('\n').map(|newline| statement_end + newline))?;
        if !source
            .get(statement_end..end_of_line)
            .is_some_and(|tail| tail.trim().is_empty())
        {
            return None;
        }
        return Some(start_of_line..end_of_line + 1);
    }
    None
}

fn line_start(source: &str, offset: usize) -> usize {
    source
        .get(..offset)
        .and_then(|prefix| prefix.rfind('\n').map(|newline| newline + 1))
        .unwrap_or(0)
}

fn statement_boundary(previous: TokenKind) -> bool {
    matches!(
        previous,
        TokenKind::Semicolon
            | TokenKind::LBrace
            | TokenKind::RBrace
            | TokenKind::Colon
            | TokenKind::For
            | TokenKind::If
            | TokenKind::Switch
            | TokenKind::Case
            | TokenKind::Default
            | TokenKind::Else
    )
}
