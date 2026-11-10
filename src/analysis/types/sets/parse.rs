//! Parse constraints independently of ordinary interface value types.
use super::{MAX_TERMS, Term, TypeSet};
use crate::analysis::types::constraints::{self, Constraint};
use crate::analysis::types::declaration_body;
use crate::analysis::types::interface::Method;
use crate::analysis::types::{MAX_BYTES, MAX_DEPTH, MAX_NODES, Ty, TypeArena, TypeId};
use crate::syntax::{Lexer, TokenKind};
use std::collections::HashSet;
use std::path::Path;

impl TypeArena<'_> {
    pub(in crate::analysis::types) fn constraint_set(
        &mut self,
        origin: &Path,
        text: &str,
        environment: &[(&str, TypeId)],
        depth: usize,
    ) -> Option<TypeSet> {
        self.step()?;
        if depth >= MAX_DEPTH || text.len() > MAX_BYTES {
            return None;
        }
        let tokens: Vec<_> = Lexer::new(text).collect();
        let first = tokens.first()?;
        let last = tokens.iter().rfind(|t| t.kind != TokenKind::Semicolon)?;
        let text = text.get(first.span.start..last.span.end)?;
        // Union separators inside signatures and composite types belong to the
        // nested grammar, not to this constraint's term list.
        let mut nesting = 0usize;
        let mut start = 0;
        let mut parts = Vec::new();
        for token in Lexer::new(text) {
            match token.kind {
                TokenKind::LParen | TokenKind::LBrack | TokenKind::LBrace => nesting += 1,
                TokenKind::RParen | TokenKind::RBrack | TokenKind::RBrace => {
                    nesting = nesting.checked_sub(1)?;
                }
                TokenKind::Or if nesting == 0 => {
                    parts.push(text.get(start..token.span.start)?);
                    start = token.span.end;
                }
                _ => {}
            }
        }
        if nesting != 0 {
            return None;
        }
        if !parts.is_empty() {
            parts.push(text.get(start..)?);
            if parts.len() > MAX_TERMS {
                return None;
            }
            let mut result = TypeSet::all();
            result.terms = Some(Vec::new());
            let mut direct = Vec::new();
            for part in parts {
                let set = self.constraint_set(origin, part, environment, depth + 1)?;
                if set.comparable || set.methods.is_some() {
                    return None;
                }
                if !set.interface {
                    for &term in set.terms.as_ref()? {
                        for &previous in &direct {
                            if self.term_intersection(term, previous)?.is_some() {
                                return None;
                            }
                        }
                        direct.push(term);
                    }
                }
                result.terms = match (result.terms, set.terms) {
                    (Some(mut left), Some(right)) => {
                        left.extend(right);
                        Some(self.normalize_terms(left)?)
                    }
                    _ => None,
                };
                result.checks.extend(set.checks);
                if result.checks.len() > MAX_NODES {
                    return None;
                }
            }
            return Some(result);
        }
        if first.kind == TokenKind::Interface {
            return self.interface_set(origin, text, environment, depth + 1);
        }
        if first.kind == TokenKind::LParen && last.kind == TokenKind::RParen {
            return self.constraint_set(origin, &text[1..text.len() - 1], environment, depth + 1);
        }
        if matches!(text, "any" | "comparable")
            && self.universe_constraint(origin, text, environment)?
        {
            let mut set = TypeSet::all();
            set.comparable = text == "comparable";
            return Some(set);
        }
        let approximate = first.kind == TokenKind::Tilde;
        let term = if approximate {
            text.get(1..)?.trim()
        } else {
            text
        };
        if environment.iter().any(|(name, _)| *name == term) {
            return None;
        }
        let ty = self.parse_in(origin, term, environment)?;
        if self.interface_constructor(ty)? {
            if approximate {
                return None;
            }
            if matches!(self.nodes[usize::from(ty.0)], Ty::Named { .. }) {
                return self.named_set(ty, depth + 1);
            }
            let mut set = TypeSet::all();
            let root = self.underlying(ty)?;
            let Ty::Interface { ref methods, .. } = self.nodes[usize::from(root.0)] else {
                return None;
            };
            if !methods.is_empty() {
                set.methods = Some(root);
            }
            return Some(set);
        }
        let ty = self.unalias(ty)?;
        if matches!(self.nodes[usize::from(ty.0)], Ty::Parameter(_))
            || approximate && matches!(self.nodes[usize::from(ty.0)], Ty::Named { .. })
        {
            return None;
        }
        self.underlying(ty)?;
        self.constraint_terms += 1;
        if self.constraint_terms > MAX_NODES {
            return None;
        }
        Some(TypeSet {
            terms: Some(vec![Term { ty, approximate }]),
            interface: false,
            ..TypeSet::all()
        })
    }

    fn interface_set(
        &mut self,
        origin: &Path,
        source: &str,
        environment: &[(&str, TypeId)],
        depth: usize,
    ) -> Option<TypeSet> {
        let origin = self.workspace.files.get_key_value(origin)?.0.as_path();
        let parsed = crate::syntax::parse_type(source);
        if !parsed.diagnostics.is_empty() {
            return None;
        }
        let container = Lexer::new(source).nth(1)?.span.start;
        let mut set = TypeSet::all();
        let mut methods = Vec::new();
        let mut explicit = HashSet::new();
        for member in parsed
            .interface_members
            .iter()
            .filter(|m| m.container == container)
        {
            if let Some(name) = &member.name {
                let name = source.get(name.clone())?;
                if name == "_" || !explicit.insert(name) {
                    return None;
                }
                let header = format!("func{}", source.get(member.annotation.clone())?);
                let signature = crate::syntax::parse_literal_signature(&header);
                let signature =
                    self.signature(origin, &header, &signature, environment, depth + 1)?;
                methods.push(Method {
                    name: name.to_owned(),
                    origin,
                    signature,
                });
                if methods.len() > MAX_NODES {
                    return None;
                }
            } else {
                let embedded = self.constraint_set(
                    origin,
                    source.get(member.annotation.clone())?,
                    environment,
                    depth + 1,
                )?;
                set = self.intersect_sets(set, embedded)?;
            }
        }
        if !methods.is_empty() {
            let mut required = TypeSet::all();
            required.methods = Some(self.store_interface(origin, methods)?);
            set = self.intersect_sets(set, required)?;
        }
        set.interface = true;
        Some(set)
    }

    fn named_set(&mut self, input: TypeId, depth: usize) -> Option<TypeSet> {
        if depth >= MAX_DEPTH {
            return None;
        }
        let Ty::Named {
            declaration,
            arguments,
        } = self.nodes[usize::from(input.0)].clone()
        else {
            return None;
        };
        let (file, symbol) = (declaration.file, declaration.symbol);
        let arguments = self.arguments[arguments].to_vec();
        let source = self
            .workspace
            .source_slice(file, symbol.end..symbol.declaration()?.end)?;
        let header = match crate::syntax::type_parameter_end(&source) {
            Some(end) => format!("func _{}()", source.get(..end)?),
            None if !symbol.generic => "func _()".to_owned(),
            None => return None,
        };
        let signature = crate::syntax::parse_signature(&header);
        if !signature.diagnostics.is_empty() {
            return None;
        }
        let names = constraints::parameter_names(&header, &signature)?;
        if names.len() != arguments.len() {
            return None;
        }
        let mut environment = Vec::new();
        for (index, name) in names.into_iter().enumerate() {
            environment.push((name, self.push(Ty::Parameter(index))?));
        }
        let constraints = self.constraints(file, &header, &signature, &environment)?;
        let mut set =
            self.constraint_set(file, declaration_body(&source)?, &environment, depth + 1)?;
        for (constraint, (_, parameter)) in constraints.into_iter().zip(&environment) {
            if !matches!(constraint, Constraint::Any) {
                set.checks.push((constraint, *parameter));
            }
        }
        if set.checks.len() > MAX_NODES {
            return None;
        }
        let bindings: Vec<_> = arguments.into_iter().map(Some).collect();
        set = self.substitute_set(&set, &bindings)?;
        set.interface = true;
        Some(set)
    }
}
