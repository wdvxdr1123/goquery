//! Identity resolution shared by references, fallback navigation and highlighting.
use crate::analysis::obj::{MemberDeclId, MethodOrigin, ObjectRef, SymbolOrigin};
use crate::analysis::resolve::NameProbe;
use crate::analysis::scope::occurrences::{Name, Occurrences, Role};
use crate::analysis::sema::{LiteralKey, Semantics};
use crate::index::IndexedSymbol as Symbol;
use crate::workspace::Workspace;
use std::ops::Range;
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub(crate) enum Identity<'a> {
    Local(Range<usize>),
    Global(&'a PathBuf, &'a Symbol),
    Member(MemberDeclId<'a>),
}
/// Per-file projection policy. Lexical candidates never prove an identity.
pub(crate) struct IdentityResolver<'a> {
    workspace: &'a Workspace,
    file: &'a Path,
    name: Option<&'a str>,
    package_global_only: bool,
    members_enabled: bool,
}

impl<'a> IdentityResolver<'a> {
    pub(crate) fn new(workspace: &'a Workspace, file: &'a Path) -> Self {
        Self {
            workspace,
            file,
            name: None,
            package_global_only: false,
            members_enabled: true,
        }
    }

    pub(crate) fn for_name(workspace: &'a Workspace, file: &'a Path, index: &'a NameProbe) -> Self {
        Self {
            name: Some(index.name()),
            ..Self::new(workspace, file)
        }
    }

    pub(crate) fn for_reference(
        workspace: &'a Workspace,
        file: &'a Path,
        index: &'a NameProbe,
        target: &Symbol,
    ) -> Self {
        Self {
            package_global_only: target.receiver.is_none(),
            members_enabled: false,
            ..Self::for_name(workspace, file, index)
        }
    }
}

impl<'a> IdentityResolver<'a> {
    pub(crate) fn resolve<'sema: 'a>(
        &mut self,
        sema: &mut Semantics<'sema>,
        analysis: &Occurrences,
        source: &str,
        name: &Name,
    ) -> Option<Identity<'a>> {
        self.resolve_uncached(sema, analysis, source, name)
    }

    fn sema_resolve<'sema: 'a>(
        &mut self,
        sema: &mut Semantics<'sema>,
        source: &str,
        name: &Name,
    ) -> Option<Option<Identity<'a>>> {
        let span_u32 = [name.span.start as u32, name.span.end as u32];
        let file = self.file;
        let obj: ObjectRef<'a> = sema.resolve_offset(file, source, span_u32)?;
        if self.package_global_only && matches!(obj, ObjectRef::Field(_) | ObjectRef::Method(_)) {
            return Some(None);
        }
        let decl_identity = |decl: crate::analysis::obj::DeclId<'a>| {
            self.workspace
                .files
                .get_key_value(decl.file)
                .map(|(path, _)| Identity::Global(path, decl.symbol))
        };
        match obj {
            ObjectRef::Const(SymbolOrigin::Package(decl))
            | ObjectRef::TypeName {
                origin: SymbolOrigin::Package(decl),
                ..
            }
            | ObjectRef::Var {
                origin: SymbolOrigin::Package(decl),
                ..
            }
            | ObjectRef::Func(decl)
            | ObjectRef::Method(MethodOrigin::Concrete(decl)) => Some(decl_identity(decl)),
            ObjectRef::Const(SymbolOrigin::Local { span, .. })
            | ObjectRef::TypeName {
                origin: SymbolOrigin::Local { span, .. },
                ..
            }
            | ObjectRef::Var {
                origin: SymbolOrigin::Local { span, .. },
                ..
            }
            | ObjectRef::TypeParam { span, .. } => {
                Some(Some(Identity::Local(span[0] as usize..span[1] as usize)))
            }
            ObjectRef::Field(member) | ObjectRef::Method(MethodOrigin::Interface(member)) => {
                if !self.members_enabled {
                    Some(None)
                } else {
                    Some(member.indexed(self.workspace).map(Identity::Member))
                }
            }
            ObjectRef::PkgName { .. }
            | ObjectRef::BuiltinFunction(_)
            | ObjectRef::BuiltinType(_)
            | ObjectRef::BuiltinConst(_)
            | ObjectRef::Nil => Some(None),
            ObjectRef::Label { span, .. } => {
                Some(Some(Identity::Local(span[0] as usize..span[1] as usize)))
            }
        }
    }

    fn resolve_uncached<'sema: 'a>(
        &mut self,
        sema: &mut Semantics<'sema>,
        analysis: &Occurrences,
        source: &str,
        name: &Name,
    ) -> Option<Identity<'a>> {
        let spelling = source.get(name.span.clone())?;
        if spelling == "_" || self.name.is_some_and(|expected| expected != spelling) {
            return None;
        }
        if let Some(resolved) = self.sema_resolve(sema, source, name) {
            return resolved;
        }
        if sema.implicit_declaration_at(
            self.file,
            source,
            [name.span.start as u32, name.span.end as u32],
        ) {
            return Some(Identity::Local(name.span.clone()));
        }
        // Elided composite keys may require a container projection. It uses
        // the same native arena, not a lexical type/name reconstruction.
        let literal = match &name.role {
            Role::LiteralKey(ty) => {
                sema.literal_key_at(self.file, source, ty.clone(), &[], spelling)
            }
            Role::LiteralElementKey(id) => {
                let (span, projections) = analysis.literal_element_projection(*id)?;
                sema.literal_key_at(self.file, source, span, &projections, spelling)
            }
            _ => return None,
        };
        match literal {
            LiteralKey::Field(member) if self.members_enabled && !self.package_global_only => {
                member.indexed(self.workspace).map(Identity::Member)
            }
            LiteralKey::Field(_) | LiteralKey::Expression | LiteralKey::Unknown => None,
        }
    }
}
