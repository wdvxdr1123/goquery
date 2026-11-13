//! Concrete receiver signatures keep implicit type parameter names scoped.
use super::CallableSignature;
use super::render::{MAX_LABEL_BYTES, SignatureTypes};
use crate::analysis::scope as locals;
use crate::analysis::type_resolution::MemberTypes;
use crate::index::IndexedSymbol as Symbol;
use crate::text::SourceMap;
use crate::workspace::Workspace;
use std::path::{Path, PathBuf};

pub(crate) struct SignatureSource<'a> {
    pub file: &'a Path,
    pub origin: &'a Path,
    pub source: &'a str,
    pub map: &'a SourceMap<'a>,
}

pub(crate) struct PreparedMethods<'a> {
    owner: (&'a PathBuf, usize),
    types: MemberTypes<'a>,
}

impl Workspace {
    pub(crate) fn scoped_method_signature(
        &self,
        file: &Path,
        origin: &Path,
        method: &Symbol,
        receiver: (&Path, &str),
    ) -> Option<(CallableSignature, bool)> {
        let source = self.source(origin)?;
        let map = SourceMap::new(&source);
        self.signature_in_source(
            &SignatureSource {
                file,
                origin,
                source: &source,
                map: &map,
            },
            method,
            Some(receiver),
            &mut None,
        )
    }

    pub(crate) fn signature_in_source<'a>(
        &'a self,
        location: &SignatureSource<'_>,
        method: &Symbol,
        receiver: Option<(&Path, &str)>,
        prepared: &mut Option<PreparedMethods<'a>>,
    ) -> Option<(CallableSignature, bool)> {
        let start = location.map.offset(method.range.start)?;
        let end = method
            .body()
            .map(|body| body.start)
            .or_else(|| location.map.offset(method.range.end))?;
        let header = location.source.get(start..end)?;
        if header.len() > MAX_LABEL_BYTES {
            return None;
        }
        let signature = crate::syntax::parse_signature(header);
        let names: Vec<_> = signature
            .receiver
            .iter()
            .flat_map(|receiver| locals::receiver_parameters(header, &receiver.annotation))
            .map(|span| header[span].to_owned())
            .collect();
        if names.is_empty() || receiver.is_none() {
            return self.signature_from_parsed(
                location.file,
                location.origin,
                header,
                &signature,
                &method.name,
            );
        }
        let (owner_file, owner) =
            self.type_declaration(location.origin, method.receiver.as_deref()?)?;
        let identity = (owner_file, owner.start);
        if let Some(prepared) = prepared
            .as_mut()
            .filter(|prepared| prepared.owner == identity)
        {
            prepared.types.rename_parameters(names)?;
        } else {
            let (scope, receiver) = receiver?;
            let types = self.scoped_method_types(
                location.file,
                scope,
                receiver,
                location.origin,
                method,
                names,
            )?;
            *prepared = Some(PreparedMethods {
                owner: identity,
                types,
            });
        }
        let types = &prepared.as_ref()?.types;
        self.signature_with_types(
            location.file,
            location.origin,
            header,
            &signature,
            &method.name,
            SignatureTypes::Member(types),
        )
    }
}
