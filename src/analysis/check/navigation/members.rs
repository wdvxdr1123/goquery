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
            let name = info
                .members
                .iter()
                .find(|name| **name == source[selection.clone()]);
            let Some(name) = name else {
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
                embedded: false,
            };
            let entries = crate::syntax::aggregate_members(declaration);
            let entry = member.entry(declaration, &entries)?;
            if (owner.end + entry.selection.start..owner.end + entry.selection.end) == *selection {
                return Some(member);
            }
        }
        None
    }
}
