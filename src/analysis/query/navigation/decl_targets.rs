//! Declaration-header type-context and member-declaration navigation targets.
use super::Target;
use crate::analysis::obj::MemberDeclaration;
use crate::analysis::scope::Locals;
use crate::syntax::{Expr, ExprId, ExprKind, Lexer, MemberKind, TokenKind, TypeReference};
use crate::text::SourceMap;
use crate::workspace::Workspace;
use std::ops::Range;
use std::path::Path;

pub(in crate::analysis::query) fn member_declaration_target<'a>(
    workspace: &'a Workspace,
    file: &Path,
    source: &str,
    selection: &Range<usize>,
    map: &SourceMap<'_>,
) -> Option<MemberDeclaration<'a>> {
    let (file, stub) = workspace.files.get_key_value(file)?;
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

pub(in crate::analysis::query) fn declaration_type_target<'a>(
    workspace: &'a Workspace,
    file: &Path,
    source: &str,
    selection: &Range<usize>,
    map: &SourceMap<'_>,
) -> Option<Target<'a>> {
    for symbol in &workspace.files[file].symbols {
        if let Some(body) = symbol.body() {
            let start = map.offset(symbol.range.start)?;
            if start <= selection.start && selection.end <= body.start {
                let header = source.get(start..body.start)?;
                source.get(start..body.end)?;
                let parsed = crate::syntax::parse_signature(header);
                let locals = Locals::from_header(source, start, body.clone(), &parsed);
                if let Some(binding) = locals
                    .definition_at(selection, &source[selection.clone()])
                    .filter(|binding| binding.selection == *selection)
                {
                    return Some(Target::Local(binding.selection.clone()));
                }
                for param in parsed
                    .receiver
                    .iter()
                    .chain(&parsed.parameters)
                    .chain(&parsed.results)
                {
                    let ann_start = start + param.annotation.start;
                    let ann_end = start + param.annotation.end;
                    if ann_start <= selection.start
                        && selection.end <= ann_end
                        && let Some(ann_text) = source.get(ann_start..ann_end)
                    {
                        let (parse_slice, sub_offset) = if param.variadic
                            && let Some(rest) = ann_text.trim_start().strip_prefix("...")
                        {
                            (rest, ann_start + (ann_text.len() - rest.len()))
                        } else {
                            (ann_text, ann_start)
                        };
                        let ann_parsed = crate::syntax::parse_type(parse_slice);
                        if ann_parsed.signature_names.iter().any(|sn| {
                            (sub_offset + sn.name.start..sub_offset + sn.name.end) == *selection
                                && &source[selection.clone()] != "_"
                        }) {
                            return Some(Target::Local(selection.clone()));
                        }
                    }
                }
                return type_reference_target(
                    workspace,
                    file,
                    source,
                    selection,
                    &parsed.type_references,
                    start,
                    &locals,
                )
                .or_else(|| {
                    type_value_target(
                        workspace,
                        file,
                        source,
                        selection,
                        &parsed.expressions,
                        start,
                        &locals,
                    )
                });
            }
        } else if symbol.is_nameable() {
            let end = map.offset(symbol.range.end)?;
            if symbol.end <= selection.start && selection.end <= end {
                let mut start = symbol.end;
                // A type alias has an '=' before its type; var/const
                // initializers are expressions and stay on the other path.
                if symbol.is_type()
                    && let Some(token) = Lexer::new(&source[start..end]).next()
                    && token.kind == TokenKind::Assign
                {
                    start += token.span.end;
                }
                let parsed = crate::syntax::parse_type(&source[start..end]);
                if parsed
                    .diagnostics
                    .iter()
                    .any(|error| error.message != "unexpected token after type")
                {
                    continue;
                }
                let locals = Locals::empty();
                if parsed.signature_names.iter().any(|sn| {
                    (start + sn.name.start..start + sn.name.end) == *selection
                        && &source[selection.clone()] != "_"
                }) {
                    return Some(Target::Local(selection.clone()));
                }
                if let Some(target) = type_reference_target(
                    workspace,
                    file,
                    source,
                    selection,
                    &parsed.references,
                    start,
                    &locals,
                )
                .or_else(|| {
                    type_value_target(
                        workspace,
                        file,
                        source,
                        selection,
                        &parsed.expressions,
                        start,
                        &locals,
                    )
                }) {
                    return Some(target);
                }
            }
        }
    }
    None
}

fn type_reference_target<'a>(
    workspace: &'a Workspace,
    file: &Path,
    source: &str,
    selection: &Range<usize>,
    references: &[TypeReference],
    base: usize,
    locals: &Locals,
) -> Option<Target<'a>> {
    let reference = references
        .iter()
        .find(|reference| (base + reference.name.start..base + reference.name.end) == *selection)?;
    let name = &source[selection.clone()];
    if let Some(qualifier) = &reference.qualifier {
        let qualifier = base + qualifier.start..base + qualifier.end;
        let alias = &source[qualifier.clone()];
        if locals.resolve_at(qualifier.start, alias).is_some() {
            return Some(Target::Unresolved);
        }
        Some(
            crate::analysis::resolve::imported_symbols(workspace, file, alias)
                .into_iter()
                .find(|(_, symbol)| symbol.is_type() && symbol.name == name)
                .map_or(Target::Unresolved, |(path, symbol)| {
                    Target::Global(path, symbol)
                }),
        )
    } else if let Some(binding) = locals.definition_at(selection, name) {
        Some(if binding.object_kind.is_type() {
            Target::Local(binding.selection.clone())
        } else {
            Target::Unresolved
        })
    } else {
        Some(
            crate::analysis::resolve::unqualified_symbol(workspace, file, name)
                .filter(|(_, symbol)| symbol.is_type())
                .map_or(Target::Unresolved, |(path, symbol)| {
                    Target::Global(path, symbol)
                }),
        )
    }
}

fn type_value_target<'a>(
    workspace: &'a Workspace,
    file: &Path,
    source: &str,
    selection: &Range<usize>,
    expressions: &[Expr],
    base: usize,
    locals: &Locals,
) -> Option<Target<'a>> {
    for expression in expressions {
        let range = match &expression.kind {
            ExprKind::Name => expression.span.clone(),
            ExprKind::Selector { field, .. } => field.clone(),
            _ => continue,
        };
        if (base + range.start..base + range.end) == *selection {
            if matches!(expression.kind, ExprKind::Name)
                && let Some(binding) = locals.definition_at(selection, &source[selection.clone()])
            {
                return Some(Target::Local(binding.selection.clone()));
            }
            if matches!(expression.kind, ExprKind::Name) {
                return Some(
                    crate::analysis::resolve::unqualified_symbol(
                        workspace,
                        file,
                        &source[selection.clone()],
                    )
                    .map_or(Target::Unresolved, |(path, symbol)| {
                        Target::Global(path, symbol)
                    }),
                );
            }
            if let ExprKind::Selector { operand, .. } = &expression.kind
                && let Some(target) = qualified_package_symbol(
                    workspace,
                    file,
                    source,
                    expressions,
                    base,
                    *operand,
                    &source[selection.clone()],
                )
            {
                return Some(target);
            }
            return Some(Target::Unresolved);
        }
    }
    None
}

/// `alias.Name` in a type/value position: resolve the imported package's
/// exported declaration through the shared import metadata, not spelling.
fn qualified_package_symbol<'a>(
    workspace: &'a Workspace,
    file: &Path,
    source: &str,
    expressions: &[Expr],
    base: usize,
    operand: ExprId,
    field: &str,
) -> Option<Target<'a>> {
    let expression = expressions.get(operand.0)?;
    if !matches!(&expression.kind, ExprKind::Name) {
        return None;
    }
    let alias = &source[base + expression.span.start..base + expression.span.end];
    let probe = crate::bloom::Fingerprint::new(field.as_bytes());
    let mut matches =
        crate::analysis::resolve::imported_symbol_candidates(workspace, file, alias, Some(probe))
            .filter(|(_, symbol)| symbol.receiver.is_none() && symbol.name == field);
    let (path, symbol) = matches.next()?;
    matches
        .next()
        .is_none()
        .then_some(Target::Global(path, symbol))
}
