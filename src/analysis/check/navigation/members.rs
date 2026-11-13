//! Declaration-name navigation without turning fields into package globals.
use crate::analysis::member::MemberDeclaration;
use crate::syntax::MemberKind;
use crate::text::SourceMap;
use crate::workspace::Workspace;
use std::ops::Range;
use std::path::Path;

impl Workspace {
    pub(crate) fn member_declaration_target<'a>(
        &'a self,
        file: &Path,
        source: &str,
        selection: &Range<usize>,
        map: &SourceMap<'_>,
    ) -> Option<MemberDeclaration<'a>> {
        let (file, stub) = self.files.get_key_value(file)?;
        for owner in &stub.symbols {
            let Some(info) = &owner.type_info else {
                continue;
            };
            let end = map.offset(owner.range.end)?;
            if !(owner.end <= selection.start && selection.end <= end) {
                continue;
            }
            let direct = info
                .members
                .iter()
                .find(|name| **name == source[selection.clone()])
                .map(|name| (name.as_str(), false));
            // Struct embeddings declare an implicit field at the final type
            // identifier. Interface embeddings only refer to another type.
            let embedded = (!info.is_interface())
                .then(|| {
                    info.embedded.iter().find_map(|annotation| {
                        let name = crate::analysis::scope::named_root(annotation)
                            .rsplit('.')
                            .next()?;
                        (name == &source[selection.clone()]).then_some((name, true))
                    })
                })
                .flatten();
            let Some((name, embedded)) = direct.or(embedded) else {
                continue;
            };
            let declaration = source.get(owner.end..end)?;
            let member = MemberDeclaration {
                file,
                owner,
                name,
                kind: if info.is_interface() {
                    MemberKind::Method
                } else {
                    MemberKind::Field
                },
                embedded,
            };
            let entries = crate::syntax::aggregate_members(declaration);
            let entry = member.entry(declaration, &entries)?;
            let span = member.selection(declaration, entry)?;
            if (owner.end + span.start..owner.end + span.end) == *selection {
                return Some(member);
            }
        }
        None
    }
}
