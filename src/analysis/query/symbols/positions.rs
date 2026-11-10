//! Resolve member positions only after bounded ranking, one source at a time.
use super::{Candidate, Context, SearchKind, SearchSymbol, SourceLocation, SourceRange};
use crate::text::SourceMap;

impl<'a, View> Context<'a, View> {
    pub(super) fn symbol_positions(
        &self,
        best: Vec<Candidate<'a>>,
    ) -> Option<Vec<SearchSymbol<'a>>> {
        let mut ranked: Vec<_> = best.into_iter().enumerate().collect();
        ranked
            .sort_by(|(_, a), (_, b)| a.file.cmp(b.file).then(a.symbol.start.cmp(&b.symbol.start)));
        let mut results = Vec::with_capacity(ranked.len());
        for group in ranked.chunk_by(|(_, a), (_, b)| a.file == b.file) {
            let file = group[0].1.file;
            let source = group
                .iter()
                .any(|(_, item)| item.member.is_some())
                .then(|| self.workspace.source(file))
                .flatten();
            let map = source.as_deref().map(SourceMap::new);
            let mut owner = None;
            let mut positions = Vec::new();
            for (rank, item) in group {
                let symbol = item.symbol;
                let selection = if let Some((member, _)) = item.member {
                    let (Some(source), Some(map)) = (source.as_deref(), map.as_ref()) else {
                        continue;
                    };
                    let Some(end) = map.offset(symbol.range.end) else {
                        continue;
                    };
                    let Some(declaration) = source.get(symbol.end..end) else {
                        continue;
                    };
                    if owner != Some(symbol.start) {
                        positions = crate::syntax::aggregate_members(declaration);
                        owner = Some(symbol.start);
                    }
                    let Some(span) = positions.iter().find(|entry| entry.symbol.name == member)
                    else {
                        continue;
                    };
                    SourceRange::new(
                        map,
                        symbol.end + span.selection.start..symbol.end + span.selection.end,
                    )
                } else {
                    {
                        let range = symbol.selection;
                        SourceRange::coordinates(
                            (range.start.line, range.start.character),
                            (range.end.line, range.end.character),
                        )
                    }
                };
                let kind = match (item.member, symbol.type_info.as_deref()) {
                    (Some((_, crate::syntax::MemberKind::Method)), _) => SearchKind::Method,
                    (Some((_, crate::syntax::MemberKind::Interface)), _) => SearchKind::Interface,
                    (Some(_), _) => SearchKind::Field,
                    (None, Some(info)) if info.is_interface() => SearchKind::Interface,
                    (None, Some(info)) if info.target.is_none() => SearchKind::Struct,
                    _ => SearchKind::Declaration,
                };
                results.push((
                    *rank,
                    SearchSymbol {
                        name: item.name.clone(),
                        container: item.container.clone(),
                        location: SourceLocation {
                            file,
                            range: selection,
                        },
                        kind,
                        symbol,
                    },
                ));
            }
        }
        results.sort_by_key(|(rank, _)| *rank);
        if results.is_empty() {
            None
        } else {
            Some(results.into_iter().map(|(_, value)| value).collect())
        }
    }
}
