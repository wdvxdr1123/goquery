//! Source names decorate native signatures without participating in identity.
use super::{MAX_BYTES, TypeArena, TypeId};
use crate::syntax::Signature;

pub(crate) struct CallableNames {
    pub(crate) receiver: String,
    pub(crate) parameters: Vec<String>,
    pub(crate) results: Vec<String>,
}

pub(super) struct SignatureNames {
    ty: TypeId,
    start: u32,
    end: u32,
    parameters: u16,
    results: u16,
}

impl TypeArena<'_> {
    pub(super) fn attach_signature_names(
        &mut self,
        ty: TypeId,
        header: &str,
        signature: &Signature,
    ) {
        let name = |parameter: &crate::syntax::Parameter| {
            parameter
                .name
                .as_ref()
                .and_then(|span| header.get(span.clone()))
                .unwrap_or("")
                .to_owned()
        };
        let names = CallableNames {
            receiver: signature.receiver.first().map_or(String::new(), name),
            parameters: signature.parameters.iter().map(name).collect(),
            results: signature.results.iter().map(name).collect(),
        };
        self.store_signature_names(ty, &names);
    }

    fn store_signature_names(&mut self, ty: TypeId, names: &CallableNames) {
        let strings = std::iter::once(&names.receiver)
            .chain(&names.parameters)
            .chain(&names.results);
        let bytes: usize = strings.clone().map(|name| name.len() + 1).sum();
        if self.signature_names.len() >= self.node_limit
            || bytes > MAX_BYTES
            || self.signature_name_bytes.len() + bytes > self.byte_limit.min(MAX_BYTES)
        {
            return;
        }
        let (Ok(parameters), Ok(results)) = (
            u16::try_from(names.parameters.len()),
            u16::try_from(names.results.len()),
        ) else {
            return;
        };
        let start = self.signature_name_bytes.len() as u32;
        for name in strings {
            self.signature_name_bytes.push_str(name);
            self.signature_name_bytes.push('\0');
        }
        self.signature_names.push(SignatureNames {
            ty,
            start,
            end: self.signature_name_bytes.len() as u32,
            parameters,
            results,
        });
    }

    pub(crate) fn callable_names(&mut self, ty: TypeId) -> Option<CallableNames> {
        let ty = self.underlying(ty)?;
        let (text, parameters, results) = self.signature_name_text(ty)?;
        let mut strings = text.split_terminator('\0');
        Some(CallableNames {
            receiver: strings.next()?.to_owned(),
            parameters: strings
                .by_ref()
                .take(usize::from(parameters))
                .map(str::to_owned)
                .collect(),
            results: strings
                .take(usize::from(results))
                .map(str::to_owned)
                .collect(),
        })
    }

    pub(super) fn signature_name_text(&self, ty: TypeId) -> Option<(&str, u16, u16)> {
        let names = self
            .signature_names
            .iter()
            .rev()
            .find(|names| names.ty == ty)?;
        let text = self
            .signature_name_bytes
            .get(names.start as usize..names.end as usize)?;
        Some((text, names.parameters, names.results))
    }

    pub(super) fn copy_signature_names(
        &mut self,
        from: TypeId,
        to: TypeId,
        method_expression: bool,
    ) {
        if let Some(mut names) = self.callable_names(from) {
            if method_expression {
                names
                    .parameters
                    .insert(0, std::mem::take(&mut names.receiver));
            }
            self.store_signature_names(to, &names);
        }
    }
}
