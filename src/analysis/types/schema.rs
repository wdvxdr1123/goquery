//! Potential method-signature identity for implementation search.
//!
//! This is `gopls`'s per-method search relation: independent type parameters may
//! bind to types, and declaration constraints do not restrict the search. It is
//! distinct from assignability and call inference. All nodes belong to the
//! request arena and disappear at its speculative watermark.
use super::unify::Unifier;
use super::{BasicKind, Ty, TypeArena, TypeId};
use crate::analysis::engine::obj::{DeclId, NamedObject};

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
            let Some(methods) = arena
                .interface_data(interface)
                .map(|data| data.methods.clone())
            else {
                return Some(false);
            };
            if methods.is_empty() {
                return Some(false);
            }
            let methods = arena.methods_slice(interface, methods).to_vec();
            for required in methods {
                let Some(actual) =
                    arena.implementation_method(concrete, required.origin, &required.name)?
                else {
                    return Some(false);
                };
                arena.validate_type(actual)?;
                arena.validate_type(required.signature)?;
                // gopls starts a fresh unifier for each required method.
                if !Unifier::match_schema(arena, actual, required.signature)? {
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
            Unifier::match_schema(arena, left, right)
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
            Unifier::match_schema(arena, signature, expected)
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
            let origin_id =
                self.get_or_insert_package_origin(declaration, declaration.symbol.alias)?;
            let count = self.ensure_origin_tparams(origin_id, false)?.len();
            if count == 0 || count > self.active_node_limit() {
                return None;
            }
            for index in 0..count {
                // Schema variables need no display name or constraint table.
                // Their TypeIds are fresh even when two owners share an ordinal.
                arguments.push(self.push_type_param(index)?);
            }
        }
        let root = self.push_named(declaration, &arguments, declaration.symbol.alias)?;
        let root = self.unalias(root)?;
        if self.interface_constructor(root)? || matches!(self.node(root)?, Ty::Pointer(_)) {
            Some(root)
        } else {
            self.pointer(root)
        }
    }
}
