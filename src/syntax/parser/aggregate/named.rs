//! Streaming named alias/underlying targets; no recursive type tree.
use crate::syntax::ast::TypeTarget;
use crate::syntax::ast::expression::{ExprId, ExprKind};
use crate::syntax::lexer::Lexer;
use crate::syntax::parser::expression::Parser;
use crate::syntax::token::{Token, TokenKind};

/// Token bound for the type-parameter ambiguity decision.
const MAX_PARAMETER_TOKENS: usize = 8192;

pub(super) fn target(source: &str) -> Option<TypeTarget> {
    let mut tokens = Lexer::new(source).take(8192).peekable();
    let generic = tokens.peek()?.kind == TokenKind::LBrack;
    if generic {
        let end = parameter_end(source)?;
        while tokens.peek().is_some_and(|token| token.span.start < end) {
            tokens.next();
        }
    }
    let alias = tokens.peek()?.kind == TokenKind::Assign;
    if alias {
        tokens.next();
    }
    let mut name = String::new();
    if tokens.peek()?.kind == TokenKind::Mul {
        tokens.next();
        name.push('*');
    }
    let first = tokens
        .next()
        .filter(|token| token.kind == TokenKind::Ident)?;
    if first.span.len() > 16 * 1024 {
        return None;
    }
    name.push_str(&source[first.span]);
    if tokens
        .peek()
        .is_some_and(|token| token.kind == TokenKind::Period)
    {
        tokens.next();
        let field = tokens
            .next()
            .filter(|token| token.kind == TokenKind::Ident)?;
        if name.len().saturating_add(field.span.len()) >= 16 * 1024 {
            return None;
        }
        name.push('.');
        name.push_str(&source[field.span]);
    }
    if name.len() > 16 * 1024 {
        return None;
    }
    if tokens
        .peek()
        .is_some_and(|token| token.kind == TokenKind::LBrack)
    {
        tokens.next();
        name.push('[');
        let mut depth = 1usize;
        for token in tokens.by_ref() {
            match token.kind {
                TokenKind::LBrack => depth += 1,
                TokenKind::RBrack => depth -= 1,
                _ => {}
            }
            name.push(' ');
            if token.kind == TokenKind::Semicolon {
                name.push(';');
            } else {
                name.push_str(&source[token.span]);
            }
            if name.len() > 16 * 1024 {
                return None;
            }
            if depth == 0 {
                break;
            }
        }
        if depth != 0 {
            return None;
        }
    }
    if tokens.any(|token| token.kind != TokenKind::Semicolon) {
        return None;
    }
    Some(TypeTarget {
        name,
        alias,
        generic,
    })
}

/// Decides whether a leading `[...]` prefix is a declaration type-parameter list
/// or array/slice type syntax, following go/parser's `parseTypeSpec` rules:
/// `extractName` splits the expression starting at the name and the split is
/// accepted when a parameter type was found, or the token after it is not `]`.
pub(super) fn parameter_end(source: &str) -> Option<usize> {
    let mut tokens: Vec<Token> = Vec::new();
    let mut depth = 0usize;
    let mut closed = false;
    for token in Lexer::new(source).take(MAX_PARAMETER_TOKENS + 1) {
        match token.kind {
            TokenKind::LBrack => depth += 1,
            TokenKind::RBrack => {
                depth = depth.checked_sub(1)?;
            }
            _ => {}
        }
        tokens.push(token);
        if depth == 0 {
            closed = true;
            break;
        }
    }
    let closing = tokens.last()?;
    if !closed || closing.kind != TokenKind::RBrack {
        return None;
    }
    let first = tokens.get(1)?;
    if first.kind != TokenKind::Ident {
        return None;
    }
    // A name immediately followed by `[` starts a type-parameter list: index and
    // slice expressions are not constant array lengths.
    if tokens
        .get(2)
        .is_some_and(|token| token.kind == TokenKind::LBrack)
    {
        return Some(closing.span.end);
    }
    let text = source.get(first.span.start..closing.span.start)?;
    let mut parser = Parser::new(text);
    let root = parser.expression(1, 0);
    if !parser.diagnostics.is_empty() {
        // Malformed expression prefixes keep the conservative token proof so
        // recovery still recognizes common constraint spellings.
        return parameter_end_conservative(&tokens).then_some(closing.span.end);
    }
    let force = parser
        .tokens
        .peek()
        .is_some_and(|token| token.kind == TokenKind::Comma);
    // The bounded content slice lexes with an inserted EOF semicolon; either a
    // missing token or that semicolon corresponds to go/parser's closing `]`.
    let at_closing_bracket = parser
        .tokens
        .peek()
        .is_none_or(|token| token.kind == TokenKind::Semicolon);
    let (_, parameter_type) = extract_name(&parser, root, force)?;
    (parameter_type.is_some() || !at_closing_bracket).then_some(closing.span.end)
}

/// go/parser's `extractName`: splits `name`, `name *type`, `name | type` or
/// `name(type)` into the parameter name and an optional constraint type.
fn extract_name(parser: &Parser<'_>, id: ExprId, force: bool) -> Option<(ExprId, Option<ExprId>)> {
    match &parser.nodes[id.0].kind {
        ExprKind::Name => Some((id, None)),
        ExprKind::Binary {
            left,
            operator,
            right,
        } => match &parser.source[operator.clone()] {
            "*" => {
                if matches!(&parser.nodes[left.0].kind, ExprKind::Name)
                    && (force || is_type_elem(parser, *right))
                {
                    Some((*left, Some(*right)))
                } else {
                    None
                }
            }
            "|" => {
                let (name, split) =
                    extract_name(parser, *left, force || is_type_elem(parser, *right))?;
                // The OR form only proves a parameter type when the left side
                // already split into name and type.
                split.is_some().then_some((name, Some(*right)))
            }
            _ => None,
        },
        ExprKind::Call {
            function,
            arguments,
            variadic,
        } => {
            if !*variadic
                && arguments.len() == 1
                && matches!(&parser.nodes[function.0].kind, ExprKind::Name)
                && (force || is_type_elem(parser, arguments[0]))
            {
                Some((*function, Some(arguments[0])))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// go/parser's `isTypeElem`: only type syntax and `~T` prove a constraint.
fn is_type_elem(parser: &Parser<'_>, id: ExprId) -> bool {
    match &parser.nodes[id.0].kind {
        ExprKind::Type => true,
        ExprKind::Unary { operator, .. } => &parser.source[operator.clone()] == "~",
        ExprKind::Binary { left, right, .. } => {
            is_type_elem(parser, *left) || is_type_elem(parser, *right)
        }
        ExprKind::Parenthesized { expression } => is_type_elem(parser, *expression),
        _ => false,
    }
}

/// Conservative token proof used only when the expression prefix is malformed.
fn parameter_end_conservative(tokens: &[Token]) -> bool {
    let mut iter = tokens.iter().skip(1);
    let Some(first) = iter.next() else {
        return false;
    };
    if first.kind != TokenKind::Ident {
        return false;
    }
    let mut stack = vec![TokenKind::RBrack];
    let mut previous = first.kind;
    let mut proven = false;
    for token in iter {
        let kind = token.kind;
        if stack.len() == 1 {
            proven |= (kind == TokenKind::Ident
                && matches!(previous, TokenKind::Ident | TokenKind::RBrack))
                || matches!(
                    kind,
                    TokenKind::Tilde
                        | TokenKind::Interface
                        | TokenKind::Struct
                        | TokenKind::Map
                        | TokenKind::Chan
                        | TokenKind::Func
                );
            if kind == TokenKind::RBrack {
                return proven || previous == TokenKind::Comma;
            }
        }
        match kind {
            TokenKind::LParen => stack.push(TokenKind::RParen),
            TokenKind::LBrack => stack.push(TokenKind::RBrack),
            TokenKind::LBrace => stack.push(TokenKind::RBrace),
            TokenKind::RParen | TokenKind::RBrack | TokenKind::RBrace
                if stack.pop() != Some(kind) =>
            {
                return false;
            }
            _ => {}
        }
        if stack.len() > 128 {
            return false;
        }
        if stack.len() == 1 {
            previous = kind;
        }
    }
    false
}
