//! Preserve a verified declaration scope at the legacy text boundary.
use super::{MAX_NODES, Ty, TypeArena, TypeId};
use std::path::Path;

impl<'a> TypeArena<'a> {
    /// A shadowed alias may have a usable transparent spelling. Keep defined
    /// type identity and prove the rendered spelling just like other exports.
    pub(crate) fn render_unaliased(&mut self, input: TypeId, consumer: &Path) -> Option<String> {
        let target = self.unalias(input)?;
        if target == input {
            return None;
        }
        self.render_resolved(target, consumer)
    }

    pub(crate) fn render_declared(
        &mut self,
        input: TypeId,
        preferred: &Path,
        consumer: &Path,
    ) -> Option<(&'a Path, String)> {
        let mut scopes = vec![self.workspace.files.get_key_value(preferred)?.0.as_path()];
        let mut pending = vec![input];
        let mut visited = Vec::new();
        while let Some(ty) = pending.pop() {
            self.step()?;
            if visited.contains(&ty) {
                continue;
            }
            visited.push(ty);
            let origin = match self.nodes[usize::from(ty.0)].clone() {
                Ty::Named {
                    declaration,
                    arguments,
                } => {
                    pending.extend_from_slice(&self.arguments[arguments]);
                    Some(declaration.file)
                }
                Ty::Struct { origin, fields } => {
                    pending.extend(self.fields[fields].iter().map(|field| field.ty));
                    Some(origin)
                }
                Ty::Interface {
                    origin, methods, ..
                } => {
                    for method in &self.methods[methods] {
                        pending.push(method.signature);
                        if !scopes.contains(&method.origin) {
                            scopes.push(method.origin);
                        }
                    }
                    Some(origin)
                }
                Ty::Signature {
                    parameters,
                    results,
                    ..
                } => {
                    pending.extend_from_slice(&self.arguments[parameters]);
                    pending.extend_from_slice(&self.arguments[results]);
                    None
                }
                Ty::Pointer(element)
                | Ty::Slice(element)
                | Ty::Error(element)
                | Ty::Array { element, .. }
                | Ty::Chan { element, .. } => {
                    pending.push(element);
                    None
                }
                Ty::Map { key, value } => {
                    pending.extend([key, value]);
                    None
                }
                Ty::Basic(_) | Ty::Parameter(_) => None,
                Ty::LocalDeclaration { .. } => return None,
            };
            if let Some(origin) = origin
                && !scopes.contains(&origin)
            {
                scopes.push(origin);
            }
            if pending.len() > MAX_NODES || scopes.len() > MAX_NODES {
                return None;
            }
        }
        // A candidate must express the entire compound type, not just one leaf.
        // Reparse in that scope and prove identity before exporting any text.
        for scope in scopes {
            if scope != consumer
                && let Some(text) = self.render_resolved(input, scope)
            {
                return Some((scope, text));
            }
        }
        None
    }
}
