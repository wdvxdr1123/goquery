//! Query-local callable signature labels, type-spelling qualification, and presentation normalization.
use crate::analysis::budget::{MAX_BYTES, MAX_DEPTH, MAX_NODES, RELATION_STEPS};
use crate::analysis::resolve::resolve_type_declaration;
use crate::syntax::{Lexer, Parameter, Signature, TokenKind};
use crate::workspace::Workspace;
use std::ops::Range;
use std::path::Path;

pub(in crate::analysis::query) const MAX_LABEL_BYTES: usize = 16 * 1024;

pub(crate) struct ParameterLabel {
    pub(crate) label: String,
}

pub(crate) struct CallableSignature {
    pub(crate) label: String,
    pub(crate) parameters: Vec<ParameterLabel>,
    pub(crate) active_parameter: Option<u32>,
}

pub(in crate::analysis::query) fn signature_from_parsed(
    workspace: &Workspace,
    file: &Path,
    origin: &Path,
    header: &str,
    signature: &Signature,
    name: &str,
) -> Option<(CallableSignature, bool)> {
    if !signature.diagnostics.is_empty()
        || !signature.type_parameters.is_empty()
        || signature.receiver.iter().any(|receiver| {
            Lexer::new(&header[receiver.annotation.clone()])
                .any(|token| token.kind == TokenKind::LBrack)
        })
    {
        return None;
    }
    signature_label(header, signature, name, |annotation| {
        signature_type(workspace, file, origin, annotation)
    })
}

pub(in crate::analysis::query) fn declared_signature(
    workspace: &Workspace,
    file: &Path,
    origin: &Path,
    header: &str,
    signature: &Signature,
    name: &str,
) -> Option<(CallableSignature, bool)> {
    if !signature.diagnostics.is_empty() {
        return None;
    }
    let names: Vec<_> = signature
        .type_parameters
        .iter()
        .map(|p| p.name.as_ref().map(|span| &header[span.clone()]))
        .collect::<Option<_>>()?;
    signature_label(header, signature, name, |annotation| {
        signature_type_scoped(workspace, file, origin, annotation, &names)
    })
}

pub(in crate::analysis::query) fn signature_type(
    workspace: &Workspace,
    file: &Path,
    origin: &Path,
    source: &str,
) -> Option<String> {
    signature_type_scoped(workspace, file, origin, source, &[])
}

pub(in crate::analysis::query) fn signature_type_scoped(
    workspace: &Workspace,
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
        if reference.qualifier.is_none() && parameters.contains(&&source[reference.name.clone()]) {
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
        let Some((path, symbol)) = resolve_type_declaration(workspace, origin, &spelling) else {
            continue;
        };
        let same = file.parent() == path.parent()
            && workspace.files[file].package == workspace.files[path].package;
        let replacement = if same {
            if symbol.name.len() > MAX_LABEL_BYTES {
                return None;
            }
            symbol.name.clone()
        } else {
            let import = workspace.import_path(path);
            let imported = workspace.files[file].imports.iter().find(|candidate| {
                !candidate.is_blank() && Some(candidate.path.as_str()) == import.as_deref()
            });
            let dot = imported.is_some_and(|import| import.is_dot());
            let alias = imported
                .and_then(|candidate| workspace.import_name(candidate))
                .unwrap_or(&workspace.files[path].package);
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

/// Keep parameter names and protocol label limits shared across declaration,
/// member and native instantiated signatures. Only type rendering varies.
pub(in crate::analysis::query) fn signature_label(
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
        let ty = function_types(&annotation_type(annotation)?)?;
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

struct Insertion {
    offset: usize,
    end: usize,
    annotation: Option<Range<usize>>,
    prefix: &'static str,
}

pub(in crate::analysis::query) fn function_types(text: &str) -> Option<String> {
    if text.len() > MAX_BYTES {
        return None;
    }
    if !Lexer::new(text).any(|token| matches!(token.kind, TokenKind::Func | TokenKind::Interface)) {
        return Some(text.to_owned());
    }
    let source = strip_comments(text);
    let mut insertions = Vec::new();
    let mut covered = 0;
    let mut nodes = 0usize;
    for token in Lexer::new(&source) {
        if token.span.start < covered
            || !matches!(token.kind, TokenKind::Func | TokenKind::Interface)
        {
            continue;
        }
        let base = token.span.start;
        let syntax = crate::syntax::parse_type_prefix(&source[base..]);
        if !syntax.diagnostics().is_empty() {
            continue;
        }
        nodes = nodes.checked_add(syntax.type_nodes().len() + syntax.signature_names.len())?;
        if nodes > MAX_NODES {
            return None;
        }
        covered = base + syntax.span.end;
        let mut names = syntax.signature_names;
        names.sort_unstable_by_key(|name| {
            (name.annotation.start, name.annotation.end, name.name.start)
        });
        for (index, name) in names.iter().enumerate() {
            if names
                .get(index + 1)
                .is_some_and(|next| next.annotation == name.annotation)
            {
                insertions.push(Insertion {
                    offset: base + name.name.end,
                    end: base + name.name.end,
                    annotation: Some(base + name.annotation.start..base + name.annotation.end),
                    prefix: " ",
                });
            } else {
                let gap = base + name.name.end..base + name.annotation.start;
                let prefix = if name.variadic { " ..." } else { " " };
                if source.get(gap.clone())? != prefix {
                    insertions.push(Insertion {
                        offset: gap.start,
                        end: gap.end,
                        annotation: None,
                        prefix,
                    });
                }
            }
        }
    }
    insertions.sort_unstable_by_key(|edit| edit.offset);
    let mut renderer = Renderer {
        source: &source,
        insertions: &insertions,
        output: String::new(),
        steps: RELATION_STEPS,
    };
    renderer.fragment(0..source.len(), 0)?;
    canonical_separators(&canonical_literals(&renderer.output)?)
}

struct Renderer<'a> {
    source: &'a str,
    insertions: &'a [Insertion],
    output: String,
    steps: usize,
}

impl Renderer<'_> {
    fn push(&mut self, text: &str) -> Option<()> {
        if self.output.len().checked_add(text.len())? > MAX_BYTES {
            return None;
        }
        self.output.push_str(text);
        Some(())
    }

    fn fragment(&mut self, range: Range<usize>, depth: usize) -> Option<()> {
        if depth >= MAX_DEPTH {
            return None;
        }
        let mut start = range.start;
        let first = self.insertions.partition_point(|edit| edit.offset < start);
        let last = self
            .insertions
            .partition_point(|edit| edit.offset < range.end);
        for index in first..last {
            self.steps = self.steps.checked_sub(1)?;
            let insertion = &self.insertions[index];
            let offset = insertion.offset;
            let end = insertion.end;
            let prefix = insertion.prefix;
            let annotation = insertion.annotation.clone();
            self.push(self.source.get(start..offset)?)?;
            self.push(prefix)?;
            if let Some(annotation) = annotation {
                self.fragment(annotation, depth + 1)?;
            }
            start = end;
        }
        self.push(self.source.get(start..range.end)?)
    }
}

fn canonical_separators(text: &str) -> Option<String> {
    let mut output = String::new();
    let mut start = 0;
    let mut previous = None;
    for token in Lexer::new(text) {
        let result_list = token.kind == TokenKind::LParen && previous == Some(TokenKind::RParen);
        previous = Some(token.kind);
        if token.kind != TokenKind::Comma && !result_list {
            continue;
        }
        let prefix = text.get(start..token.span.start)?.trim_end();
        if output.len().checked_add(prefix.len() + 2)? > MAX_BYTES {
            return None;
        }
        output.push_str(prefix);
        if result_list {
            output.push(' ');
            start = token.span.start;
            continue;
        }
        output.push_str(", ");
        let rest = &text[token.span.end..];
        start = token.span.end + rest.len() - rest.trim_start().len();
    }
    if output.len().checked_add(text.len() - start)? > MAX_BYTES {
        return None;
    }
    output.push_str(&text[start..]);
    Some(output)
}

fn canonical_literals(text: &str) -> Option<String> {
    let mut output = String::new();
    let mut start = 0;
    for token in Lexer::new(text).filter(|token| token.kind == TokenKind::String) {
        let bytes = crate::syntax::decode_string_literal(text.get(token.span.clone())?)?;
        let quoted = crate::syntax::quote_bytes(&bytes);
        let prefix = text.get(start..token.span.start)?;
        if output.len().checked_add(prefix.len() + quoted.len())? > MAX_BYTES {
            return None;
        }
        output.push_str(prefix);
        output.push_str(&quoted);
        start = token.span.end;
    }
    if output.len().checked_add(text.len() - start)? > MAX_BYTES {
        return None;
    }
    output.push_str(&text[start..]);
    Some(output)
}

/// Remove comments without interpreting literal contents.
pub(in crate::analysis::query) fn strip_comments(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut start = 0;
    for token in Lexer::with_comments(text).filter(|token| token.kind == TokenKind::Comment) {
        output.push_str(&text[start..token.span.start]);
        output.push(if text[token.span.clone()].contains('\n') {
            '\n'
        } else {
            ' '
        });
        start = token.span.end;
    }
    output.push_str(&text[start..]);
    output
}
