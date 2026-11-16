//! Concrete receiver signatures keep implicit type parameter names scoped.
use crate::analysis::query::signatures::label::{
    CallableSignature, MAX_LABEL_BYTES, signature_from_parsed, signature_label,
};
use crate::analysis::scope as locals;
use crate::analysis::types::lookup::ScopedOwnerEnv;
use crate::index::IndexedSymbol as Symbol;
use crate::syntax::TokenKind;
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
    types: ScopedOwnerEnv<'a>,
}

pub(super) fn signature_in_source<'a>(
    workspace: &'a Workspace,
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
        return signature_from_parsed(
            workspace,
            location.file,
            location.origin,
            header,
            &signature,
            &method.name,
        );
    }
    let (owner_file, owner) = crate::analysis::resolve::resolve_type_declaration(
        workspace,
        location.origin,
        method.receiver.as_deref()?,
    )?;
    let identity = (owner_file, owner.start);
    if let Some(prepared) = prepared
        .as_mut()
        .filter(|prepared| prepared.owner == identity)
    {
        prepared.types.rename_parameters(names)?;
    } else {
        let (scope, receiver) = receiver?;
        let types = ScopedOwnerEnv::for_method(
            workspace,
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
    if !signature.diagnostics.is_empty()
        || !signature.type_parameters.is_empty()
        || (signature.receiver.iter().any(|r| {
            crate::syntax::Lexer::new(&header[r.annotation.clone()])
                .any(|token| token.kind == TokenKind::LBrack)
        }) && prepared.is_none())
    {
        return None;
    }
    let types = &prepared.as_ref()?.types;
    signature_label(header, &signature, &method.name, |annotation| {
        types.render(annotation)
    })
}
