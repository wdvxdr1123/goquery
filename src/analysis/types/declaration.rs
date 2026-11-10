//! gopls-style type-name projection, retaining aliases and signature result order.
use super::{MAX_BYTES, MAX_DEPTH, MAX_NODES, Ty, TypeArena, TypeId};
use crate::index::IndexedSymbol as Symbol;
use std::ops::Range;
use std::path::Path;

pub(crate) enum TypeDeclaration<'a> {
    Named(&'a Path, &'a Symbol),
    Builtin(&'static str),
    Local(&'a Path, Range<usize>),
}
pub(crate) struct LocalTypeName<'s> {
    pub(crate) name: &'s str,
    pub(crate) selection: Range<usize>,
    pub(crate) formal: bool,
}

/// One written local `type` declaration, borrowed for the duration of the query.
/// Only the four facts go/types needs for a local defined type or alias are kept.
pub(crate) struct LocalTypeDefinition<'s> {
    pub(crate) name: &'s str,
    pub(crate) selection: Range<usize>,
    pub(crate) annotation: &'s str,
    pub(crate) alias: bool,
}

#[derive(Clone, Copy)]
pub(super) enum LocalTypeState {
    Pending,
    Ready(TypeId),
    Failed,
}

/// Owned query-local fact: a bare local type declaration has no body tree. The
/// RHS is parsed once, in the same declaration environment as its references.
pub(super) struct LocalTypeFact<'a> {
    pub(super) name: Box<str>,
    pub(super) origin: &'a Path,
    pub(super) selection: Range<usize>,
    pub(super) annotation: Box<str>,
    pub(super) node: TypeId,
    pub(super) alias: bool,
    pub(super) state: LocalTypeState,
}

// Local defined types/aliases use the documented 64-entry, 16 KiB budget.
const MAX_LOCAL_TYPES: usize = 64;

impl<'a> TypeArena<'a> {
    /// Register query-local local-type facts and parse one written annotation in
    /// that environment. Identity is `(file, selection)`; the spelling is display only.
    pub(crate) fn declared_scope_type(
        &mut self,
        origin: &Path,
        text: &str,
        definitions: &[LocalTypeDefinition<'_>],
    ) -> Option<TypeId> {
        if text.len() > MAX_BYTES || definitions.len() > MAX_LOCAL_TYPES {
            return None;
        }
        let origin = self.workspace.files.get_key_value(origin)?.0.as_path();
        let mut environment: Vec<(&str, TypeId)> = Vec::new();
        for definition in definitions {
            if definition.name.is_empty()
                || definition.annotation.is_empty()
                || environment.iter().any(|(name, _)| *name == definition.name)
            {
                return None;
            }
            environment.push((definition.name, self.local_type_node(origin, definition)?));
        }
        for definition in definitions {
            let index = self.local_type_index(origin, &definition.selection)?;
            if !matches!(self.local_types[index].state, LocalTypeState::Pending) {
                continue;
            }
            let parsed = self.parse_in(origin, definition.annotation, &environment);
            self.local_types[index].state = match parsed {
                Some(ty) => LocalTypeState::Ready(ty),
                None => LocalTypeState::Failed,
            };
        }
        self.parse_in(origin, text, &environment)
    }

    fn local_type_node(
        &mut self,
        origin: &'a Path,
        definition: &LocalTypeDefinition<'_>,
    ) -> Option<TypeId> {
        if let Some(fact) = self
            .local_types
            .iter()
            .find(|fact| fact.origin == origin && fact.selection == definition.selection)
        {
            // One declaration has exactly one written definition. A conflict means
            // the query mixed incompatible scopes; stay unknown rather than pick one.
            return (fact.name.as_ref() == definition.name
                && fact.alias == definition.alias
                && fact.annotation.as_ref() == definition.annotation)
                .then_some(fact.node);
        }
        if self.local_types.len() >= MAX_LOCAL_TYPES {
            return None;
        }
        let bytes = self
            .local_type_bytes
            .checked_add(definition.name.len())?
            .checked_add(definition.annotation.len())?;
        if bytes > MAX_BYTES {
            return None;
        }
        let node = self.push(Ty::LocalDeclaration {
            origin,
            selection: definition.selection.clone(),
            formal: false,
        })?;
        self.local_types.push(LocalTypeFact {
            name: definition.name.into(),
            origin,
            selection: definition.selection.clone(),
            annotation: definition.annotation.into(),
            node,
            alias: definition.alias,
            state: LocalTypeState::Pending,
        });
        self.local_type_bytes = bytes;
        Some(node)
    }

    pub(super) fn local_type_index(
        &self,
        origin: &Path,
        selection: &Range<usize>,
    ) -> Option<usize> {
        self.local_types
            .iter()
            .position(|fact| fact.origin == origin && fact.selection == *selection)
    }

    /// Display name of a registered local declaration; the spelling never proves
    /// identity. Unregistered opaque declarations fall back to a bounded read.
    pub(super) fn local_type_name(
        &self,
        origin: &Path,
        selection: &Range<usize>,
    ) -> Option<String> {
        if let Some(index) = self.local_type_index(origin, selection) {
            return Some(self.local_types[index].name.to_string());
        }
        self.workspace.source_slice(origin, selection.clone())
    }

    /// Expand the outer local declaration chain with an active set; the depth and
    /// node budgets bound every step. Package types are returned for the caller's
    /// existing lazy expansion. Cycles, aliases without facts and generic local
    /// type usage stay unknown.
    pub(super) fn local_declaration_underlying(&mut self, input: TypeId) -> Option<TypeId> {
        let mut active: Vec<(&Path, Range<usize>)> = Vec::new();
        let mut current = input;
        for _ in 0..MAX_DEPTH {
            let (origin, selection, formal) = match self.nodes.get(usize::from(current.0))? {
                Ty::LocalDeclaration {
                    origin,
                    selection,
                    formal,
                } => (*origin, selection.clone(), *formal),
                _ => return Some(current),
            };
            if formal {
                return None;
            }
            if active
                .iter()
                .any(|(file, span)| *file == origin && *span == selection)
            {
                return None;
            }
            active.push((origin, selection.clone()));
            let index = self.local_type_index(origin, &selection)?;
            current = match self.local_types[index].state {
                LocalTypeState::Ready(ty) => ty,
                LocalTypeState::Pending | LocalTypeState::Failed => return None,
            };
        }
        None
    }

    /// One alias step: `Some(Some(next))` for a registered local alias,
    /// `Some(None)` for a non-local or defined type, `None` when unknown.
    pub(super) fn local_alias(&self, input: TypeId) -> Option<Option<TypeId>> {
        let Ty::LocalDeclaration {
            origin,
            selection,
            formal,
        } = self.nodes.get(usize::from(input.0))?
        else {
            return Some(None);
        };
        if *formal {
            return None;
        }
        let index = self.local_type_index(origin, selection)?;
        if !self.local_types[index].alias {
            return Some(None);
        }
        match self.local_types[index].state {
            LocalTypeState::Ready(ty) => Some(Some(ty)),
            LocalTypeState::Pending | LocalTypeState::Failed => None,
        }
    }

    /// Whether a compound type contains a query-local declaration, so callers can
    /// keep such spellings display-only instead of reparsing them as identities.
    pub(crate) fn has_local_declaration(&mut self, input: TypeId) -> Option<bool> {
        let mut pending = vec![input];
        let mut visited = Vec::new();
        while let Some(ty) = pending.pop() {
            if visited.contains(&ty) {
                continue;
            }
            if visited.len() >= MAX_NODES {
                return None;
            }
            visited.push(ty);
            match self.nodes.get(usize::from(ty.0))? {
                Ty::LocalDeclaration { .. } => return Some(true),
                Ty::Named { arguments, .. } => {
                    pending.extend_from_slice(&self.arguments[arguments.clone()])
                }
                Ty::Struct { fields, .. } => {
                    let fields = self.fields[fields.clone()].to_vec();
                    pending.extend(fields.into_iter().map(|field| field.ty));
                }
                Ty::Interface { methods, .. } => {
                    let methods = self.methods[methods.clone()].to_vec();
                    pending.extend(methods.into_iter().map(|method| method.signature));
                }
                Ty::Signature {
                    parameters,
                    results,
                    ..
                } => {
                    pending.extend_from_slice(&self.arguments[parameters.clone()]);
                    pending.extend_from_slice(&self.arguments[results.clone()]);
                }
                Ty::Pointer(element)
                | Ty::Slice(element)
                | Ty::Error(element)
                | Ty::Array { element, .. }
                | Ty::Chan { element, .. } => pending.push(*element),
                Ty::Map { key, value } => pending.extend([*key, *value]),
                Ty::Basic(_) | Ty::Parameter(_) => {}
            }
        }
        Some(false)
    }

    /// Test-only structural identity for a local defined type; aliases are already
    /// transparent. Declarations keep their file/offset identity, never a spelling.
    #[cfg(test)]
    pub(super) fn local_declaration_identity(
        &mut self,
        input: TypeId,
    ) -> Option<serde_json::Value> {
        let (origin, selection, formal) = match self.nodes.get(usize::from(input.0))? {
            Ty::LocalDeclaration {
                origin,
                selection,
                formal,
            } => (*origin, selection.clone(), *formal),
            _ => return None,
        };
        if formal {
            return None;
        }
        self.local_type_index(origin, &selection)?;
        Some(serde_json::json!(["local", origin, selection.start, []]))
    }

    /// Query-local lexical declarations enter only the projection graph. Their
    /// bodies/constraints remain unknown; no source/name strings are retained.
    pub(crate) fn declaration_type(
        &mut self,
        origin: &Path,
        text: &str,
        names: &[LocalTypeName<'_>],
    ) -> Option<TypeId> {
        if names.len() > 64 {
            return None;
        }
        let origin = self.workspace.files.get_key_value(origin)?.0.as_path();
        let mut environment = Vec::new();
        for name in names {
            if environment.iter().any(|(key, _)| *key == name.name) {
                return None;
            }
            let ty = self.push(Ty::LocalDeclaration {
                origin,
                selection: name.selection.clone(),
                formal: name.formal,
            })?;
            environment.push((name.name, ty));
        }
        self.parse_in(origin, text, &environment)
    }

    /// Projection-only schema: opaque formals preserve declaration references.
    /// It deliberately proves no constraints and must not check value calls.
    pub(crate) fn declaration_signature(
        &mut self,
        origin: &Path,
        header: &str,
        parsed: &crate::syntax::Signature,
    ) -> Option<TypeId> {
        let mut names = super::constraints::parameter_names(header, parsed)?
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if let Some(receiver) = parsed.receiver.first() {
            for span in crate::analysis::scope::receiver_parameters(header, &receiver.annotation) {
                let name = header.get(span)?.to_owned();
                if !names.contains(&name) {
                    names.push(name);
                }
            }
        }
        if names.len() > 32 {
            return None;
        }
        let mut environment = Vec::new();
        for (index, name) in names.iter().enumerate() {
            environment.push((name.as_str(), self.push(Ty::Parameter(index))?));
        }
        if header.len() > super::MAX_BYTES || !parsed.diagnostics.is_empty() {
            return None;
        }
        let mut results = Vec::new();
        for result in &parsed.results {
            results.push(self.parse_in(
                origin,
                header.get(result.annotation.clone())?,
                &environment,
            )?);
        }
        self.store_signature(Vec::new(), results, false)
    }

    /// Named/alias origins stop traversal; unnamed containers unwrap elements,
    /// signatures visit results only, and maps/anonymous aggregates have no name.
    /// Repeated results are retained, matching the reference server's ordering.
    pub(crate) fn type_declarations(&mut self, input: TypeId) -> Option<Vec<TypeDeclaration<'a>>> {
        let mut result = Vec::new();
        let mut pending = vec![(input, 0)];
        while let Some((input, depth)) = pending.pop() {
            self.step()?;
            if depth >= MAX_DEPTH || result.len() >= MAX_NODES {
                return None;
            }
            match self.nodes.get(usize::from(input.0))? {
                Ty::Named { declaration, .. } => {
                    result.push(TypeDeclaration::Named(declaration.file, declaration.symbol))
                }
                Ty::Basic(kind) if !kind.untyped() => {
                    result.push(TypeDeclaration::Builtin(kind.name()))
                }
                Ty::LocalDeclaration {
                    origin,
                    selection,
                    formal: false,
                } => result.push(TypeDeclaration::Local(origin, selection.clone())),
                Ty::LocalDeclaration { formal: true, .. } => {}
                Ty::Error(_) => result.push(TypeDeclaration::Builtin("error")),
                Ty::Interface {
                    builtin_any: true, ..
                } => result.push(TypeDeclaration::Builtin("any")),
                Ty::Pointer(element)
                | Ty::Slice(element)
                | Ty::Array { element, .. }
                | Ty::Chan { element, .. } => pending.push((*element, depth + 1)),
                Ty::Signature { results, .. } => pending.extend(
                    self.arguments[results.clone()]
                        .iter()
                        .rev()
                        .map(|ty| (*ty, depth + 1)),
                ),
                Ty::Basic(_)
                | Ty::Parameter(_)
                | Ty::Interface { .. }
                | Ty::Struct { .. }
                | Ty::Map { .. } => {}
            }
            if pending.len() > MAX_NODES {
                return None;
            }
        }
        Some(result)
    }
}
