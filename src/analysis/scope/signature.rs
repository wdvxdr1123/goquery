//! Signature bindings and implicit receiver type parameters from handwritten syntax.
use crate::analysis::scope::{self as locals, Binding, DeclarationKind, Locals, ObjectKind};
use crate::syntax::TokenKind;
use std::ops::Range;

impl Locals {
    /// Names in a function type are declaration-only objects; they must not
    /// resolve to a same-spelled enclosing value or enter the enclosing scope.
    pub(super) fn type_signature_names(
        &mut self,
        source: &str,
        names: &[crate::syntax::SignatureName],
        base: usize,
    ) {
        let remaining = 256usize.saturating_sub(
            self.bindings
                .iter()
                .filter(|binding| binding.scope.is_empty())
                .count(),
        );
        for name in names.iter().take(remaining) {
            let selection = base + name.name.start..base + name.name.end;
            if self
                .bindings
                .iter()
                .any(|binding| binding.selection == selection)
                || &source[selection.clone()] == "_"
            {
                continue;
            }
            let annotation = base + name.annotation.start..base + name.annotation.end;
            self.bindings.push(Binding {
                name: source[selection.clone()].to_owned(),
                selection,
                scope: 0..0,
                visible_from: 0,
                object_kind: ObjectKind::Parameter,
                declaration: if name.variadic {
                    DeclarationKind::Variadic
                } else {
                    DeclarationKind::Value
                },
                declared_type: (!name.variadic)
                    .then(|| locals::named_type(&source[annotation.clone()]))
                    .flatten(),
                annotation: (!name.variadic).then(|| Box::new(annotation)),
                initializer: None,
            });
        }
    }

    pub(super) fn from_signature(source: &str, body_offset: usize) -> Self {
        Self::from_parsed_signature(
            source,
            body_offset,
            &crate::syntax::parse_signature(&source[..body_offset]),
        )
    }

    pub(crate) fn from_header(
        source: &str,
        start: usize,
        body: Range<usize>,
        signature: &crate::syntax::Signature,
    ) -> Self {
        let mut locals =
            Self::from_parsed_signature(&source[start..body.end], body.start - start, signature);
        locals.rebase(start);
        locals
    }

    pub(crate) fn from_parsed_signature(
        source: &str,
        body_offset: usize,
        signature: &crate::syntax::Signature,
    ) -> Self {
        let mut locals = Self::empty();
        let mut implicit = Vec::new();
        for receiver in &signature.receiver {
            let parameters = receiver_parameters(source, &receiver.annotation);
            locals.receiver_constraints.extend(
                parameters
                    .iter()
                    .map(|parameter| (parameter.clone(), receiver.annotation.clone())),
            );
            implicit.extend(parameters);
        }
        let type_names: Vec<_> = signature
            .type_parameters
            .iter()
            .filter_map(|parameter| parameter.name.as_ref())
            .chain(&implicit)
            .map(|name| &source[name.clone()])
            .collect();
        for selection in &implicit {
            locals.bindings.push(Binding {
                name: source[selection.clone()].to_owned(),
                selection: selection.clone(),
                scope: 0..source.len(),
                visible_from: 0,
                object_kind: ObjectKind::TypeParameter,
                declaration: DeclarationKind::TypeParameter,
                declared_type: None,
                annotation: None,
                initializer: None,
            });
        }
        for (parameter, object_kind) in signature
            .type_parameters
            .iter()
            .map(|p| (p, ObjectKind::TypeParameter))
            .chain(
                signature
                    .receiver
                    .iter()
                    .chain(&signature.parameters)
                    .map(|p| (p, ObjectKind::Parameter))
                    .chain(signature.results.iter().map(|p| (p, ObjectKind::Variable))),
            )
        {
            let Some(selection) = &parameter.name else {
                continue;
            };
            let name = &source[selection.clone()];
            if name == "_" {
                continue;
            }
            let declared_type = if object_kind.is_type() || parameter.variadic {
                None
            } else {
                locals::named_type(&source[parameter.annotation.clone()]).filter(|ty| {
                    !type_names.contains(
                        &locals::named_root(ty)
                            .split('.')
                            .next()
                            .expect("named type"),
                    )
                })
            };
            if object_kind.is_type() {
                locals
                    .type_constraints
                    .push((selection.clone(), parameter.annotation.clone()));
            }
            locals.bindings.push(Binding {
                name: name.to_owned(),
                selection: selection.clone(),
                scope: if object_kind.is_type() {
                    0..source.len()
                } else {
                    body_offset..source.len()
                },
                visible_from: if object_kind.is_type() {
                    0
                } else {
                    body_offset
                },
                object_kind,
                declaration: if object_kind.is_type() {
                    DeclarationKind::TypeParameter
                } else if parameter.variadic {
                    DeclarationKind::Variadic
                } else {
                    DeclarationKind::Value
                },
                declared_type,
                annotation: (!object_kind.is_type() && !parameter.variadic)
                    .then(|| Box::new(parameter.annotation.clone())),
                initializer: None,
            });
        }
        locals
    }
}

// Receiver type arguments declare implicit type parameters; they must shadow
// package types even though generic method-set resolution is still pending.
pub(crate) fn receiver_parameters(source: &str, annotation: &Range<usize>) -> Vec<Range<usize>> {
    let mut tokens = crate::syntax::Lexer::new(&source[annotation.clone()]).peekable();
    let mut parentheses = 0usize;
    while let Some(token) = tokens.peek() {
        match token.kind {
            TokenKind::LParen => parentheses += 1,
            TokenKind::Mul => {}
            _ => break,
        }
        tokens.next();
    }
    if !tokens
        .next()
        .is_some_and(|token| token.kind == TokenKind::Ident)
        || !tokens
            .next()
            .is_some_and(|token| token.kind == TokenKind::LBrack)
    {
        return Vec::new();
    }
    let mut names = Vec::new();
    loop {
        let Some(name) = tokens.next().filter(|token| token.kind == TokenKind::Ident) else {
            return Vec::new();
        };
        names.push(annotation.start + name.span.start..annotation.start + name.span.end);
        match tokens.next().map(|token| token.kind) {
            Some(TokenKind::RBrack) => break,
            Some(TokenKind::Comma) => {
                if tokens
                    .peek()
                    .is_some_and(|token| token.kind == TokenKind::RBrack)
                {
                    tokens.next();
                    break;
                }
            }
            _ => return Vec::new(),
        }
    }
    for _ in 0..parentheses {
        if tokens.next().map(|token| token.kind) != Some(TokenKind::RParen) {
            return Vec::new();
        }
    }
    if tokens
        .next()
        .is_some_and(|token| token.kind != TokenKind::Semicolon)
        || tokens.next().is_some()
    {
        return Vec::new();
    }
    names
}
