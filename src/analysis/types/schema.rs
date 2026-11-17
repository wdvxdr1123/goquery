//! Potential method-signature identity for implementation search.
//!
//! This is gopls's per-method search relation: independent type parameters may
//! bind to types, and declaration constraints do not restrict the search. It is
//! distinct from assignability and call inference. All nodes belong to the
//! request arena and disappear at its speculative watermark.
use super::relation::TypePairs;
use super::{BasicKind, MAX_DEPTH, MAX_NODES, Tags, Ty, TypeArena, TypeId};
use crate::analysis::obj::{DeclId, NamedObject};
use std::collections::HashMap;

impl<'a> TypeArena<'a> {
    /// Final proof includes native requirements omitted by lightweight stubs.
    pub(crate) fn implementation_matches(
        &mut self,
        concrete: DeclId<'a>,
        interface: DeclId<'a>,
    ) -> Option<bool> {
        self.with_projection(true, |arena| {
            let concrete = arena.implementation_receiver(concrete)?;
            let interface = arena.implementation_receiver(interface)?;
            let interface = arena.underlying(interface)?;
            let Ty::Interface { methods, .. } = arena.nodes.get(usize::from(interface.0))?.clone()
            else {
                return Some(false);
            };
            if methods.is_empty() {
                return Some(false);
            }
            let methods = arena.methods[methods].to_vec();
            for required in methods {
                let Some(actual) =
                    arena.implementation_method(concrete, required.origin, &required.name)?
                else {
                    return Some(false);
                };
                arena.validate_type(actual)?;
                arena.validate_type(required.signature)?;
                // gopls starts a fresh unifier for each required method.
                if !arena.schema_match(actual, required.signature)? {
                    return Some(false);
                }
            }
            Some(true)
        })
    }

    pub(crate) fn implementation_signatures_match(
        &mut self,
        left: DeclId<'a>,
        left_member: NamedObject<'_>,
        right: DeclId<'a>,
        right_member: NamedObject<'_>,
    ) -> Option<bool> {
        let (left_origin, right_origin) = (left_member.file()?, right_member.file()?);
        if left_member.name != right_member.name
            || !crate::syntax::is_exported_name(left_member.name)
                && !self.same_field_package(left_origin, right_origin)?
        {
            return Some(false);
        }
        self.with_projection(true, |arena| {
            let left = arena.implementation_receiver(left)?;
            let right = arena.implementation_receiver(right)?;
            let Some(left) = arena.implementation_method(left, left_origin, left_member.name)?
            else {
                return Some(false);
            };
            let Some(right) =
                arena.implementation_method(right, right_origin, right_member.name)?
            else {
                return Some(false);
            };
            arena.validate_type(left)?;
            arena.validate_type(right)?;
            arena.schema_match(left, right)
        })
    }

    pub(crate) fn implements_universe_error(&mut self, declaration: DeclId<'a>) -> Option<bool> {
        self.with_projection(true, |arena| {
            let recv = arena.implementation_receiver(declaration)?;
            let Some(signature) = arena.implementation_method(recv, declaration.file, "Error")?
            else {
                return Some(false);
            };
            let string = arena.basic(BasicKind::String)?;
            let expected = arena.store_signature(Vec::new(), vec![string], false)?;
            arena.validate_type(signature)?;
            arena.schema_match(signature, expected)
        })
    }

    /// Go's implementation search uses T for interfaces and *T for concrete
    /// types, while an alias that already denotes a pointer keeps that pointer.
    pub(super) fn implementation_receiver(&mut self, declaration: DeclId<'a>) -> Option<TypeId> {
        if !declaration.symbol.is_type() {
            return None;
        }
        let mut arguments = Vec::new();
        if declaration.symbol.generic {
            let source = self.type_parameter_source(declaration)?;
            let (header, signature) = super::named::parse_type_parameters_of(&source, true)?;
            if !signature.diagnostics.is_empty() {
                return None;
            }
            let names = super::constraints::parameter_names(header, &signature)?;
            if names.is_empty() || names.len() > 32 {
                return None;
            }
            for index in 0..names.len() {
                // Schema variables need no display name or constraint table.
                // Their TypeIds are fresh even when two owners share an ordinal.
                arguments.push(self.push(Ty::Parameter(index))?);
            }
        }
        let root = self.push_named(declaration, &arguments, declaration.symbol.alias)?;
        let root = self.unalias(root)?;
        if self.interface_constructor(root)?
            || matches!(self.nodes.get(usize::from(root.0))?, Ty::Pointer(_))
        {
            Some(root)
        } else {
            self.pointer(root)
        }
    }

    fn schema_resolve(
        &mut self,
        mut input: TypeId,
        bindings: &HashMap<TypeId, TypeId>,
    ) -> Option<TypeId> {
        loop {
            self.step()?;
            input = self.unalias(input)?;
            if !matches!(self.nodes.get(usize::from(input.0))?, Ty::Parameter(_)) {
                return Some(input);
            }
            match bindings.get(&input) {
                Some(bound) => input = *bound,
                None => return Some(input),
            }
        }
    }

    fn schema_occurs(
        &mut self,
        parameter: TypeId,
        input: TypeId,
        bindings: &HashMap<TypeId, TypeId>,
    ) -> Option<bool> {
        let mut pending = vec![(input, 0usize)];
        let mut seen = vec![false; self.nodes.len()];
        while let Some((input, depth)) = pending.pop() {
            self.step()?;
            if depth >= MAX_DEPTH {
                return None;
            }
            let input = self.schema_resolve(input, bindings)?;
            if input == parameter {
                return Some(true);
            }
            if std::mem::replace(seen.get_mut(usize::from(input.0))?, true) {
                continue;
            }
            let TypePairs::Children(children) = self.identity_pairs(input, input, Tags::Include)?
            else {
                return None;
            };
            if pending.len() + children.len() > MAX_NODES {
                return None;
            }
            pending.extend(children.into_iter().map(|(child, _)| (child, depth + 1)));
        }
        Some(false)
    }

    fn schema_match(&mut self, left: TypeId, right: TypeId) -> Option<bool> {
        let mut bindings = HashMap::new();
        let mut pending = vec![(left, right, 0usize)];
        while let Some((left, right, depth)) = pending.pop() {
            self.step()?;
            if depth >= MAX_DEPTH {
                return None;
            }
            let left = self.schema_resolve(left, &bindings)?;
            let right = self.schema_resolve(right, &bindings)?;
            if left == right {
                continue;
            }
            let parameter = match (
                self.nodes.get(usize::from(left.0))?,
                self.nodes.get(usize::from(right.0))?,
            ) {
                (Ty::Parameter(_), _) => Some((left, right)),
                (_, Ty::Parameter(_)) => Some((right, left)),
                _ => None,
            };
            if let Some((parameter, value)) = parameter {
                if self.schema_occurs(parameter, value, &bindings)? {
                    return Some(false);
                }
                if bindings.len() >= 64 {
                    return None;
                }
                bindings.insert(parameter, value);
                continue;
            }
            let TypePairs::Children(children) = self.identity_pairs(left, right, Tags::Include)?
            else {
                return Some(false);
            };
            if pending.len() + children.len() > MAX_NODES {
                return None;
            }
            pending.extend(
                children
                    .into_iter()
                    .map(|(left, right)| (left, right, depth + 1)),
            );
        }
        Some(true)
    }
}
