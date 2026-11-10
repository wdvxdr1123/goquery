//! Semantic navigation targets; protocol locations/items are feature-owned.
pub(crate) mod anonymous;
mod members;
mod types;
use crate::analysis::check::{Inference, Selection};
use crate::analysis::member::MemberDeclaration;
use crate::analysis::scope::CursorScope;
use crate::index::IndexedSymbol as Symbol;
use crate::syntax::ExprKind;
use crate::workspace::Workspace;
use std::ops::Range;
use std::path::{Path, PathBuf};

pub(crate) enum Target<'a> {
    Opaque,
    Unresolved,
    Local(Range<usize>),
    Global(&'a PathBuf, &'a Symbol),
    Member(MemberDeclaration<'a>),
}

impl Workspace {
    pub(crate) fn expression_target<'a>(
        &'a self,
        file: &Path,
        source: &str,
        selection: &Range<usize>,
        scope: &CursorScope,
    ) -> Target<'a> {
        let name = &source[selection.clone()];
        if let Some(binding) = scope
            .locals
            .definition_at(selection, name)
            .or_else(|| scope.locals.range_blank_at(selection))
            .filter(|binding| binding.selection == *selection)
        {
            return Target::Local(binding.selection.clone());
        }
        let Some(body) = &scope.body else {
            return Target::Opaque;
        };
        if let Some(target) = self.type_reference_target(
            file,
            source,
            selection,
            &body.tree.type_references,
            body.base,
            &scope.locals,
        ) {
            return target;
        }
        let expressions = &body.tree.expressions;
        for (index, expression) in expressions.iter().enumerate() {
            let span = body.base + expression.span.start..body.base + expression.span.end;
            let field = match &expression.kind {
                ExprKind::Selector { field, .. } => {
                    Some(body.base + field.start..body.base + field.end)
                }
                _ => None,
            };
            if field.as_ref() == Some(selection)
                || span == *selection && matches!(expression.kind, ExprKind::Name)
            {
                if field.is_none() && expressions.iter().any(|parent| matches!(parent.kind, ExprKind::KeyValue { key, .. } if key.0 == index)) {
                    return Inference::new(self, file, source, &scope.locals, expressions, body.base)
                        .literal_field(crate::syntax::ExprId(index)).map_or(Target::Opaque, Target::Member);
                }
                if field.is_none()
                    && let Some(binding) = scope.locals.definition_at(selection, name)
                {
                    return Target::Local(binding.selection.clone());
                }
                return Inference::new(self, file, source, &scope.locals, expressions, body.base)
                    .selection(crate::syntax::ExprId(index))
                    .map_or(Target::Unresolved, |selection| match selection {
                        Selection::Global(path, symbol) => Target::Global(path, symbol),
                        Selection::Member(member) => Target::Member(member),
                    });
            }
        }
        Target::Opaque
    }
}
