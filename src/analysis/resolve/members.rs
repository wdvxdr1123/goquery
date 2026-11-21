//! Source-backed member descriptions, separate from canonical object identity.
use super::exported;
use crate::analysis::engine::budget::MAX_BYTES;
use crate::analysis::engine::obj::{DeclId, MemberDeclId, MethodOrigin, NamedObject, ObjectRef};
use crate::index::IndexedSymbol;
use crate::syntax::{AggregateMember, MemberKind, TokenKind};
use crate::text::SourceMap;
use crate::workspace::Workspace;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tower_lsp::lsp_types::Range;

impl<'a> MemberDeclId<'a> {
    pub(crate) fn describe(
        self,
        workspace: &'a crate::workspace::Workspace,
    ) -> Option<MemberDescription<'a>> {
        let [start, end] = self.selection;
        if start >= end || end - start > 1024 {
            return None;
        }
        let raw_name = workspace.source_slice(self.file, start as usize..end as usize)?;
        self.describe_name(workspace, &raw_name)
    }

    /// A native selection already supplies the exact lookup name; reading its
    /// source again is unnecessary for indexed display metadata.
    pub(crate) fn describe_name(
        self,
        workspace: &'a crate::workspace::Workspace,
        target: &str,
    ) -> Option<MemberDescription<'a>> {
        let [start, end] = self.selection;
        let (start, end) = (start as usize, end as usize);
        if start >= end || end.saturating_sub(start) > 1024 {
            return None;
        }
        let (file, stub) = workspace.files.get_key_value(self.file)?;
        let owner = self
            .owner
            .filter(|sym| {
                stub.symbols.iter().any(|s| std::ptr::eq(s, *sym))
                    && sym
                        .declaration()
                        .is_some_and(|decl| decl.start <= start && end <= decl.end)
            })
            .or_else(|| {
                stub.symbols.iter().find(|sym| {
                    sym.is_type()
                        && sym
                            .declaration()
                            .is_some_and(|decl| decl.start <= start && end <= decl.end)
                })
            })?;
        let info = owner.type_info.as_ref()?;
        let name: &'a str = if self.embedded {
            info.embedded.iter().find_map(|entry| {
                let head = entry.split('[').next().unwrap_or(entry.as_str());
                let bare = head.trim_start_matches('*').rsplit('.').next()?;
                (bare == target).then_some(bare)
            })?
        } else {
            info.members
                .iter()
                .find(|m| m.as_str() == target)
                .map(String::as_str)
                .or_else(|| {
                    info.search_members
                        .iter()
                        .find(|m| m.name == target)
                        .map(|m| m.name.as_str())
                })?
        };
        let kind = if info.is_interface() {
            MemberKind::Method
        } else {
            MemberKind::Field
        };
        Some(MemberDescription {
            file,
            owner,
            name,
            kind,
            embedded: self.embedded,
        })
    }
}

pub(crate) struct MemberIds<'a> {
    workspace: &'a Workspace,
    headers: HashMap<DeclId<'a>, Vec<(&'a str, MemberDeclId<'a>)>>,
    bytes: usize,
    identities: usize,
}

impl<'a> MemberIds<'a> {
    pub(crate) fn new(workspace: &'a Workspace) -> Self {
        Self {
            workspace,
            headers: HashMap::new(),
            bytes: 0,
            identities: 0,
        }
    }

    pub(crate) fn get(&mut self, member: MemberDescription<'a>) -> Option<MemberDeclId<'a>> {
        let owner = DeclId {
            file: member.file,
            symbol: member.owner,
        };
        if !self.headers.contains_key(&owner) {
            let info = member.owner.type_info.as_ref()?;
            let count = info.members.len().checked_add(info.embedded.len())?;
            if self.headers.len() >= 128 || self.identities.checked_add(count)? > 1024 {
                return None;
            }
            let span = member.owner.end..member.owner.declaration()?.end;
            if span.len() > crate::analysis::engine::budget::MAX_BYTES
                || self.bytes.saturating_add(span.len()) > 64 * 1024
            {
                return None;
            }
            self.bytes += span.len();
            let source = self.workspace.source_slice(member.file, span)?;
            let entries = crate::syntax::aggregate_members(&source);
            let mut members = Vec::new();
            for name in &info.members {
                let view = MemberDescription {
                    name,
                    embedded: false,
                    ..member
                };
                if let Some(identity) = view.identity_from_entries(&source, &entries) {
                    members.push((name.as_str(), identity));
                }
            }
            if !info.is_interface() {
                for embedded in &info.embedded {
                    let name = embedded
                        .split('[')
                        .next()?
                        .trim_start_matches('*')
                        .rsplit('.')
                        .next()?;
                    let view = MemberDescription {
                        name,
                        embedded: true,
                        ..member
                    };
                    if let Some(identity) = view.identity_from_entries(&source, &entries) {
                        members.push((name, identity));
                    }
                }
            }
            self.identities += members.len();
            self.headers.insert(owner, members);
        }
        self.headers
            .get(&owner)?
            .iter()
            .find(|(name, id)| *name == member.name && id.embedded == member.embedded)
            .map(|(_, identity)| *identity)
    }
}

/// Private method/field names have package identity, even when embedded.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct MemberId<'a> {
    pub(crate) name: &'a str,
    package: Option<(&'a Path, &'a str)>,
}

pub(crate) fn member_id<'a>(
    workspace: &'a Workspace,
    file: &'a Path,
    name: &'a str,
) -> MemberId<'a> {
    MemberId {
        name,
        package: (!exported(name)).then(|| {
            (
                file.parent().expect("source directory"),
                workspace.files[file].package.as_str(),
            )
        }),
    }
}

/// A request-local source/display projection. Equality, caches and reference
/// matching use MemberDeclId/ObjectRef rather than this description.
#[derive(Clone, Copy)]
pub(crate) struct MemberDescription<'a> {
    pub file: &'a PathBuf,
    pub owner: &'a IndexedSymbol,
    pub name: &'a str,
    pub kind: MemberKind,
    pub embedded: bool,
}

impl<'a> NamedObject<'a> {
    pub(crate) fn member(
        self,
        workspace: &'a crate::workspace::Workspace,
    ) -> Option<MemberDescription<'a>> {
        let (identity, kind) = match self.object {
            ObjectRef::Field(identity) => (identity, MemberKind::Field),
            ObjectRef::Method(MethodOrigin::Interface(identity)) => (identity, MemberKind::Method),
            _ => return None,
        };
        let (file, _) = workspace.files.get_key_value(identity.file)?;
        Some(MemberDescription {
            file,
            owner: identity.owner?,
            name: self.name,
            kind,
            embedded: identity.embedded,
        })
    }
}

pub(crate) struct MemberData {
    pub selection: Range,
    pub annotation: Option<String>,
}

impl<'a> MemberDescription<'a> {
    pub(crate) fn identity_in_source(self, source: &str) -> Option<MemberDeclId<'a>> {
        let declaration = source.get(self.owner.end..self.owner.declaration()?.end)?;
        if declaration.len() > MAX_BYTES {
            return None;
        }
        let entries = crate::syntax::aggregate_members(declaration);
        self.identity_from_entries(declaration, &entries)
    }

    pub(crate) fn identity_from_entries(
        self,
        declaration: &str,
        entries: &[AggregateMember],
    ) -> Option<MemberDeclId<'a>> {
        let entry = self.entry(declaration, entries)?;
        let selection = self.selection(declaration, entry)?;
        Some(MemberDeclId {
            file: self.file,
            owner: Some(self.owner),
            selection: [
                u32::try_from(self.owner.end + selection.start).ok()?,
                u32::try_from(self.owner.end + selection.end).ok()?,
            ],
            embedded: self.embedded,
            builtin: false,
        })
    }
    pub(crate) fn selection(
        &self,
        declaration: &str,
        entry: &AggregateMember,
    ) -> Option<std::ops::Range<usize>> {
        if !self.embedded {
            return Some(entry.selection.clone());
        }
        let annotation = declaration.get(entry.annotation.clone())?;
        let token = crate::syntax::Lexer::new(annotation)
            .take_while(|token| token.kind != TokenKind::LBrack)
            .filter(|token| token.kind == TokenKind::Ident)
            .last()?;
        Some(entry.annotation.start + token.span.start..entry.annotation.start + token.span.end)
    }

    pub(crate) fn entry<'entry>(
        &self,
        declaration: &str,
        entries: &'entry [AggregateMember],
    ) -> Option<&'entry AggregateMember> {
        let root = crate::syntax::aggregate_offset(declaration)?;
        entries.iter().find(|entry| {
            entry.container == root
                && if self.embedded {
                    entry.selection.start == entry.annotation.start
                        && entry
                            .symbol
                            .name
                            .trim_start_matches('*')
                            .split('[')
                            .next()
                            .unwrap_or("")
                            .rsplit('.')
                            .next()
                            == Some(self.name)
                } else {
                    entry.symbol.name == self.name
                }
        })
    }

    pub(crate) fn data(self, workspace: &crate::workspace::Workspace) -> Option<MemberData> {
        let source = workspace.source(self.file)?;
        let map = SourceMap::new(&source);
        self.data_in_source(&source, &map)
    }

    pub(crate) fn data_in_source(self, source: &str, map: &SourceMap<'_>) -> Option<MemberData> {
        let base = self.owner.end;
        let declaration = source.get(base..map.offset(self.owner.range.end)?)?;
        let entries = crate::syntax::aggregate_members(declaration);
        let entry = self.entry(declaration, &entries)?;
        let selection = self.selection(declaration, entry)?;
        Some(MemberData {
            selection: map.range(base + selection.start, base + selection.end),
            annotation: (entry.annotation.len() <= MAX_BYTES)
                .then(|| declaration.get(entry.annotation.clone()).map(str::to_owned))
                .flatten(),
        })
    }
}
