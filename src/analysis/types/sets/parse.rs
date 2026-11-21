//! Parse constraints on top of the unified `interface`/`union` representation.
use super::{MAX_TERMS, Term, TypeSet};
use crate::analysis::engine::types::constraints::{self, Constraint};
use crate::analysis::engine::types::declaration_body;
use crate::analysis::engine::types::{MAX_BYTES, MAX_DEPTH, MAX_NODES, Ty, TypeArena, TypeId};
use crate::syntax::{Lexer, TokenKind};
use std::path::Path;

impl TypeArena<'_> {
    pub(in crate::analysis::engine::types) fn constraint_set_checked(
        &mut self,
        origin: &Path,
        text: &str,
        environment: &[(&str, TypeId)],
        depth: usize,
    ) -> crate::analysis::engine::types::CheckOutcome<TypeSet> {
        use crate::analysis::engine::types::{CheckOutcome, UnknownReason};
        if self.step().is_none() || depth >= MAX_DEPTH || text.len() > MAX_BYTES {
            return CheckOutcome::Unknown(UnknownReason::BudgetExceeded);
        }
        let tokens: Vec<_> = Lexer::new(text).collect();
        let (Some(first), Some(last)) = (
            tokens.first(),
            tokens.iter().rfind(|t| t.kind != TokenKind::Semicolon),
        ) else {
            return CheckOutcome::Unknown(UnknownReason::SyntaxError);
        };
        let Some(text) = text.get(first.span.start..last.span.end) else {
            return CheckOutcome::Unknown(UnknownReason::SyntaxError);
        };
        // Union separators inside signatures and composite types belong to the
        // nested grammar, not to this constraint's term list.
        let mut nesting = 0usize;
        let mut start = 0;
        let mut parts = Vec::new();
        for token in Lexer::new(text) {
            match token.kind {
                TokenKind::LParen | TokenKind::LBrack | TokenKind::LBrace => nesting += 1,
                TokenKind::RParen | TokenKind::RBrack | TokenKind::RBrace => {
                    let Some(next) = nesting.checked_sub(1) else {
                        return CheckOutcome::Unknown(UnknownReason::SyntaxError);
                    };
                    nesting = next;
                }
                TokenKind::Or if nesting == 0 => {
                    let Some(part) = text.get(start..token.span.start) else {
                        return CheckOutcome::Unknown(UnknownReason::SyntaxError);
                    };
                    parts.push(part);
                    start = token.span.end;
                }
                _ => {}
            }
        }
        if nesting != 0 {
            return CheckOutcome::Unknown(UnknownReason::SyntaxError);
        }
        if !parts.is_empty() {
            let Some(tail) = text.get(start..) else {
                return CheckOutcome::Unknown(UnknownReason::SyntaxError);
            };
            parts.push(tail);
            if parts.len() > MAX_TERMS {
                return CheckOutcome::Unknown(UnknownReason::BudgetExceeded);
            }
            let mut result = TypeSet::all();
            result.terms = Some(Vec::new());
            let mut direct = Vec::new();
            for part in parts {
                let set = match self.constraint_set_checked(origin, part, environment, depth + 1) {
                    CheckOutcome::Ok(set) => set,
                    other => return other,
                };
                if set.comparable || set.methods.is_some() {
                    return CheckOutcome::Invalid(());
                }
                if !set.interface {
                    let Some(part_terms) = set.terms.as_ref() else {
                        return CheckOutcome::Unknown(UnknownReason::Unsupported);
                    };
                    for &term in part_terms {
                        for &previous in &direct {
                            match self.term_intersection(term, previous) {
                                Some(Some(_)) => return CheckOutcome::Invalid(()),
                                Some(None) => {}
                                None => {
                                    return CheckOutcome::Unknown(UnknownReason::BudgetExceeded);
                                }
                            }
                        }
                        direct.push(term);
                    }
                }
                result.terms = match (result.terms, set.terms) {
                    (Some(mut left), Some(right)) => {
                        left.extend(right);
                        match self.normalize_terms(left) {
                            Some(normalized) => Some(normalized),
                            None => return CheckOutcome::Unknown(UnknownReason::BudgetExceeded),
                        }
                    }
                    _ => None,
                };
                result.checks.extend(set.checks);
                if result.checks.len() > MAX_NODES {
                    return CheckOutcome::Unknown(UnknownReason::BudgetExceeded);
                }
            }
            return CheckOutcome::Ok(result);
        }
        if first.kind == TokenKind::Interface {
            let iface =
                match self.read_interface_at_checked(origin, text, None, environment, depth + 1) {
                    CheckOutcome::Ok(id) => id,
                    other => {
                        return match other {
                            CheckOutcome::Ok(_) => unreachable!(),
                            CheckOutcome::Invalid(()) => CheckOutcome::Invalid(()),
                            CheckOutcome::Unknown(reason) => CheckOutcome::Unknown(reason),
                        };
                    }
                };
            return self.interface_type_set_checked(iface, depth + 1);
        }
        if first.kind == TokenKind::LParen && last.kind == TokenKind::RParen {
            return self.constraint_set_checked(
                origin,
                &text[1..text.len() - 1],
                environment,
                depth + 1,
            );
        }
        if matches!(text, "any" | "comparable")
            && self.universe_constraint(origin, text, environment) == Some(true)
        {
            let mut set = TypeSet::all();
            set.comparable = text == "comparable";
            return CheckOutcome::Ok(set);
        }
        let approximate = first.kind == TokenKind::Tilde;
        let term = if approximate {
            let Some(rest) = text.get(1..) else {
                return CheckOutcome::Unknown(UnknownReason::SyntaxError);
            };
            rest.trim()
        } else {
            text
        };
        if environment.iter().any(|(name, _)| *name == term) {
            return CheckOutcome::Invalid(());
        }
        let ty = match self.parse_in_checked(origin, term, None, environment) {
            CheckOutcome::Ok(ty) => ty,
            CheckOutcome::Invalid(()) => return CheckOutcome::Invalid(()),
            CheckOutcome::Unknown(reason) => return CheckOutcome::Unknown(reason),
        };
        let Some(is_iface) = self.interface_constructor(ty) else {
            return CheckOutcome::Unknown(UnknownReason::MissingMetadata);
        };
        if is_iface {
            if approximate {
                return CheckOutcome::Invalid(());
            }
            if self.named_info(ty).is_some() {
                return match self.named_set(ty, depth + 1) {
                    Some(set) => CheckOutcome::Ok(set),
                    None => CheckOutcome::Unknown(UnknownReason::MissingMetadata),
                };
            }
            let Some(root) = self.underlying(ty) else {
                return CheckOutcome::Unknown(UnknownReason::MissingMetadata);
            };
            return self.interface_type_set_checked(root, depth + 1);
        }
        let Some(ty) = self.unalias(ty) else {
            return CheckOutcome::Unknown(UnknownReason::MissingMetadata);
        };
        if matches!(self.node(ty), Some(Ty::TypeParam(_)))
            || (approximate && matches!(self.node(ty), Some(Ty::Named { .. })))
        {
            return CheckOutcome::Invalid(());
        }
        if self.underlying(ty).is_none() {
            return CheckOutcome::Unknown(UnknownReason::MissingMetadata);
        }
        if self.alloc_tier == crate::analysis::engine::types::ty::AllocTier::Local {
            self.local.constraint_terms += 1;
            if self.local.constraint_terms > self.local.pool.node_limit {
                return CheckOutcome::Unknown(UnknownReason::BudgetExceeded);
            }
        }
        CheckOutcome::Ok(TypeSet {
            terms: Some(vec![Term { ty, approximate }]),
            interface: false,
            ..TypeSet::all()
        })
    }

    pub(super) fn named_set(&mut self, input: TypeId, depth: usize) -> Option<TypeSet> {
        if depth >= MAX_DEPTH {
            return None;
        }
        let (declaration, arguments, _) = self.named_info(input)?;
        let (file, symbol) = (declaration.file, declaration.symbol);
        let arguments = self.arguments_slice(input, arguments).to_vec();
        let source = self
            .workspace
            .source_slice(file, symbol.end..symbol.declaration()?.end)?;
        let (header, signature) = crate::analysis::engine::types::named::parse_type_parameters_of(
            &source,
            symbol.generic,
        )?;
        if !signature.diagnostics.is_empty() {
            return None;
        }
        let names = constraints::parameter_names(header, &signature)?;
        if names.len() != arguments.len() {
            return None;
        }
        let mut param_ids = Vec::with_capacity(names.len());
        let mut environment = Vec::with_capacity(names.len());
        for (index, name) in names.into_iter().enumerate() {
            let param = self.with_local_alloc(|store| store.push_type_param(index))?;
            param_ids.push(param);
            environment.push((name, param));
        }
        let constraints = self.constraints(file, header, &signature, &environment)?;
        let mut set = self
            .constraint_set_checked(file, declaration_body(&source)?, &environment, depth + 1)
            .ok()?;
        for (constraint, (_, parameter)) in constraints.into_iter().zip(&environment) {
            if !matches!(constraint, Constraint::Any) {
                set.checks.push((constraint, *parameter));
            }
        }
        if set.checks.len() > MAX_NODES {
            return None;
        }
        let bindings: Vec<_> = arguments.into_iter().map(Some).collect();
        set = self.substitute_set(&set, &param_ids, &bindings)?;
        set.interface = true;
        Some(set)
    }
}
