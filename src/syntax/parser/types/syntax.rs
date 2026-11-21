//! Type syntax consumed on demand with a compact `u32`-indexed AST tree.
use crate::syntax::ast::types::{
    ChanDir, MAX_TYPE_NODES, TypeNodeId, TypeNodeKind, TypeReference, TypeSyntax, TypeTreeKind,
};
use crate::syntax::parser::expression::Parser;
use crate::syntax::parser::types::tree::TypeTreeBuilder;
use crate::syntax::token::TokenKind;
use std::ops::Range;

/// Parses one type, collecting name spans, array-length expressions, and a
/// compact indexed type AST.
pub fn parse_type(source: &str) -> TypeSyntax {
    parse(source, End::Complete, false)
}

/// Parses one type constraint element (allowing `~` and `|` unions directly),
/// collecting name spans without requiring synthetic `interface{...}` wrapping.
pub fn parse_constraint(source: &str) -> TypeSyntax {
    parse(source, End::Complete, true)
}

/// Parses the first complete type in a larger token stream; span ends before
/// the following delimiter. Diagnostics still describe the complete prefix.
pub fn parse_type_prefix(source: &str) -> TypeSyntax {
    parse(source, End::Prefix, false)
}

#[derive(PartialEq, Eq)]
enum End {
    Complete,
    Prefix,
}

fn parse(source: &str, end: End, constraint: bool) -> TypeSyntax {
    let mut parser = Parser::new(source);
    let mut builder = TypeTreeBuilder::new();
    let (span, root) = if constraint {
        parser.type_elem_tree(0, Some(&mut builder))
    } else {
        parser.type_with_inference(0, true, Some(&mut builder))
    };
    while parser
        .tokens
        .peek()
        .is_some_and(|token| token.kind == TokenKind::Semicolon)
    {
        parser.tokens.next();
    }
    if end == End::Complete
        && let Some(token) = parser.tokens.next()
    {
        parser.error(token.span, "unexpected token after type");
    }
    let tree = builder.finish(root);
    TypeSyntax {
        span,
        references: parser.type_references,
        signature_names: parser.signature_names,
        fields: parser.fields,
        interface_members: parser.interface_members,
        expressions: parser.nodes,
        diagnostics: parser.diagnostics,
        type_nodes: parser.type_nodes,
        tree_root: tree.root,
        tree_nodes: tree.nodes,
        field_nodes: tree.fields,
        interface_member_nodes: tree.interface_members,
        sig_nodes: tree.sigs,
        param_nodes: tree.params,
        type_node_lists: tree.lists,
        tree_overflowed: tree.overflowed,
    }
}

impl Parser<'_> {
    pub(in crate::syntax::parser) fn type_syntax(&mut self, depth: usize) -> Range<usize> {
        self.type_with_inference(depth, true, None).0
    }

    pub(in crate::syntax::parser) fn array_field_type(&mut self, depth: usize) -> Range<usize> {
        self.type_with_inference(depth, false, None).0
    }

    pub(super) fn type_with_inference(
        &mut self,
        depth: usize,
        inferred_array: bool,
        mut tree: Option<&mut TypeTreeBuilder>,
    ) -> (Range<usize>, Option<TypeNodeId>) {
        let Some(token) = self.tokens.peek().cloned() else {
            let end = self.source.len();
            self.error(end..end, "expected type");
            let id = tree.map(|t| t.push_missing(end..end));
            return (end..end, id);
        };
        if depth >= 128 {
            self.tokens.next();
            self.error(token.span.clone(), "type nesting limit exceeded");
            let id = tree.map(|t| t.push_missing(token.span.clone()));
            return (token.span, id);
        }
        let start = token.span.start;
        let spelling = &self.source[token.span.clone()];
        let (end, id) = match (token.kind, spelling) {
            (TokenKind::Mul, _) => {
                self.tokens.next();
                let node = self.begin_type_node(TypeNodeKind::Pointer, start);
                let (inner, inner_id) =
                    self.type_with_inference(depth + 1, true, tree.as_deref_mut());
                let end = inner.end;
                self.end_type_node(node, end);
                let id = tree
                    .zip(inner_id)
                    .map(|(t, elem)| t.push_node(TypeTreeKind::Pointer(elem), start..end));
                (end, id)
            }
            (TokenKind::LParen, _) => {
                self.tokens.next();
                let node = self.begin_type_node(TypeNodeKind::Parenthesized, start);
                let (inner, inner_id) =
                    self.type_with_inference(depth + 1, true, tree.as_deref_mut());
                let end = self.close(TokenKind::RParen, "expected ')' in type", inner.end);
                self.end_type_node(node, end);
                let id = tree
                    .zip(inner_id)
                    .map(|(t, inner)| t.push_node(TypeTreeKind::Parenthesized(inner), start..end));
                (end, id)
            }
            (TokenKind::LBrack, _) => {
                self.tokens.next();
                let inferred_length = self.eat_spelling("...");
                if inferred_length && !inferred_array {
                    self.error(
                        token.span.clone(),
                        "inferred array length is not allowed in a named field",
                    );
                }
                let slice = !inferred_length
                    && self
                        .tokens
                        .peek()
                        .is_some_and(|t| t.kind == TokenKind::RBrack);
                let node = self.begin_type_node(
                    if slice {
                        TypeNodeKind::Slice
                    } else {
                        TypeNodeKind::Array
                    },
                    start,
                );
                let len_expr = if !inferred_length && !slice {
                    Some(self.expression_without_type_nodes(1, depth + 1))
                } else {
                    None
                };
                self.close(
                    TokenKind::RBrack,
                    "expected ']' in array type",
                    token.span.end,
                );
                let (elem_span, elem_id) =
                    self.type_with_inference(depth + 1, true, tree.as_deref_mut());
                let end = elem_span.end;
                self.end_type_node(node, end);
                let id = tree.zip(elem_id).map(|(t, elem)| {
                    let kind = if slice {
                        TypeTreeKind::Slice(elem)
                    } else {
                        TypeTreeKind::Array { len_expr, elem }
                    };
                    t.push_node(kind, start..end)
                });
                (end, id)
            }
            (TokenKind::Map, _) => {
                self.tokens.next();
                let node = self.begin_type_node(TypeNodeKind::Map, start);
                self.expect_kind(TokenKind::LBrack, "expected '[' after map");
                let (key_span, key_id) =
                    self.type_with_inference(depth + 1, true, tree.as_deref_mut());
                self.close(
                    TokenKind::RBrack,
                    "expected ']' after map key",
                    key_span.end,
                );
                let (val_span, val_id) =
                    self.type_with_inference(depth + 1, true, tree.as_deref_mut());
                let end = val_span.end;
                self.end_type_node(node, end);
                let id = tree.and_then(|t| {
                    let key = key_id?;
                    let value = val_id?;
                    Some(t.push_node(TypeTreeKind::Map { key, value }, start..end))
                });
                (end, id)
            }
            (TokenKind::Chan, _) | (TokenKind::Arrow, _) => {
                self.tokens.next();
                let (kind, dir) = if spelling == "<-" {
                    if !self.eat_spelling("chan") {
                        self.error(token.span.clone(), "expected chan after '<-' in type");
                    }
                    (TypeNodeKind::RecvChan, ChanDir::Recv)
                } else if self.eat_spelling("<-") {
                    (TypeNodeKind::SendChan, ChanDir::Send)
                } else {
                    (TypeNodeKind::Chan, ChanDir::Both)
                };
                let node = self.begin_type_node(kind, start);
                let (elem_span, elem_id) =
                    self.type_with_inference(depth + 1, true, tree.as_deref_mut());
                let end = elem_span.end;
                self.end_type_node(node, end);
                let id = tree
                    .zip(elem_id)
                    .map(|(t, elem)| t.push_node(TypeTreeKind::Chan { dir, elem }, start..end));
                (end, id)
            }
            (TokenKind::Struct | TokenKind::Interface, _) => {
                self.tokens.next();
                let interface = token.kind == TokenKind::Interface;
                let node = self.begin_type_node(
                    if interface {
                        TypeNodeKind::Interface
                    } else {
                        TypeNodeKind::Struct
                    },
                    start,
                );
                let (end, kind) =
                    self.aggregate_type_tree(interface, depth + 1, tree.as_deref_mut());
                self.end_type_node(node, end);
                let id = tree
                    .zip(kind)
                    .map(|(t, kind)| t.push_node(kind, start..end));
                (end, id)
            }
            (TokenKind::Func, _) => {
                self.tokens.next();
                let node = self.begin_type_node(TypeNodeKind::Function, start);
                let (end, sig_id) =
                    self.function_type_signature_tree(depth + 1, tree.as_deref_mut());
                self.end_type_node(node, end);
                let id = tree
                    .zip(sig_id)
                    .map(|(t, sig)| t.push_node(TypeTreeKind::Func(sig), start..end));
                (end, id)
            }
            (TokenKind::Ident, _) => {
                self.tokens.next();
                let mut end = token.span.end;
                let mut qualifier = None;
                let name = token.span.clone();
                let mut id = None;
                if self
                    .tokens
                    .peek()
                    .is_some_and(|t| t.kind == TokenKind::Period)
                {
                    self.tokens.next();
                    qualifier = Some(token.span.clone());
                    if let Some(name) = self
                        .tokens
                        .peek()
                        .filter(|t| t.kind == TokenKind::Ident)
                        .cloned()
                    {
                        self.tokens.next();
                        end = name.span.end;
                        // Keep the final component distinct from the package alias.
                        self.type_references.push(TypeReference {
                            qualifier: qualifier.clone(),
                            name: name.span.clone(),
                        });
                        self.record_type_node(TypeNodeKind::Qualified, start..end);
                        if let Some(t) = tree.as_deref_mut() {
                            let qual = TypeTreeBuilder::span32(token.span.clone());
                            let ident = TypeTreeBuilder::span32(name.span);
                            id = Some(t.push_node(
                                TypeTreeKind::Qualified { qual, name: ident },
                                start..end,
                            ));
                        }
                    } else {
                        self.error(end..end, "expected qualified type name");
                        if let Some(t) = tree.as_deref_mut() {
                            id = Some(t.push_missing(start..end));
                        }
                    }
                }
                if qualifier.is_none() {
                    self.type_references.push(TypeReference {
                        qualifier: None,
                        name,
                    });
                    self.record_type_node(TypeNodeKind::Name, token.span.clone());
                    if let Some(t) = tree.as_deref_mut() {
                        let span = TypeTreeBuilder::span32(token.span.clone());
                        id = Some(t.push_node(TypeTreeKind::Name { span }, token.span));
                    }
                }
                if self
                    .tokens
                    .peek()
                    .is_some_and(|t| t.kind == TokenKind::LBrack)
                {
                    self.tokens.next();
                    let node = self.begin_type_node(TypeNodeKind::Instantiation, start);
                    let mut count = 0;
                    let mut local_args = Vec::new();
                    while self.tokens.peek().is_some_and(|t| {
                        !matches!(
                            t.kind,
                            TokenKind::RBrack | TokenKind::Semicolon | TokenKind::RParen
                        )
                    }) {
                        let (argument, arg_id) =
                            self.type_with_inference(depth + 1, true, tree.as_deref_mut());
                        end = argument.end;
                        count += 1;
                        if let (Some(t), Some(arg_id)) = (tree.as_deref_mut(), arg_id) {
                            if t.is_overflowed() || local_args.len() >= MAX_TYPE_NODES {
                                t.mark_overflow();
                            } else {
                                local_args.push(arg_id);
                            }
                        }
                        if !self.eat_spelling(",") {
                            break;
                        }
                    }
                    if count == 0 {
                        self.error(end..end, "expected type argument");
                    }
                    end = self.close(TokenKind::RBrack, "expected ']' after type arguments", end);
                    self.end_type_node(node, end);
                    if let (Some(t), Some(base)) = (tree, id) {
                        let args = t.push_type_list(local_args);
                        id = Some(
                            t.push_node(TypeTreeKind::Instantiation { base, args }, start..end),
                        );
                    }
                }
                (end, id)
            }
            _ => {
                self.error(token.span.clone(), "expected type");
                if !matches!(
                    token.kind,
                    TokenKind::RParen | TokenKind::RBrack | TokenKind::Comma | TokenKind::Semicolon
                ) {
                    self.tokens.next();
                }
                let id = tree.map(|t| t.push_missing(start..start));
                (start, id)
            }
        };
        (start..end, id)
    }

    fn eat_spelling(&mut self, spelling: &str) -> bool {
        if self
            .tokens
            .peek()
            .is_some_and(|t| &self.source[t.span.clone()] == spelling)
        {
            self.tokens.next();
            true
        } else {
            false
        }
    }

    pub(in crate::syntax::parser) fn expect_kind(
        &mut self,
        kind: TokenKind,
        message: &'static str,
    ) {
        if self.tokens.peek().is_some_and(|t| t.kind == kind) {
            self.tokens.next();
        } else {
            let span = self
                .tokens
                .peek()
                .map_or(self.source.len()..self.source.len(), |t| t.span.clone());
            self.error(span, message);
        }
    }

    // Opaque function-literal bodies are scanned iteratively.
    pub(in crate::syntax::parser) fn type_group(
        &mut self,
        opening: TokenKind,
        closing: TokenKind,
    ) -> usize {
        let start = self
            .tokens
            .peek()
            .map_or(self.source.len(), |t| t.span.start);
        if !self.tokens.peek().is_some_and(|t| t.kind == opening) {
            self.error(start..start, "expected type member group");
            return start;
        }
        self.tokens.next();
        let mut closings = vec![closing];
        while let Some(token) = self.tokens.next() {
            match token.kind {
                TokenKind::LParen => closings.push(TokenKind::RParen),
                TokenKind::LBrack => closings.push(TokenKind::RBrack),
                TokenKind::LBrace => closings.push(TokenKind::RBrace),
                TokenKind::RParen | TokenKind::RBrack | TokenKind::RBrace => {
                    if closings.pop() != Some(token.kind) {
                        self.error(token.span.clone(), "mismatched type delimiter");
                    }
                    if closings.is_empty() {
                        return token.span.end;
                    }
                }
                TokenKind::Illegal => self.error(token.span, "invalid token in type"),
                _ => {}
            }
        }
        self.error(
            self.source.len()..self.source.len(),
            "unclosed type member group",
        );
        self.source.len()
    }
}
