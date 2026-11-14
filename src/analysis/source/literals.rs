//! Expected container types for literals with elided element/key types.
use super::build::LiteralType;
use super::{Role, SourceAnalysis};
use crate::analysis::types::TypeForm;
use crate::syntax::{Expr, ExprId, ExprKind};
use crate::workspace::Workspace;
use std::collections::HashMap;
use std::ops::Range;
use std::path::Path;

impl SourceAnalysis {
    pub(super) fn literal_elements(&mut self, expressions: &[Expr], base: usize) {
        // Sibling elided literals project the same container type. Keep one
        // descriptor per projection instead of one per literal occurrence.
        let mut contexts = HashMap::new();
        for (id, expression) in expressions.iter().enumerate() {
            let ExprKind::Composite {
                literal_type: Some(ty),
                elements,
            } = &expression.kind
            else {
                continue;
            };
            if !elided_children(expressions, elements).any(|(id, _)| {
                let span = &expressions[id.0].span;
                self.contains_name(&(base + span.start..base + span.end))
            }) {
                continue;
            }
            if self.literal_types.len() >= 4096 {
                self.literal_limit = true;
                return;
            }
            let ty = &expressions[ty.0].span;
            let context = self.literal_types.len();
            self.literal_types
                .push(LiteralType::Explicit(base + ty.start..base + ty.end));
            let mut pending = vec![(ExprId(id), context, 0)];
            while let Some((id, context, depth)) = pending.pop() {
                let ExprKind::Composite {
                    literal_type,
                    elements,
                } = &expressions[id.0].kind
                else {
                    continue;
                };
                if literal_type.is_none() {
                    for element in elements {
                        if let ExprKind::KeyValue { key, .. } = &expressions[element.0].kind
                            && matches!(expressions[key.0].kind, ExprKind::Name)
                        {
                            let key = &expressions[key.0].span;
                            self.mark(
                                base + key.start..base + key.end,
                                Role::LiteralElementKey(context),
                            );
                        }
                    }
                }
                if depth >= 64 {
                    self.literal_limit |= elided_children(expressions, elements).next().is_some();
                    continue;
                }
                for (child, key) in elided_children(expressions, elements) {
                    let span = &expressions[child.0].span;
                    if !self.contains_name(&(base + span.start..base + span.end)) {
                        continue;
                    }
                    let next = match contexts.get(&(context, key)) {
                        Some(next) => *next,
                        None => {
                            if self.literal_types.len() >= 4096 {
                                self.literal_limit = true;
                                return;
                            }
                            let next = self.literal_types.len();
                            self.literal_types.push(LiteralType::Element {
                                container: context,
                                key,
                            });
                            contexts.insert((context, key), next);
                            next
                        }
                    };
                    pending.push((child, next, depth + 1));
                }
            }
        }
    }

    /// Projection spans stay query-local and share the existing scoped Go type
    /// resolver. In particular, map keys are expressions, never struct labels.
    pub(crate) fn literal_element_type(
        &self,
        workspace: &Workspace,
        file: &Path,
        source: &str,
        context: usize,
    ) -> Option<(String, usize)> {
        let (span, projections) = self.literal_element_projection(context)?;
        let mut ty = source.get(span.clone())?.to_owned();
        for key in projections.into_iter().rev() {
            let form = workspace.type_form(file, &ty, &self.locals, span.start)?;
            if !matches!(form, TypeForm::Array | TypeForm::Slice | TypeForm::Map)
                || key && !matches!(form, TypeForm::Map)
            {
                return None;
            }
            ty = workspace.range_type(file, &ty, &self.locals, span.start, usize::from(!key), 2)?;
        }
        Some((ty, span.start))
    }

    /// Preserve the written container origin for declaration-identity queries.
    pub(crate) fn literal_element_projection(
        &self,
        mut context: usize,
    ) -> Option<(Range<usize>, Vec<bool>)> {
        let mut projections = Vec::new();
        let span = loop {
            if projections.len() > 64 {
                return None;
            }
            match self.literal_types.get(context)? {
                LiteralType::Explicit(span) => break span,
                LiteralType::Element { container, key } => {
                    projections.push(*key);
                    context = *container;
                }
            }
        };
        if span.len() > 16 * 1024 {
            return None;
        }
        Some((span.clone(), projections))
    }
}

fn elided_children<'a>(
    expressions: &'a [Expr],
    elements: &'a [ExprId],
) -> impl Iterator<Item = (ExprId, bool)> + 'a {
    elements
        .iter()
        .flat_map(|element| {
            if let ExprKind::KeyValue { key, value } = expressions[element.0].kind {
                [Some((key, true)), Some((value, false))]
            } else {
                [None, Some((*element, false))]
            }
        })
        .flatten()
        .filter(|(id, _)| {
            matches!(
                expressions[id.0].kind,
                ExprKind::Composite {
                    literal_type: None,
                    ..
                }
            )
        })
}
