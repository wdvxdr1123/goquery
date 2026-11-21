//! Struct fields, interface methods and type terms retain compact name spans.
use crate::syntax::ast::types::{
    FieldNode, InterfaceMember, InterfaceMemberNode, MAX_TYPE_NODES, TypeField, TypeNodeId,
    TypeNodeKind, TypeReference, TypeTreeKind,
};
use crate::syntax::parser::expression::Parser;
use crate::syntax::parser::types::tree::TypeTreeBuilder;
use crate::syntax::token::TokenKind;
use std::ops::Range;

impl Parser<'_> {
    pub(super) fn aggregate_type_tree(
        &mut self,
        interface: bool,
        depth: usize,
        mut tree: Option<&mut TypeTreeBuilder>,
    ) -> (usize, Option<TypeTreeKind>) {
        let start = self
            .tokens
            .peek()
            .map_or(self.source.len(), |token| token.span.start);
        self.expect_kind(TokenKind::LBrace, "expected aggregate body");
        let mut local_fields = Vec::new();
        let mut local_members = Vec::new();
        while let Some(token) = self.tokens.peek().cloned() {
            if token.kind == TokenKind::RBrace {
                break;
            }
            if token.kind == TokenKind::Semicolon {
                self.tokens.next();
                continue;
            }
            if interface {
                self.interface_member_tree(depth, start, tree.as_deref_mut(), &mut local_members);
            } else {
                self.struct_field_tree(depth, start, tree.as_deref_mut(), &mut local_fields);
            }
            if self
                .tokens
                .peek()
                .is_some_and(|token| token.kind == TokenKind::Semicolon)
            {
                self.tokens.next();
            } else if !self
                .tokens
                .peek()
                .is_some_and(|token| token.kind == TokenKind::RBrace)
            {
                self.error(token.span.clone(), "expected aggregate member separator");
                while self.tokens.peek().is_some_and(|token| {
                    !matches!(token.kind, TokenKind::Semicolon | TokenKind::RBrace)
                }) {
                    self.tokens.next();
                }
            }
            if self
                .tokens
                .peek()
                .is_some_and(|next| next.span.start <= token.span.start)
            {
                self.tokens.next();
                self.error(token.span, "invalid aggregate member");
            }
        }
        let end = self.close(TokenKind::RBrace, "expected '}' after aggregate", start);
        let kind = tree.map(|t| {
            if interface {
                let members = t.push_interface_members(local_members);
                TypeTreeKind::Interface { members }
            } else {
                let fields = t.push_fields(local_fields);
                TypeTreeKind::Struct { fields }
            }
        });
        (end, kind)
    }

    fn struct_field_tree(
        &mut self,
        depth: usize,
        container: usize,
        mut tree: Option<&mut TypeTreeBuilder>,
        local_fields: &mut Vec<FieldNode>,
    ) {
        let first = self.tokens.peek().expect("struct member").clone();
        if !matches!(first.kind, TokenKind::Ident | TokenKind::Mul)
            || first.kind == TokenKind::Mul
                && !self
                    .tokens
                    .clone()
                    .nth(1)
                    .is_some_and(|token| token.kind == TokenKind::Ident)
        {
            self.error(
                first.span.clone(),
                "expected field name or embedded named type",
            );
        }
        let mut field = self.type_parameter_field(depth, tree.as_deref_mut());
        let mut names: Vec<_> = field.name.iter().cloned().collect();
        let mut grouped = false;
        while field.annotation.is_none()
            && self
                .tokens
                .peek()
                .is_some_and(|token| token.kind == TokenKind::Comma)
        {
            grouped = true;
            self.tokens.next();
            field = self.type_parameter_field(depth, tree.as_deref_mut());
            names.extend(field.name.iter().cloned());
        }
        let mut bare_embedded_ty = None;
        if grouped && (field.annotation.is_none() || field.name.is_none()) {
            self.error(first.span.clone(), "expected type after field names");
        } else if field.annotation.is_none()
            && let Some(name) = field.name.clone()
        {
            self.type_references.push(TypeReference {
                qualifier: None,
                name: name.clone(),
            });
            // A bare embedded type name does not pass through `type_syntax`.
            self.record_type_node(TypeNodeKind::Name, name.clone());
            if let Some(t) = tree.as_deref_mut() {
                let span = TypeTreeBuilder::span32(name.clone());
                bare_embedded_ty = Some(t.push_node(TypeTreeKind::Name { span }, name));
            }
        }
        if field.variadic {
            self.error(first.span.clone(), "struct field cannot be variadic");
        }
        let tag = if self
            .tokens
            .peek()
            .is_some_and(|token| token.kind == TokenKind::String)
        {
            self.tokens.next().map(|token| token.span)
        } else {
            None
        };
        if let Some(t) = tree {
            let tag32 = tag.clone().map(TypeTreeBuilder::span32);
            if grouped && field.annotation.is_none() {
                let missing_ty = t.push_missing(first.span);
                for name in &names {
                    if t.is_overflowed() || local_fields.len() >= MAX_TYPE_NODES {
                        t.mark_overflow();
                        break;
                    }
                    local_fields.push(FieldNode {
                        name: Some(TypeTreeBuilder::span32(name.clone())),
                        ty: missing_ty,
                        embedded: false,
                        tag: tag32.clone(),
                    });
                }
            } else if let Some(ty) = bare_embedded_ty {
                if t.is_overflowed() || local_fields.len() >= MAX_TYPE_NODES {
                    t.mark_overflow();
                } else {
                    local_fields.push(FieldNode {
                        name: field.name.clone().map(TypeTreeBuilder::span32),
                        ty,
                        embedded: true,
                        tag: tag32,
                    });
                }
            } else if let Some(ann) = field.annotation.clone() {
                let ty = field.ty.unwrap_or_else(|| t.push_missing(ann));
                if names.is_empty() {
                    if t.is_overflowed() || local_fields.len() >= MAX_TYPE_NODES {
                        t.mark_overflow();
                    } else {
                        let name = t.embedded_field_name(ty);
                        local_fields.push(FieldNode {
                            name,
                            ty,
                            embedded: true,
                            tag: tag32,
                        });
                    }
                } else {
                    for name in &names {
                        if t.is_overflowed() || local_fields.len() >= MAX_TYPE_NODES {
                            t.mark_overflow();
                            break;
                        }
                        local_fields.push(FieldNode {
                            name: Some(TypeTreeBuilder::span32(name.clone())),
                            ty,
                            embedded: false,
                            tag: tag32.clone(),
                        });
                    }
                }
            }
        }
        let embedded = names.is_empty() || field.annotation.is_none();
        let annotation = field.annotation.or(field.name);
        if let Some(annotation) = annotation {
            if embedded {
                // The implicit name is the written type identifier, before any
                // generic arguments; qualifiers and a leading '*' are excluded.
                names = crate::syntax::lexer::Lexer::new(&self.source[annotation.clone()])
                    .take_while(|token| token.kind != TokenKind::LBrack)
                    .filter(|token| token.kind == TokenKind::Ident)
                    .last()
                    .map(|token| {
                        annotation.start + token.span.start..annotation.start + token.span.end
                    })
                    .into_iter()
                    .collect();
            }
            self.fields.extend(names.into_iter().map(|name| TypeField {
                name,
                annotation: annotation.clone(),
                container,
                embedded,
                tag: tag.clone(),
            }));
        }
    }

    pub(super) fn type_elem_tree(
        &mut self,
        depth: usize,
        mut tree: Option<&mut TypeTreeBuilder>,
    ) -> (Range<usize>, Option<TypeNodeId>) {
        let start = self
            .tokens
            .peek()
            .map_or(self.source.len(), |token| token.span.start);
        let (first_end, first_id) = self.type_term_tree(depth, tree.as_deref_mut());
        if !self
            .tokens
            .peek()
            .is_some_and(|token| token.kind == TokenKind::Or)
        {
            return (start..first_end, first_id);
        }
        let mut end = first_end;
        let mut local_terms = Vec::new();
        if let Some(id) = first_id {
            local_terms.push(id);
        }
        while self
            .tokens
            .peek()
            .is_some_and(|token| token.kind == TokenKind::Or)
        {
            self.tokens.next();
            let (term_end, term_id) = self.type_term_tree(depth, tree.as_deref_mut());
            end = term_end;
            if let (Some(t), Some(id)) = (tree.as_deref_mut(), term_id) {
                if t.is_overflowed() || local_terms.len() >= MAX_TYPE_NODES {
                    t.mark_overflow();
                } else {
                    local_terms.push(id);
                }
            }
        }
        let id = tree.map(|t| {
            let terms = t.push_type_list(local_terms);
            t.push_node(TypeTreeKind::Union { terms }, start..end)
        });
        (start..end, id)
    }

    fn type_term_tree(
        &mut self,
        depth: usize,
        mut tree: Option<&mut TypeTreeBuilder>,
    ) -> (usize, Option<TypeNodeId>) {
        let tilde_start = if self
            .tokens
            .peek()
            .is_some_and(|token| token.kind == TokenKind::Tilde)
        {
            Some(self.tokens.next().expect("peeked tilde").span.start)
        } else {
            None
        };
        let (span, inner_id) = self.type_with_inference(depth + 1, true, tree.as_deref_mut());
        let end = span.end;
        let id = match (tilde_start, tree, inner_id) {
            (Some(tilde), Some(t), Some(inner)) => {
                Some(t.push_node(TypeTreeKind::Approx(inner), tilde..end))
            }
            (_, _, id) => id,
        };
        (end, id)
    }

    fn interface_member_tree(
        &mut self,
        depth: usize,
        container: usize,
        mut tree: Option<&mut TypeTreeBuilder>,
        local_members: &mut Vec<InterfaceMemberNode>,
    ) {
        if self
            .tokens
            .peek()
            .is_some_and(|token| token.kind == TokenKind::Ident)
            && self
                .tokens
                .clone()
                .nth(1)
                .is_some_and(|token| token.kind == TokenKind::LParen)
        {
            let name = self.tokens.next().expect("peeked method name").span;
            let start = self
                .tokens
                .peek()
                .expect("peeked method parentheses")
                .span
                .start;
            let (end, sig_id) = self.function_type_signature_tree(depth + 1, tree.as_deref_mut());
            self.interface_members.push(InterfaceMember {
                name: Some(name.clone()),
                annotation: start..end,
                container,
            });
            if let (Some(t), Some(sig)) = (tree, sig_id) {
                if t.is_overflowed() || local_members.len() >= MAX_TYPE_NODES {
                    t.mark_overflow();
                } else {
                    local_members.push(InterfaceMemberNode::Method {
                        name: TypeTreeBuilder::span32(name),
                        sig,
                    });
                }
            }
        } else {
            let (annotation, ty_id) = self.type_elem_tree(depth, tree.as_deref_mut());
            self.interface_members.push(InterfaceMember {
                name: None,
                annotation,
                container,
            });
            if let (Some(t), Some(ty)) = (tree, ty_id) {
                if t.is_overflowed() || local_members.len() >= MAX_TYPE_NODES {
                    t.mark_overflow();
                } else {
                    local_members.push(InterfaceMemberNode::Embedded(ty));
                }
            }
        }
    }
}
