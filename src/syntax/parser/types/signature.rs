//! Tree-aware parameter and function-type signature parsing for [`TypeSyntax`].
use crate::syntax::ast::signature::Parameter;
use crate::syntax::ast::types::{
    MAX_TYPE_NODES, ParamNode, SigNodeId, SignatureName, SignatureNameKind, TypeNodeId,
    TypeNodeKind, TypeReference, TypeTreeKind,
};
use crate::syntax::parser::expression::Parser;
use crate::syntax::parser::signature::ParameterContext;
use crate::syntax::parser::types::tree::TypeTreeBuilder;
use crate::syntax::token::TokenKind;
use std::ops::Range;

pub(super) struct TypeParameterField {
    pub(super) name: Option<Range<usize>>,
    pub(super) annotation: Option<Range<usize>>,
    pub(super) ty: Option<TypeNodeId>,
    pub(super) variadic: bool,
}

impl Parser<'_> {
    pub(super) fn function_type_signature_tree(
        &mut self,
        depth: usize,
        mut tree: Option<&mut TypeTreeBuilder>,
    ) -> (usize, Option<SigNodeId>) {
        // Function types cannot have type parameters, but go/parser still consumes
        // and reports them before parsing the parameter list. Consuming the list
        // keeps trailing conversions/calls and function-literal bodies recoverable
        // (e.g. `(func [P any]())(nil)` and `func [P any]() {}`).
        if self
            .tokens
            .peek()
            .is_some_and(|token| token.kind == TokenKind::LBrack)
        {
            let start = self
                .tokens
                .peek()
                .expect("peeked type parameter list")
                .span
                .start;
            let (_, end) = self.parameter_list(ParameterContext::TypeParameters, depth);
            self.error(start..end, "function type must have no type parameters");
        }
        let sig_start = self
            .tokens
            .peek()
            .map_or(self.source.len(), |token| token.span.start);
        let (parameters, param_nodes, end) =
            self.type_parameter_list(ParameterContext::Inputs, depth, tree.as_deref_mut());
        let (results, result_nodes, end) =
            self.type_result_parameters(depth, end, tree.as_deref_mut());
        for (parameters, kind) in [
            (parameters, SignatureNameKind::Parameter),
            (results, SignatureNameKind::Result),
        ] {
            self.signature_names
                .extend(parameters.into_iter().filter_map(|parameter| {
                    parameter.name.map(|name| SignatureName {
                        name,
                        kind,
                        annotation: parameter.annotation,
                        variadic: parameter.variadic,
                    })
                }));
        }
        let sig_id = tree.map(|t| {
            let variadic = param_nodes.last().is_some_and(|param| param.variadic);
            t.push_sig(sig_start..end, param_nodes, result_nodes, variadic)
        });
        (end, sig_id)
    }

    fn type_parameter_list(
        &mut self,
        context: ParameterContext,
        depth: usize,
        mut tree: Option<&mut TypeTreeBuilder>,
    ) -> (Vec<Parameter>, Vec<ParamNode>, usize) {
        let start = self
            .tokens
            .peek()
            .map_or(self.source.len(), |token| token.span.start);
        self.expect_kind(TokenKind::LParen, "expected parameter list");
        let mut fields = Vec::new();
        while let Some(token) = self.tokens.peek().cloned() {
            if token.kind == TokenKind::RParen {
                break;
            }
            if matches!(token.kind, TokenKind::Semicolon | TokenKind::RBrace) {
                self.error(token.span, "expected parameter or closing delimiter");
                break;
            }
            fields.push(self.type_parameter_field(depth, tree.as_deref_mut()));
            if self
                .tokens
                .peek()
                .is_some_and(|token| token.kind == TokenKind::Comma)
            {
                self.tokens.next();
            } else {
                break;
            }
            if self
                .tokens
                .peek()
                .is_some_and(|next| next.span.start <= token.span.start)
            {
                self.tokens.next();
                break;
            }
        }
        let end = self.close(
            TokenKind::RParen,
            "expected parameter list closing delimiter",
            start,
        );
        let named = fields
            .iter()
            .any(|field| field.name.is_some() && field.annotation.is_some());
        if named {
            let mut inherited = None;
            for field in fields.iter_mut().rev() {
                if field.annotation.is_some() {
                    inherited = field
                        .annotation
                        .clone()
                        .map(|span| (span, field.ty, field.variadic));
                } else if let Some((span, ty, variadic)) = &inherited {
                    field.annotation = Some(span.clone());
                    field.ty = *ty;
                    field.variadic = *variadic;
                } else {
                    self.error(start..end, "missing parameter type");
                }
                if field.name.is_none() {
                    self.error(start..end, "mixed named and unnamed parameters");
                }
            }
        } else {
            for field in &mut fields {
                if field.annotation.is_none() {
                    field.annotation = field.name.take();
                    if let Some(name) = &field.annotation {
                        self.type_references.push(TypeReference {
                            qualifier: None,
                            name: name.clone(),
                        });
                        // `type_syntax` is bypassed for a bare unnamed type name.
                        self.record_type_node(TypeNodeKind::Name, name.clone());
                        if let Some(t) = tree.as_deref_mut() {
                            let span = TypeTreeBuilder::span32(name.clone());
                            field.ty = Some(t.push_node(TypeTreeKind::Name { span }, name.clone()));
                        }
                    }
                }
            }
        }
        let count = fields.len();
        let mut parameters = Vec::with_capacity(count);
        let mut param_nodes = Vec::new();
        for (index, field) in fields.into_iter().enumerate() {
            if field.variadic && (context != ParameterContext::Inputs || index + 1 != count) {
                self.error(start..end, "variadic type requires final input parameter");
            }
            if let Some(t) = tree.as_deref_mut() {
                if t.is_overflowed() || param_nodes.len() >= MAX_TYPE_NODES {
                    t.mark_overflow();
                } else {
                    let ty = field.ty.unwrap_or_else(|| t.push_missing(end..end));
                    param_nodes.push(ParamNode {
                        name: field.name.clone().map(TypeTreeBuilder::span32),
                        ty,
                        variadic: field.variadic,
                    });
                }
            }
            parameters.push(Parameter {
                name: field.name,
                annotation: field.annotation.unwrap_or(end..end),
                variadic: field.variadic,
            });
        }
        (parameters, param_nodes, end)
    }

    fn type_result_parameters(
        &mut self,
        depth: usize,
        fallback: usize,
        mut tree: Option<&mut TypeTreeBuilder>,
    ) -> (Vec<Parameter>, Vec<ParamNode>, usize) {
        if self
            .tokens
            .peek()
            .is_some_and(|token| token.kind == TokenKind::LParen)
        {
            self.type_parameter_list(ParameterContext::Results, depth, tree)
        } else if self
            .tokens
            .peek()
            .is_some_and(|token| starts_type(token.kind))
        {
            let (annotation, ty) = self.type_with_inference(depth + 1, true, tree.as_deref_mut());
            let end = annotation.end;
            let param_nodes = if let Some(t) = tree {
                let ty = ty.unwrap_or_else(|| t.push_missing(end..end));
                vec![ParamNode {
                    name: None,
                    ty,
                    variadic: false,
                }]
            } else {
                Vec::new()
            };
            (
                vec![Parameter {
                    name: None,
                    annotation,
                    variadic: false,
                }],
                param_nodes,
                end,
            )
        } else {
            (Vec::new(), Vec::new(), fallback)
        }
    }

    pub(super) fn type_parameter_field(
        &mut self,
        depth: usize,
        tree: Option<&mut TypeTreeBuilder>,
    ) -> TypeParameterField {
        let saved = self.tokens.clone();
        let mut name = None;
        if self
            .tokens
            .peek()
            .is_some_and(|token| token.kind == TokenKind::Ident)
        {
            let ident = self.tokens.next().expect("peeked parameter identifier");
            let next = self.tokens.peek().map(|token| token.kind);
            let unnamed = next == Some(TokenKind::Period)
                || next == Some(TokenKind::LBrack) && !self.type_bracket_followed_by_type();
            if unnamed {
                self.tokens = saved;
            } else {
                name = Some(ident.span);
                if !self.tokens.peek().is_some_and(|token| {
                    starts_type(token.kind) || token.kind == TokenKind::Ellipsis
                }) {
                    return TypeParameterField {
                        name,
                        annotation: None,
                        ty: None,
                        variadic: false,
                    };
                }
            }
        }
        let variadic = self
            .tokens
            .peek()
            .is_some_and(|token| token.kind == TokenKind::Ellipsis);
        if variadic {
            self.tokens.next();
        }
        let inferred_array = !(name.is_some()
            && self
                .tokens
                .peek()
                .is_some_and(|token| token.kind == TokenKind::LBrack));
        let (annotation, ty) = self.type_with_inference(depth + 1, inferred_array, tree);
        TypeParameterField {
            name,
            annotation: Some(annotation),
            ty,
            variadic,
        }
    }

    fn type_bracket_followed_by_type(&self) -> bool {
        let mut tokens = self.tokens.clone();
        let mut depth = 0;
        for token in tokens.by_ref() {
            match token.kind {
                TokenKind::LBrack => depth += 1,
                TokenKind::RBrack => {
                    depth -= 1;
                    if depth == 0 {
                        return tokens.next().is_some_and(|token| starts_type(token.kind));
                    }
                }
                _ => {}
            }
        }
        false
    }
}

fn starts_type(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Ident
            | TokenKind::Mul
            | TokenKind::Arrow
            | TokenKind::Func
            | TokenKind::Chan
            | TokenKind::Map
            | TokenKind::Struct
            | TokenKind::Interface
            | TokenKind::LParen
            | TokenKind::LBrack
    )
}
