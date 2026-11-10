//! Query-local lexical bindings. No source or syntax nodes survive the query.
mod body;
mod cursor;
mod kind;
pub(crate) mod named;
mod resolver;
pub(crate) mod signature;
pub(crate) mod universe;
pub(crate) use cursor::CursorScope;
pub(crate) use kind::ObjectKind;
pub(crate) use named::{
    arguments as type_arguments, named_root, named_type, parameters as type_parameters,
};
pub(crate) use resolver::{Identity, Resolver};
pub(crate) use signature::receiver_parameters;

use std::ops::Range;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeclarationKind {
    Value,
    Constant,
    Type,
    TypeParameter,
    Variadic,
}

pub struct Binding {
    pub name: String,
    pub selection: Range<usize>,
    scope: Range<usize>,
    visible_from: usize,
    pub object_kind: ObjectKind,
    pub(crate) declaration: DeclarationKind,
    pub declared_type: Option<String>,
    annotation: Option<Box<Range<usize>>>,
    initializer: Option<Box<Initializer>>,
}

struct Initializer {
    span: Range<usize>,
    result: usize,
    count: usize,
    kind: InitializerKind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum InitializerKind {
    Value,
    Range,
}

impl Binding {
    pub(crate) fn annotation(&self) -> Option<&Range<usize>> {
        self.annotation.as_deref()
    }

    pub(crate) fn initializer(&self) -> Option<(&Range<usize>, usize, usize)> {
        self.initializer
            .as_ref()
            .filter(|value| value.kind == InitializerKind::Value)
            .map(|value| (&value.span, value.result, value.count))
    }
    pub(crate) fn range_initializer(&self) -> Option<(&Range<usize>, usize, usize)> {
        self.initializer
            .as_ref()
            .filter(|value| value.kind == InitializerKind::Range)
            .map(|value| (&value.span, value.result, value.count))
    }
    pub(crate) fn is_inner_than(&self, other: &Self) -> bool {
        (self.scope.len(), std::cmp::Reverse(self.visible_from))
            < (other.scope.len(), std::cmp::Reverse(other.visible_from))
    }

    pub(crate) fn scope_start(&self) -> usize {
        self.scope.start
    }
}

/// Sparse source-only facts for implicit case variables. Query-local and capped
/// at 128 clauses / 256 written types, independent of ordinary binding count.
pub(crate) struct SwitchCase {
    selection: usize,
    scope_start: usize,
    pub(crate) operand: Range<usize>,
    pub(crate) types: Vec<Range<usize>>,
    pub(crate) clauses: usize,
    pub(crate) default: bool,
}

// Vec capacities stop at these power-of-two caps. No source strings or trees
// enter this metadata budget; type-name declarations have a separate 256 cap.
const _: () = assert!(128 * size_of::<SwitchCase>() + 256 * size_of::<Range<usize>>() <= 16 * 1024);

pub struct Locals {
    bindings: Vec<Binding>,
    type_constraints: Vec<(Range<usize>, Range<usize>)>,
    receiver_constraints: Vec<(Range<usize>, Range<usize>)>,
    switch_cases: Vec<SwitchCase>,
}

impl Locals {
    pub(crate) fn empty() -> Self {
        Self {
            bindings: Vec::new(),
            type_constraints: Vec::new(),
            receiver_constraints: Vec::new(),
            switch_cases: Vec::new(),
        }
    }

    pub(crate) fn extend(&mut self, other: Self) {
        for case in other.switch_cases {
            if self.switch_cases.len() < 128
                && self
                    .switch_cases
                    .iter()
                    .map(|case| case.types.len())
                    .sum::<usize>()
                    + case.types.len()
                    <= 256
            {
                self.switch_cases.push(case);
            }
        }
        self.type_constraints.extend(other.type_constraints);
        self.receiver_constraints.extend(other.receiver_constraints);
        for mut binding in other.bindings {
            if binding.declared_type.as_ref().is_some_and(|ty| {
                self.resolve_at(
                    binding.selection.start,
                    ty.split('.').next().expect("named type"),
                )
                .is_some()
            }) {
                binding.declared_type = None;
            }
            self.bindings.push(binding);
        }
    }

    pub(crate) fn switch_case(&self, binding: &Binding) -> Option<&SwitchCase> {
        self.switch_cases.iter().find(|case| {
            case.selection == binding.selection.start && case.scope_start == binding.scope.start
        })
    }

    pub(crate) fn switch_clauses(
        &self,
        selected: &SwitchCase,
    ) -> impl Iterator<Item = &SwitchCase> {
        let operand = selected.operand.clone();
        self.switch_cases
            .iter()
            .filter(move |case| case.operand == operand)
    }

    pub(crate) fn type_parameter_receiver(&self, binding: &Binding) -> Option<&Range<usize>> {
        self.receiver_constraints
            .iter()
            .find(|(selection, _)| *selection == binding.selection)
            .map(|(_, receiver)| receiver)
    }

    pub(crate) fn type_parameter_constraint(&self, binding: &Binding) -> Option<&Range<usize>> {
        self.type_constraints
            .iter()
            .find(|(selection, _)| *selection == binding.selection)
            .map(|(_, bound)| bound)
    }

    pub(crate) fn type_parameter(
        &mut self,
        source: &str,
        selection: Range<usize>,
        scope: Range<usize>,
    ) {
        self.bindings.push(Binding {
            name: source[selection.clone()].to_owned(),
            selection,
            visible_from: scope.start,
            scope,
            object_kind: ObjectKind::TypeParameter,
            declaration: DeclarationKind::TypeParameter,
            declared_type: None,
            annotation: None,
            initializer: None,
        });
    }

    pub(crate) fn definition_at(&self, selection: &Range<usize>, name: &str) -> Option<&Binding> {
        self.bindings
            .iter()
            .find(|binding| binding.name != "_" && binding.selection == *selection)
            .or_else(|| self.resolve_at(selection.start, name))
    }

    pub(crate) fn range_blank_at(&self, selection: &Range<usize>) -> Option<&Binding> {
        self.bindings
            .iter()
            .find(|binding| binding.name == "_" && binding.selection == *selection)
    }

    pub(crate) fn resolve_at(&self, offset: usize, name: &str) -> Option<&Binding> {
        self.bindings
            .iter()
            .filter(|binding| {
                binding.name != "_"
                    && binding.name == name
                    && binding.scope.contains(&offset)
                    && binding.visible_from <= offset
            })
            .min_by_key(|binding| (binding.scope.len(), std::cmp::Reverse(binding.visible_from)))
    }

    pub(crate) fn visible_bindings(&self, offset: usize) -> impl Iterator<Item = &Binding> {
        self.bindings.iter().filter(move |binding| {
            binding.name != "_" && binding.scope.contains(&offset) && binding.visible_from <= offset
        })
    }

    pub(crate) fn has_outer_binding(&self, binding: &Binding) -> bool {
        self.bindings.iter().any(|outer| {
            outer.name == binding.name
                && outer.selection != binding.selection
                && outer.scope != binding.scope
                && outer.scope.start <= binding.scope.start
                && outer.scope.end >= binding.scope.end
                && outer.scope.contains(&binding.selection.start)
                && outer.visible_from <= binding.selection.start
        })
    }
}
