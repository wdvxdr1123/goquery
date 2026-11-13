//! Query-local interface relations and symbolic implementation environments.
use super::MemberTypes;
use super::context::{OwnerKind, TypeContext};
use crate::analysis::method_set::ReceiverMember;
use crate::index::IndexedSymbol as Symbol;
use crate::workspace::Workspace;
use std::path::Path;

impl Workspace {
    pub(crate) fn implementation_types(
        &self,
        file: &Path,
        receiver: &Symbol,
        member: ReceiverMember<'_>,
        names: Vec<String>,
    ) -> Option<MemberTypes<'_>> {
        let file = self.files.get_key_value(file)?.0;
        let (origin, owner_file, owner, kind) = match member {
            ReceiverMember::Method(origin, method) => {
                let (owner_file, owner) = if origin.parent() == file.parent()
                    && self.files[origin].package == self.files[file].package
                    && method.receiver.as_deref() == Some(receiver.name.as_str())
                {
                    (file, receiver)
                } else {
                    self.type_declaration(origin, method.receiver.as_deref()?)?
                };
                (origin, owner_file, owner, OwnerKind::Receiver)
            }
            ReceiverMember::Declaration(member) => {
                (member.file, member.file, member.owner, OwnerKind::Aggregate)
            }
        };
        let origin = self.files.get_key_value(origin)?.0;
        let mut context = TypeContext::new(self);
        // Member lookup has already proved the declaring owner and promotion
        // path. A nongeneric owner has no formals to substitute, even when the
        // member is promoted into a different or generic receiver. Its signature
        // is interpreted in `origin`, so no embedding graph needs rebuilding.
        let mut environment = if !owner.generic {
            None
        } else {
            let input = context.implementation_input(file, receiver)?;
            context.owner_environment(input, owner_file, owner, kind)?.1
        };
        if matches!(kind, OwnerKind::Receiver) {
            environment = context.renamed_environment(environment, names)?;
        }
        Some(MemberTypes {
            context,
            origin,
            environment,
            file,
        })
    }

    /// Generic named indirections need written arguments during metadata lookup.
    /// Simple aggregate declarations can use their stub names without a read.
    pub(crate) fn implementation_spelling(&self, file: &Path, owner: &Symbol) -> Option<String> {
        if !owner
            .type_info
            .as_ref()
            .and_then(|info| info.target.as_ref())
            .is_some_and(|target| target.generic)
        {
            return Some(owner.name.clone());
        }
        let source = self.source(file)?;
        let names = crate::analysis::scope::type_parameters(source.get(owner.end..)?)?;
        let result = format!("{}[{}]", owner.name, names.join(","));
        (result.len() <= 16 * 1024).then_some(result)
    }
}
