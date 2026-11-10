//! Type-context resolution uses proven type-name occurrences, not name matches.
use super::Target;
use crate::analysis::scope::Locals;
use crate::syntax::{ExprKind, TypeReference};
use crate::text::SourceMap;
use crate::workspace::Workspace;
use std::ops::Range;
use std::path::Path;

impl Workspace {
    pub(super) fn type_reference_target<'a>(
        &'a self,
        file: &Path,
        source: &str,
        selection: &Range<usize>,
        references: &[TypeReference],
        base: usize,
        locals: &Locals,
    ) -> Option<Target<'a>> {
        let reference = references.iter().find(|reference| {
            (base + reference.name.start..base + reference.name.end) == *selection
        })?;
        let name = &source[selection.clone()];
        if let Some(qualifier) = &reference.qualifier {
            let qualifier = base + qualifier.start..base + qualifier.end;
            let alias = &source[qualifier.clone()];
            if locals.resolve_at(qualifier.start, alias).is_some() {
                return Some(Target::Unresolved);
            }
            Some(
                self.imported_symbols(file, alias)
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
                self.unqualified_symbol(file, name)
                    .filter(|(_, symbol)| symbol.is_type())
                    .map_or(Target::Unresolved, |(path, symbol)| {
                        Target::Global(path, symbol)
                    }),
            )
        }
    }

    pub(crate) fn declaration_type_target<'a>(
        &'a self,
        file: &Path,
        source: &str,
        selection: &Range<usize>,
        map: &SourceMap<'_>,
    ) -> Option<Target<'a>> {
        for symbol in &self.files[file].symbols {
            if let Some(body) = &symbol.body {
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
                    return self
                        .type_reference_target(
                            file,
                            source,
                            selection,
                            &parsed.type_references,
                            start,
                            &locals,
                        )
                        .or_else(|| {
                            self.type_value_target(
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
                        && let Some(token) = crate::syntax::Lexer::new(&source[start..end]).next()
                        && token.kind == crate::syntax::TokenKind::Assign
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
                    if let Some(target) = self
                        .type_reference_target(
                            file,
                            source,
                            selection,
                            &parsed.references,
                            start,
                            &locals,
                        )
                        .or_else(|| {
                            self.type_value_target(
                                file,
                                source,
                                selection,
                                &parsed.expressions,
                                start,
                                &locals,
                            )
                        })
                    {
                        return Some(target);
                    }
                }
            }
        }
        None
    }

    fn type_value_target<'a>(
        &'a self,
        file: &Path,
        source: &str,
        selection: &Range<usize>,
        expressions: &[crate::syntax::Expr],
        base: usize,
        locals: &Locals,
    ) -> Option<Target<'a>> {
        for (index, expression) in expressions.iter().enumerate() {
            let range = match &expression.kind {
                ExprKind::Name => expression.span.clone(),
                ExprKind::Selector { field, .. } => field.clone(),
                _ => continue,
            };
            if (base + range.start..base + range.end) == *selection {
                if matches!(expression.kind, ExprKind::Name)
                    && let Some(binding) =
                        locals.definition_at(selection, &source[selection.clone()])
                {
                    return Some(Target::Local(binding.selection.clone()));
                }
                let call = self
                    .resolve_call(
                        file,
                        expressions,
                        crate::syntax::ExprId(index),
                        source,
                        base,
                        locals,
                    )
                    .map_or(Target::Unresolved, |(path, symbol)| {
                        Target::Global(path, symbol)
                    });
                if !matches!(call, Target::Unresolved) {
                    return Some(call);
                }
                if let ExprKind::Selector { operand, .. } = &expression.kind
                    && let Some(target) = self.qualified_package_symbol(
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
        &'a self,
        file: &Path,
        source: &str,
        expressions: &[crate::syntax::Expr],
        base: usize,
        operand: crate::syntax::ExprId,
        field: &str,
    ) -> Option<Target<'a>> {
        let expression = expressions.get(operand.0)?;
        if !matches!(&expression.kind, ExprKind::Name) {
            return None;
        }
        let alias = &source[base + expression.span.start..base + expression.span.end];
        let probe = crate::bloom::Fingerprint::new(field.as_bytes());
        let mut matches = self
            .imported_symbol_candidates(file, alias, Some(probe))
            .filter(|(_, symbol)| symbol.receiver.is_none() && symbol.name == field);
        let (path, symbol) = matches.next()?;
        matches
            .next()
            .is_none()
            .then_some(Target::Global(path, symbol))
    }
}
