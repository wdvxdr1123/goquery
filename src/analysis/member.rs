//! Query-local member locations and annotations, keyed by the owning declaration.
use crate::index::IndexedSymbol as Symbol;
use crate::syntax::{AggregateMember, MemberKind, TokenKind};
use crate::text::SourceMap;
use crate::workspace::Workspace;
use std::path::PathBuf;
use tower_lsp::lsp_types::Range;

#[derive(Clone, Copy)]
pub(crate) struct MemberDeclaration<'a> {
    pub file: &'a PathBuf,
    pub owner: &'a Symbol,
    pub name: &'a str,
    pub kind: MemberKind,
    pub embedded: bool,
}

pub(crate) struct MemberData {
    pub selection: Range,
    pub annotation: Option<String>,
}

impl MemberDeclaration<'_> {
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

    pub(crate) fn same(&self, other: &Self) -> bool {
        self.file == other.file && self.owner.start == other.owner.start && self.name == other.name
    }

    pub(crate) fn entry<'a>(
        &self,
        declaration: &str,
        entries: &'a [AggregateMember],
    ) -> Option<&'a AggregateMember> {
        let root = crate::syntax::aggregate_offset(declaration)?;
        entries.iter().find(|entry| {
            entry.container == root
                && if self.embedded {
                    entry.selection.start == entry.annotation.start
                        && crate::analysis::scope::named_root(
                            entry.symbol.name.trim_start_matches('*'),
                        )
                        .rsplit('.')
                        .next()
                            == Some(self.name)
                } else {
                    entry.symbol.name == self.name
                }
        })
    }
}

impl Workspace {
    pub(crate) fn member_data(&self, member: MemberDeclaration<'_>) -> Option<MemberData> {
        let source = self.source(member.file)?;
        let map = SourceMap::new(&source);
        Self::member_data_in_source(member, &source, &map)
    }

    pub(crate) fn member_data_in_source(
        member: MemberDeclaration<'_>,
        source: &str,
        map: &SourceMap<'_>,
    ) -> Option<MemberData> {
        let base = member.owner.end;
        let declaration = source.get(base..map.offset(member.owner.range.end)?)?;
        let entries = crate::syntax::aggregate_members(declaration);
        let entry = member.entry(declaration, &entries)?;
        let selection = member.selection(declaration, entry)?;
        Some(MemberData {
            selection: map.range(base + selection.start, base + selection.end),
            annotation: (entry.annotation.len() <= 16 * 1024)
                .then(|| declaration.get(entry.annotation.clone()).map(str::to_owned))
                .flatten(),
        })
    }
}
