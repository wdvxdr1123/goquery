//! Identity resolution shared by references, fallback navigation and highlighting.
use crate::analysis::obj::{
    MemberDeclaration, MethodOrigin, ObjectRef, ReceiverMember, SymbolOrigin,
};
use crate::analysis::resolve::NameIndex;
use crate::analysis::scope::{self as locals, Locals};
use crate::analysis::sema::Semantics;
use crate::analysis::source::{Name, Role, SourceAnalysis};
use crate::analysis::types::TypeForm;
use crate::index::IndexedSymbol as Symbol;
use crate::syntax::{MemberKind, TokenKind};
use crate::text::SourceMap;
use crate::workspace::Workspace;
use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub(crate) enum Identity<'a> {
    Local(Range<usize>),
    Global(&'a PathBuf, &'a Symbol),
    Member(MemberDeclaration<'a>),
}
type Symbols<'a> = HashMap<&'a str, (&'a PathBuf, &'a Symbol)>;

/// Bounded by the packages and receiver types actually used by one file query.
pub(crate) struct IdentityResolver<'a> {
    workspace: &'a Workspace,
    index: Option<&'a NameIndex<'a>>,
    package_global_only: bool,
    members_enabled: bool,
    file: &'a Path,
    package: HashMap<&'a str, Option<(&'a PathBuf, &'a Symbol)>>,
    imports: HashMap<String, Symbols<'a>>,
    members: HashMap<String, Vec<ReceiverMember<'a>>>,
    sema: Option<Semantics<'a>>,
}

impl<'a> IdentityResolver<'a> {
    pub(crate) fn new(workspace: &'a Workspace, file: &'a Path) -> Self {
        let mut package = HashMap::new();
        for (path, symbol) in crate::analysis::resolve::file_symbol_iter(workspace, file) {
            package
                .entry(symbol.name.as_str())
                .and_modify(|entry| *entry = None)
                .or_insert(Some((path, symbol)));
        }
        Self {
            workspace,
            index: None,
            package_global_only: false,
            members_enabled: true,
            file,
            package,
            imports: HashMap::new(),
            members: HashMap::new(),
            sema: None,
        }
    }

    pub(crate) fn for_name(
        workspace: &'a Workspace,
        file: &'a Path,
        index: &'a NameIndex<'a>,
    ) -> Self {
        Self {
            workspace,
            file,
            index: Some(index),
            package_global_only: false,
            members_enabled: true,
            package: HashMap::new(),
            imports: HashMap::new(),
            members: HashMap::new(),
            sema: None,
        }
    }

    pub(crate) fn for_reference(
        workspace: &'a Workspace,
        file: &'a Path,
        index: &'a NameIndex<'a>,
        target: &Symbol,
    ) -> Self {
        let mut resolver = Self::for_name(workspace, file, index);
        resolver.package_global_only = target.receiver.is_none();
        resolver.members_enabled = false;
        resolver
    }

    fn unqualified(&self, name: &str) -> Option<(&'a PathBuf, &'a Symbol)> {
        if let Some(index) = self.index {
            debug_assert_eq!(index.name(), name);
            index.unqualified(self.file)
        } else {
            self.package.get(name).copied().flatten()
        }
    }

    fn imported(&mut self, alias: &str, name: &str) -> Option<(&'a PathBuf, &'a Symbol)> {
        if let Some(index) = self.index {
            debug_assert_eq!(index.name(), name);
            return index.imported(self.file, alias);
        }
        let symbols = self.imports.entry(alias.to_owned()).or_insert_with(|| {
            crate::analysis::resolve::imported_symbol_iter(self.workspace, self.file, alias)
                .map(|(path, symbol)| (symbol.name.as_str(), (path, symbol)))
                .collect()
        });
        symbols.get(name).copied()
    }
}

impl<'a> IdentityResolver<'a> {
    pub(crate) fn resolve(
        &mut self,
        analysis: &SourceAnalysis,
        source: &str,
        name: &Name,
    ) -> Option<Identity<'a>> {
        self.resolve_uncached(analysis, source, name)
    }

    fn sema_resolve(&mut self, source: &str, name: &Name) -> Option<Option<Identity<'a>>> {
        let span_u32 = [name.span.start as u32, name.span.end as u32];
        let workspace = self.workspace;
        let file = self.file;
        let obj = {
            let sema = self.sema.get_or_insert_with(|| Semantics::new(workspace));
            let (region, _) = sema.region_and_arena_at(file, source, span_u32[0], None)?;
            region
                .def_by_span(span_u32)
                .or_else(|| region.use_by_span(span_u32))
                .or_else(|| region.selection_by_span(span_u32).map(|sel| sel.obj))?
        };
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
                    Some(
                        member
                            .to_member_declaration(self.workspace)
                            .map(Identity::Member),
                    )
                }
            }
            ObjectRef::PkgName { .. }
            | ObjectRef::BuiltinFunction(_)
            | ObjectRef::BuiltinType(_)
            | ObjectRef::BuiltinConst(_)
            | ObjectRef::Nil
            | ObjectRef::Label { .. } => Some(None),
        }
    }

    fn resolve_uncached(
        &mut self,
        analysis: &SourceAnalysis,
        source: &str,
        name: &Name,
    ) -> Option<Identity<'a>> {
        if let Some((path, stub)) = self.workspace.files.get_key_value(self.file)
            && let Some(symbol) = stub
                .symbols
                .iter()
                .find(|symbol| symbol.start == name.span.start && symbol.end == name.span.end)
        {
            return Some(Identity::Global(path, symbol));
        }
        let spelling = &source[name.span.clone()];
        if spelling == "_" {
            return None;
        }
        match &name.role {
            Role::ExpressionMember(_) => {
                if self.package_global_only {
                    return None;
                }
                self.sema_resolve(source, name).flatten()
            }
            Role::Parameter | Role::Result => Some(Identity::Local(name.span.clone())),
            Role::FieldDeclaration(owner) => self
                .field_declaration(*owner, spelling)
                .map(Identity::Member),
            Role::LiteralKey(_) | Role::LiteralElementKey(_) => {
                if let Some(resolved) = self.sema_resolve(source, name) {
                    return resolved;
                }
                let literal = match &name.role {
                    Role::LiteralKey(ty) => {
                        self.literal_key(source, &analysis.locals, ty, spelling)
                    }
                    Role::LiteralElementKey(id) => analysis
                        .literal_element_type(self.workspace, self.file, source, *id)
                        .map_or(LiteralKey::Unknown, |(ty, offset)| {
                            self.literal_key_type(source, &analysis.locals, &ty, offset, spelling)
                        }),
                    _ => unreachable!("literal key role"),
                };
                match literal {
                    LiteralKey::Field(member) => Some(Identity::Member(member)),
                    LiteralKey::Expression => {
                        if let Some(binding) = analysis.locals.definition_at(&name.span, spelling) {
                            Some(Identity::Local(binding.selection.clone()))
                        } else {
                            self.unqualified(spelling)
                                .map(|(path, symbol)| Identity::Global(path, symbol))
                        }
                    }
                    LiteralKey::Unknown => None,
                }
            }
            Role::Namespace
            | Role::Field
            | Role::Ignored
            | Role::BlankDeclaration { .. }
            | Role::Member(None) => None,
            Role::Type(Some(qualifier))
            | Role::EmbeddedType(Some(qualifier))
            | Role::Member(Some(qualifier)) => {
                let text = &source[qualifier.clone()];
                if analysis.locals.resolve_at(qualifier.start, text).is_some() {
                    if matches!(name.role, Role::Type(_) | Role::EmbeddedType(_))
                        || self.package_global_only
                    {
                        return None;
                    }
                    self.sema_resolve(source, name).flatten()
                } else if let Some((path, symbol)) = self.imported(text, spelling) {
                    Some(Identity::Global(path, symbol))
                } else if matches!(name.role, Role::Member(_))
                    && !self.package_global_only
                    && crate::analysis::resolve::unqualified_symbol(self.workspace, self.file, text)
                        .is_some_and(|(_, symbol)| symbol.is_value())
                {
                    self.sema_resolve(source, name).flatten()
                } else {
                    None
                }
            }
            Role::Name | Role::Type(None) | Role::EmbeddedType(None) => {
                if let Some(binding) = analysis.locals.definition_at(&name.span, spelling) {
                    return Some(Identity::Local(binding.selection.clone()));
                }
                self.unqualified(spelling)
                    .map(|(p, s)| Identity::Global(p, s))
            }
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum LiteralKey<'a> {
    Field(MemberDeclaration<'a>),
    Expression,
    Unknown,
}

impl<'a> IdentityResolver<'a> {
    pub(super) fn member(&mut self, receiver: &str, name: &str) -> Option<Identity<'a>> {
        let members = self.members.entry(receiver.to_owned()).or_insert_with(|| {
            if self.index.is_some() {
                crate::analysis::types::lookup::receiver_member(
                    self.workspace,
                    self.file,
                    receiver,
                    name,
                )
                .into_iter()
                .collect()
            } else {
                crate::analysis::types::lookup::receiver_members(
                    self.workspace,
                    self.file,
                    receiver,
                )
            }
        });
        members.iter().find_map(|member| match member {
            ReceiverMember::Method(path, symbol) if symbol.name == name => {
                Some(Identity::Global(path, symbol))
            }
            ReceiverMember::Declaration(member) if member.name == name => {
                Some(Identity::Member(*member))
            }
            _ => None,
        })
    }

    pub(super) fn field_declaration(
        &self,
        start: usize,
        name: &str,
    ) -> Option<MemberDeclaration<'a>> {
        if !self.members_enabled || self.package_global_only {
            return None;
        }
        let (file, stub) = self.workspace.files.get_key_value(self.file)?;
        let owner = stub
            .symbols
            .iter()
            .find(|symbol| symbol.start == start && symbol.is_type())?;
        let info = owner.type_info.as_ref()?;
        let mut names = info.members.iter().filter(|member| *member == name);
        let name = names.next()?;
        if names.next().is_some() {
            return None;
        }
        Some(MemberDeclaration {
            file,
            owner,
            name,
            kind: if info.is_interface() {
                MemberKind::Method
            } else {
                MemberKind::Field
            },
            embedded: false,
        })
    }

    pub(super) fn literal_key(
        &mut self,
        source: &str,
        locals: &Locals,
        ty: &Range<usize>,
        name: &str,
    ) -> LiteralKey<'a> {
        self.literal_key_type(source, locals, &source[ty.clone()], ty.start, name)
    }

    pub(super) fn literal_key_type(
        &mut self,
        source: &str,
        locals: &Locals,
        ty: &str,
        offset: usize,
        name: &str,
    ) -> LiteralKey<'a> {
        let Some(ty_name) = locals::named_type(ty) else {
            return if crate::analysis::types::type_form_in_scope(
                self.workspace,
                self.file,
                ty,
                locals,
                offset,
            )
            .is_some_and(|form| matches!(form, TypeForm::Map | TypeForm::Array | TypeForm::Slice))
            {
                LiteralKey::Expression
            } else {
                LiteralKey::Unknown
            };
        };
        if let Some(binding) = locals.resolve_at(
            offset,
            locals::named_root(&ty_name)
                .split('.')
                .next()
                .expect("named type"),
        ) {
            return if binding.object_kind.is_type()
                && binding
                    .annotation()
                    .and_then(|span| source.get(span.clone()))
                    .is_some_and(map_or_array)
            {
                LiteralKey::Expression
            } else {
                LiteralKey::Unknown
            };
        }
        let Some((file, owner)) =
            crate::analysis::types::lookup::composite_target(self.workspace, self.file, &ty_name)
        else {
            return LiteralKey::Unknown;
        };
        if owner.type_info.is_some() {
            if self.package_global_only || !self.members_enabled {
                return LiteralKey::Unknown;
            }
            return match self.member(&ty_name, name) {
                Some(Identity::Member(member))
                    if member.file == file
                        && std::ptr::eq(member.owner, owner)
                        && member.kind == MemberKind::Field =>
                {
                    LiteralKey::Field(member)
                }
                _ => LiteralKey::Unknown,
            };
        }
        let Some(source) = self.workspace.source(file) else {
            return LiteralKey::Unknown;
        };
        let map = SourceMap::new(&source);
        let proven = map
            .offset(owner.range.end)
            .and_then(|end| source.get(owner.end..end))
            .is_some_and(map_or_array);
        if proven {
            LiteralKey::Expression
        } else {
            LiteralKey::Unknown
        }
    }
}

fn map_or_array(source: &str) -> bool {
    crate::syntax::Lexer::new(source)
        .next()
        .is_some_and(|token| matches!(token.kind, TokenKind::Map | TokenKind::LBrack))
}
