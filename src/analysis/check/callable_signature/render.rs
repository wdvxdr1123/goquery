//! Signature labels are derived from current declarations, never cached source.
use super::{CallableSignature, ParameterLabel};
use crate::analysis::type_resolution::MemberTypes;
use crate::index::IndexedSymbol as Symbol;
use crate::syntax::{Parameter, Signature, TokenKind};
use crate::text::SourceMap;
use crate::workspace::Workspace;
use std::path::Path;

pub(super) const MAX_LABEL_BYTES: usize = 16 * 1024;

#[derive(Clone, Copy)]
pub(super) enum SignatureTypes<'a, 'workspace> {
    Call,
    Member(&'a MemberTypes<'workspace>),
    Declaration(&'a [&'a str]),
}

impl Workspace {
    pub(crate) fn signature_information(
        &self,
        file: &Path,
        origin: &Path,
        symbol: &Symbol,
    ) -> Option<(CallableSignature, bool)> {
        let source = self.source(origin)?;
        let map = SourceMap::new(&source);
        let start = map.offset(symbol.range.start)?;
        let end = symbol
            .body()
            .map(|body| body.start)
            .or_else(|| map.offset(symbol.range.end))?;
        let header = source.get(start..end)?;
        if header.len() > MAX_LABEL_BYTES {
            return None;
        }
        let signature = crate::syntax::parse_signature(header);
        self.signature_from_parsed(file, origin, header, &signature, &symbol.name)
    }

    pub(crate) fn signature_from_parsed(
        &self,
        file: &Path,
        origin: &Path,
        header: &str,
        signature: &Signature,
        name: &str,
    ) -> Option<(CallableSignature, bool)> {
        self.signature_with_types(file, origin, header, signature, name, SignatureTypes::Call)
    }

    pub(crate) fn declared_signature(
        &self,
        file: &Path,
        origin: &Path,
        header: &str,
        signature: &Signature,
        name: &str,
    ) -> Option<(CallableSignature, bool)> {
        let names: Vec<_> = signature
            .type_parameters
            .iter()
            .map(|p| p.name.as_ref().map(|span| &header[span.clone()]))
            .collect::<Option<_>>()?;
        self.signature_with_types(
            file,
            origin,
            header,
            signature,
            name,
            SignatureTypes::Declaration(&names),
        )
    }

    pub(super) fn signature_with_types(
        &self,
        file: &Path,
        origin: &Path,
        header: &str,
        signature: &Signature,
        name: &str,
        types: SignatureTypes<'_, '_>,
    ) -> Option<(CallableSignature, bool)> {
        if !signature.diagnostics.is_empty()
            || !signature.type_parameters.is_empty()
                && !matches!(types, SignatureTypes::Declaration(_))
            || matches!(types, SignatureTypes::Call)
                && signature.receiver.iter().any(|receiver| {
                    crate::syntax::Lexer::new(&header[receiver.annotation.clone()])
                        .any(|token| token.kind == TokenKind::LBrack)
                })
        {
            return None;
        }
        signature_label(header, signature, name, |annotation| match types {
            SignatureTypes::Member(types) => types.render(annotation),
            SignatureTypes::Declaration(names) => {
                self.signature_type_scoped(file, origin, annotation, names)
            }
            SignatureTypes::Call => self.signature_type(file, origin, annotation),
        })
    }

    pub(crate) fn signature_type(
        &self,
        file: &Path,
        origin: &Path,
        source: &str,
    ) -> Option<String> {
        self.signature_type_scoped(file, origin, source, &[])
    }
}

/// Keep parameter names and protocol label limits shared across declaration,
/// member and native instantiated signatures. Only type rendering varies.
pub(crate) fn signature_label(
    header: &str,
    signature: &Signature,
    name: &str,
    mut annotation_type: impl FnMut(&str) -> Option<String>,
) -> Option<(CallableSignature, bool)> {
    let mut raw_bytes = name.len().checked_add(8)?;
    for parameter in signature.parameters.iter().chain(&signature.results) {
        raw_bytes = raw_bytes
            .checked_add(parameter.annotation.len())?
            .checked_add(parameter.name.as_ref().map_or(0, |name| name.len()))?
            .checked_add(8)?;
        if raw_bytes > MAX_LABEL_BYTES {
            return None;
        }
    }
    if raw_bytes > MAX_LABEL_BYTES {
        return None;
    }
    let mut parameter = |parameter: &Parameter| -> Option<String> {
        let annotation = &header[parameter.annotation.clone()];
        let ty = annotation_type(annotation)?;
        let ty = if parameter.variadic {
            format!("...{ty}")
        } else {
            ty
        };
        Some(
            parameter
                .name
                .as_ref()
                .map_or(ty.clone(), |name| format!("{} {ty}", &header[name.clone()])),
        )
    };
    let mut expanded = name.len() + 8;
    let mut parameters = Vec::new();
    for input in &signature.parameters {
        let label = parameter(input)?;
        expanded = expanded.checked_add(label.len() + 2)?;
        if expanded > MAX_LABEL_BYTES {
            return None;
        }
        parameters.push(ParameterLabel { label });
    }
    let mut results = Vec::new();
    for result in &signature.results {
        let label = parameter(result)?;
        expanded = expanded.checked_add(label.len() + 2)?;
        if expanded > MAX_LABEL_BYTES {
            return None;
        }
        results.push(label);
    }
    let mut label = format!(
        "{}({})",
        name,
        parameters
            .iter()
            .map(|p| p.label.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    if !results.is_empty() {
        label.push(' ');
        let parentheses = results.len() > 1 || signature.results[0].name.is_some();
        if parentheses {
            label.push('(');
        }
        label.push_str(&results.join(", "));
        if parentheses {
            label.push(')');
        }
    }
    let variadic = signature
        .parameters
        .last()
        .is_some_and(|parameter| parameter.variadic);
    Some((
        CallableSignature {
            label,
            parameters,
            active_parameter: None,
        },
        variadic,
    ))
}

impl Workspace {
    pub(crate) fn signature_type_scoped(
        &self,
        file: &Path,
        origin: &Path,
        source: &str,
        parameters: &[&str],
    ) -> Option<String> {
        if source.len() > MAX_LABEL_BYTES {
            return None;
        }
        let parsed = crate::syntax::parse_type(source);
        let mut substitutions = Vec::new();
        let mut substitution_bytes = 0usize;
        for reference in parsed.references {
            if reference.qualifier.is_none()
                && parameters.contains(&&source[reference.name.clone()])
            {
                continue;
            }
            let start = reference
                .qualifier
                .as_ref()
                .map_or(reference.name.start, |qualifier| qualifier.start);
            let spelling = reference.qualifier.as_ref().map_or_else(
                || source[reference.name.clone()].to_owned(),
                |qualifier| {
                    format!(
                        "{}.{}",
                        &source[qualifier.clone()],
                        &source[reference.name.clone()]
                    )
                },
            );
            let Some((path, symbol)) = self.type_declaration(origin, &spelling) else {
                continue;
            };
            let same = file.parent() == path.parent()
                && self.files[file].package == self.files[path].package;
            let replacement = if same {
                if symbol.name.len() > MAX_LABEL_BYTES {
                    return None;
                }
                symbol.name.clone()
            } else {
                let import = self.import_path(path);
                let imported = self.files[file].imports.iter().find(|candidate| {
                    !candidate.is_blank() && Some(candidate.path.as_str()) == import.as_deref()
                });
                let dot = imported.is_some_and(|import| import.is_dot());
                let alias = imported
                    .and_then(|candidate| self.import_name(candidate))
                    .unwrap_or(&self.files[path].package);
                if alias
                    .len()
                    .saturating_add(symbol.name.len())
                    .saturating_add(1)
                    > MAX_LABEL_BYTES
                {
                    return None;
                }
                if dot {
                    symbol.name.clone()
                } else {
                    format!("{alias}.{}", symbol.name)
                }
            };
            substitution_bytes = substitution_bytes.checked_add(replacement.len())?;
            if substitution_bytes > MAX_LABEL_BYTES {
                return None;
            }
            substitutions.push((start..reference.name.end, replacement));
        }
        substitutions.sort_unstable_by_key(|(span, _)| span.start);
        let mut output = String::new();
        let mut end = 0;
        for (span, replacement) in substitutions {
            if span.start < end {
                continue;
            }
            if output
                .len()
                .saturating_add(span.start - end)
                .saturating_add(replacement.len())
                > MAX_LABEL_BYTES
            {
                return None;
            }
            output.push_str(&source[end..span.start]);
            output.push_str(&replacement);
            end = span.end;
        }
        if output.len().saturating_add(source.len() - end) > MAX_LABEL_BYTES {
            return None;
        }
        output.push_str(&source[end..]);
        Some(output.trim().to_owned())
    }
}
