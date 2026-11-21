//! Cross-file relations prove canonical declarations in the shared type arena.
use super::Semantics;
use crate::analysis::engine::obj::{DeclId, MethodOrigin, NamedObject, ObjectRef};
use std::path::Path;

/// High-level facade for cross-file interface implementation and embedding relations.
pub(crate) struct TypeRelationQuery<'r, 'a> {
    sema: &'r mut Semantics<'a>,
}

impl<'r, 'a> TypeRelationQuery<'r, 'a> {
    pub(super) fn new(sema: &'r mut Semantics<'a>) -> Self {
        Self { sema }
    }

    pub(crate) fn direct_embedding_declarations(
        &mut self,
        declaration: DeclId<'_>,
    ) -> Option<Vec<DeclId<'a>>> {
        self.sema.direct_embedding_declarations(declaration)
    }

    pub(crate) fn declaration_form(
        &mut self,
        declaration: DeclId<'_>,
    ) -> Option<crate::analysis::engine::types::TypeForm> {
        self.sema.declaration_form(declaration)
    }

    pub(crate) fn implementation_members_matching(
        &mut self,
        declaration: DeclId<'_>,
        required: Option<&[&str]>,
    ) -> Option<(bool, Vec<NamedObject<'a>>)> {
        self.sema
            .implementation_members_matching(declaration, required)
    }

    pub(crate) fn implementation_method_object(
        &mut self,
        declaration: DeclId<'_>,
        origin: &Path,
        name: &str,
    ) -> Option<ObjectRef<'a>> {
        self.sema
            .implementation_method_object(declaration, origin, name)
    }

    pub(crate) fn implementation_signatures_match(
        &mut self,
        left: DeclId<'_>,
        left_member: NamedObject<'_>,
        right: DeclId<'_>,
        right_member: NamedObject<'_>,
    ) -> Option<bool> {
        self.sema
            .implementation_signatures_match(left, left_member, right, right_member)
    }

    pub(crate) fn implementation_matches(
        &mut self,
        concrete: DeclId<'_>,
        interface: DeclId<'_>,
    ) -> Option<bool> {
        self.sema.implementation_matches(concrete, interface)
    }

    pub(crate) fn implements_universe_error(&mut self, declaration: DeclId<'_>) -> Option<bool> {
        self.sema.implements_universe_error(declaration)
    }
}

impl<'a> Semantics<'a> {
    pub(crate) fn direct_embedding_declarations(
        &mut self,
        declaration: DeclId<'_>,
    ) -> Option<Vec<DeclId<'a>>> {
        let declaration = self.canonical_decl(declaration)?;
        self.arena.with_projection(true, |arena| {
            arena.direct_embedding_declarations(declaration)
        })
    }

    pub(crate) fn declaration_form(
        &mut self,
        declaration: DeclId<'_>,
    ) -> Option<crate::analysis::engine::types::TypeForm> {
        let declaration = self.canonical_decl(declaration)?;
        self.arena.with_projection(true, |arena| {
            let ty = arena.declaration_type(declaration)?;
            arena.presentation_type(ty).map(|(form, _, _)| form)
        })
    }

    /// Requested method objects are selected by the native traversal. Indexed names
    /// attach borrowed display labels only; missing metadata makes the set
    /// unavailable rather than returning a partial set of requirements.
    pub(crate) fn implementation_members_matching(
        &mut self,
        declaration: DeclId<'_>,
        required: Option<&[&str]>,
    ) -> Option<(bool, Vec<NamedObject<'a>>)> {
        let declaration = self.canonical_decl(declaration)?;
        self.prepare_for_new_region();
        let (abstract_, members) = self
            .arena
            .implementation_member_set(declaration, required)?;
        let members = members
            .into_iter()
            .map(|(name, object)| {
                let name = match object {
                    ObjectRef::Method(MethodOrigin::Concrete(decl)) => decl.symbol.name.as_str(),
                    ObjectRef::Method(MethodOrigin::Interface(member))
                        if member.builtin && name == "Error" =>
                    {
                        "Error"
                    }
                    ObjectRef::Method(MethodOrigin::Interface(member)) => {
                        member.describe_name(self.workspace, &name)?.name
                    }
                    _ => return None,
                };
                Some(NamedObject { name, object })
            })
            .collect::<Option<Vec<_>>>()?;
        Some((abstract_, members))
    }

    pub(crate) fn implementation_method_object(
        &mut self,
        declaration: DeclId<'_>,
        origin: &Path,
        name: &str,
    ) -> Option<ObjectRef<'a>> {
        let declaration = self.canonical_decl(declaration)?;
        self.prepare_for_new_region();
        self.arena
            .implementation_method_object(declaration, origin, name)
    }

    /// Match one required method in the gopls implementation-search relation.
    /// Each call has independent existential formals; it is not assignment.
    pub(crate) fn implementation_signatures_match(
        &mut self,
        left: DeclId<'_>,
        left_member: NamedObject<'_>,
        right: DeclId<'_>,
        right_member: NamedObject<'_>,
    ) -> Option<bool> {
        let left = self.canonical_decl(left)?;
        let right = self.canonical_decl(right)?;
        self.prepare_for_new_region();
        self.arena
            .implementation_signatures_match(left, left_member, right, right_member)
    }

    pub(crate) fn implementation_matches(
        &mut self,
        concrete: DeclId<'_>,
        interface: DeclId<'_>,
    ) -> Option<bool> {
        let concrete = self.canonical_decl(concrete)?;
        let interface = self.canonical_decl(interface)?;
        self.prepare_for_new_region();
        self.arena.implementation_matches(concrete, interface)
    }

    pub(crate) fn implements_universe_error(&mut self, declaration: DeclId<'_>) -> Option<bool> {
        let declaration = self.canonical_decl(declaration)?;
        self.prepare_for_new_region();
        self.arena.implements_universe_error(declaration)
    }
}
