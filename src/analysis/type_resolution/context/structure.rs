//! Ordered structural field identities, discarded with the signature context.
use super::TypeContext;
use crate::analysis::resolve;
use crate::analysis::type_resolution::signature;
use crate::syntax::{Lexer, TypeSyntax};
use std::path::PathBuf;

impl<'a> TypeContext<'a> {
    pub(super) fn struct_identity(
        &mut self,
        file: &'a PathBuf,
        text: &str,
        parsed: &TypeSyntax,
        environment: Option<usize>,
        depth: usize,
    ) -> Option<String> {
        let container = Lexer::new(text).nth(1)?.span.start;
        let mut identities = Vec::new();
        let mut bytes = 0usize;
        for field in parsed
            .fields
            .iter()
            .filter(|field| field.container == container)
        {
            let name = text.get(field.name.clone())?;
            let package = if resolve::exported(name) {
                String::new()
            } else {
                format!(
                    "{}:{}",
                    file.parent()?.to_str()?,
                    self.workspace.files[file].package
                )
            };
            let tag = match &field.tag {
                Some(span) => crate::syntax::decode_string_literal(text.get(span.clone())?)?,
                None => Vec::new(),
            };
            // A Go tag is a byte string. Hex is injective even for non-UTF-8
            // escapes, unlike lossy UTF-8 decoding or source-spelling identity.
            let tag = tag.iter().map(|byte| format!("{byte:02x}")).collect();
            let node = self.push(file, text.get(field.annotation.clone())?, environment)?;
            let identity = signature::pack(
                "field",
                &[
                    signature::pack("atom", &[name.to_owned()])?,
                    signature::pack("atom", &[package])?,
                    signature::pack("atom", &[field.embedded.to_string()])?,
                    signature::pack("atom", &[tag])?,
                    self.identity(node, depth + 1)?,
                ],
            )?;
            bytes = bytes.checked_add(identity.len() + 24)?;
            if bytes > 16 * 1024 {
                return None;
            }
            identities.push(identity);
        }
        signature::pack("struct", &identities)
    }
}
