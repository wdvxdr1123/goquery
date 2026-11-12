//! Bounded, query-local structural types. No closed source is retained.
use super::{BasicKind, Field, MAX_BYTES, MAX_NODES, Method};

use crate::index::IndexedSymbol as Symbol;
use crate::workspace::Workspace;
use std::ops::Range;
use std::path::Path;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct TypeId(pub(super) u16);

/// A declaration identity, independent of the importing file's spelling.
#[derive(Clone, Copy)]
pub(super) struct DeclRef<'a> {
    pub(super) file: &'a Path,
    pub(super) symbol: &'a Symbol,
}

#[derive(Clone)]
pub(super) enum Ty<'a> {
    Basic(BasicKind),
    Error(TypeId),
    /// A formal's identity is its TypeId; the index is only for schema substitution.
    Parameter(usize),
    /// Declaration-only local identity: no underlying/checking facts are invented.
    LocalDeclaration {
        origin: &'a Path,
        selection: Range<usize>,
        formal: bool,
    },
    Interface {
        origin: &'a Path,
        methods: Range<usize>,
        builtin_any: bool,
    },
    Signature {
        parameters: Range<usize>,
        results: Range<usize>,
        variadic: bool,
    },
    Struct {
        origin: &'a Path,
        fields: Range<usize>,
    },
    Named {
        declaration: DeclRef<'a>,
        arguments: Range<usize>,
    },
    Pointer(TypeId),
    Slice(TypeId),
    Array {
        length: u64,
        element: TypeId,
    },
    Map {
        key: TypeId,
        value: TypeId,
    },
    Chan {
        direction: Direction,
        element: TypeId,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Direction {
    Both,
    Send,
    Receive,
}

// Keep the 64-bit per-node ceiling unchanged when adding projection metadata.
#[cfg(target_pointer_width = "64")]
const _: () = assert!(std::mem::size_of::<Ty<'static>>() <= 48);

#[derive(Clone, Copy)]
pub(super) enum DisplayScope<'p> {
    Consumer(&'p Path),
    Qualified,
}

/// Nodes and argument lists live only as long as the current type operation.
/// Unsupported/invalid/budget-limited inputs return None, never an `any` type.
pub(crate) struct TypeArena<'a> {
    pub(super) workspace: &'a Workspace,
    pub(super) nodes: Vec<Ty<'a>>,
    pub(super) arguments: Vec<TypeId>,
    pub(super) expansions: Vec<(TypeId, TypeId)>,
    pub(super) named_checks: Vec<(TypeId, bool)>,
    pub(super) named_check_depth: usize,
    pub(super) relation_steps: usize,
    pub(super) expansion_depth: usize,
    pub(super) constraint_depth: usize,
    pub(super) constraint_terms: usize,
    // Facts belong to fresh formal nodes, not the substitution indices reused by owners.
    pub(super) parameter_constraints: Vec<(TypeId, super::constraints::Constraint)>,
    pub(super) local_parameters: Vec<super::parameters::LocalParameter<'a>>,
    pub(super) local_types: Vec<super::declaration::LocalTypeFact<'a>>,
    pub(super) local_type_bytes: usize,
    pub(super) value_facts: Vec<super::values::ValueFact<'a>>,
    local_values: Vec<(&'a Path, usize, Option<Option<super::Operand>>)>,
    pub(super) fields: Vec<Field>,
    pub(super) field_bytes: usize,
    pub(super) methods: Vec<Method<'a>>,
    pub(super) method_bytes: usize,
    pub(super) invalid_type: bool,
}

impl<'a> TypeArena<'a> {
    pub(crate) fn has_type_error(&self) -> bool {
        self.invalid_type
    }
    pub(crate) fn basic(&mut self, kind: BasicKind) -> Option<TypeId> {
        self.push(Ty::Basic(kind))
    }
    pub(crate) fn basic_kind(&mut self, id: TypeId) -> Option<BasicKind> {
        let id = self.underlying(id)?;
        match self.nodes.get(usize::from(id.0))? {
            Ty::Basic(kind) => Some(*kind),
            _ => None,
        }
    }
    pub(crate) fn new(workspace: &'a Workspace) -> Self {
        Self {
            workspace,
            nodes: Vec::new(),
            arguments: Vec::new(),
            expansions: Vec::new(),
            named_checks: Vec::new(),
            named_check_depth: 0,
            relation_steps: crate::analysis::query::budget::RELATION_STEPS,
            expansion_depth: 0,
            constraint_depth: 0,
            constraint_terms: 0,
            parameter_constraints: Vec::new(),
            local_parameters: Vec::new(),
            local_types: Vec::new(),
            local_type_bytes: 0,
            value_facts: Vec::new(),
            local_values: Vec::new(),
            fields: Vec::new(),
            field_bytes: 0,
            methods: Vec::new(),
            method_bytes: 0,
            invalid_type: false,
        }
    }

    /// Lazily check each lexical variable once per query. An in-progress entry
    /// rejects cycles; canonical declaration positions distinguish shadowing.
    pub(crate) fn local_value(
        &mut self,
        origin: &Path,
        selection: usize,
        check: impl FnOnce(&mut Self) -> Option<Option<super::Operand>>,
    ) -> Option<Option<super::Operand>> {
        let origin = self.workspace.files.get_key_value(origin)?.0.as_path();
        if let Some((_, _, value)) = self
            .local_values
            .iter()
            .find(|(file, at, _)| *file == origin && *at == selection)
        {
            return *value;
        }
        if self.local_values.len() >= super::MAX_NODES {
            return Some(None);
        }
        let index = self.local_values.len();
        self.local_values.push((origin, selection, Some(None)));
        let value = check(self);
        self.local_values[index].2 = value;
        value
    }

    pub(super) fn push(&mut self, ty: Ty<'a>) -> Option<TypeId> {
        if self.nodes.len() >= MAX_NODES {
            return None;
        }
        let id = TypeId(self.nodes.len() as u16);
        self.nodes.push(ty);
        Some(id)
    }

    pub(crate) fn pointer(&mut self, element: TypeId) -> Option<TypeId> {
        self.push(Ty::Pointer(element))
    }

    pub(crate) fn render(&self, id: TypeId, consumer: &Path) -> Option<String> {
        let mut output = String::new();
        self.write(id, consumer, &mut output, 0)?;
        Some(output)
    }

    /// The legacy text boundary must preserve identity under consumer shadowing.
    pub(crate) fn render_resolved(&mut self, id: TypeId, consumer: &Path) -> Option<String> {
        let text = self.render(id, consumer)?;
        let projected = self.parse(consumer, &text)?;
        self.identical(id, projected)?.then_some(text)
    }

    pub(super) fn write(
        &self,
        id: TypeId,
        consumer: &Path,
        out: &mut String,
        depth: usize,
    ) -> Option<()> {
        self.write_in(id, DisplayScope::Consumer(consumer), out, depth)
    }
    pub(crate) fn diagnostic_type(&self, id: TypeId) -> Option<String> {
        let mut output = String::new();
        self.write_in(id, DisplayScope::Qualified, &mut output, 0)?;
        Some(output)
    }
    pub(super) fn write_in(
        &self,
        id: TypeId,
        consumer: DisplayScope<'_>,
        out: &mut String,
        depth: usize,
    ) -> Option<()> {
        if depth >= super::MAX_DEPTH {
            return None;
        }
        match self.nodes.get(usize::from(id.0))? {
            Ty::Parameter(_) => {
                let (origin, _, name) = self.local_parameter(id)?;
                if matches!(consumer, DisplayScope::Consumer(file) if file != origin) {
                    return None;
                }
                out.push_str(name);
            }
            Ty::LocalDeclaration {
                origin,
                selection,
                formal,
            } => {
                match consumer {
                    // A local declaration renders only for consumers in its own file;
                    // no other scope can prove the same spelling without a shadow risk.
                    DisplayScope::Consumer(file) if file == *origin && !*formal => {
                        out.push_str(&self.local_type_name(origin, selection)?);
                    }
                    DisplayScope::Qualified => {
                        if !formal {
                            out.push_str(&self.workspace.import_path(origin)?);
                            out.push('.');
                        }
                        out.push_str(&self.workspace.source_slice(origin, selection.clone())?);
                    }
                    _ => return None,
                }
            }
            Ty::Error(_) => out.push_str("error"),
            Ty::Interface {
                origin,
                methods,
                builtin_any,
            } => {
                self.write_interface(origin, methods.clone(), *builtin_any, consumer, out, depth)?
            }
            Ty::Signature {
                parameters,
                results,
                variadic,
            } => self.write_signature(
                parameters.clone(),
                results.clone(),
                *variadic,
                consumer,
                out,
                depth,
            )?,
            Ty::Struct { origin, fields } => {
                self.write_struct(origin, fields.clone(), consumer, out, depth)?
            }
            Ty::Basic(kind) => out.push_str(kind.name()),
            Ty::Named {
                declaration,
                arguments,
            } => {
                match consumer {
                    DisplayScope::Consumer(file) => out.push_str(&self.workspace.result_type(
                        file,
                        declaration.file,
                        &declaration.symbol.name,
                    )?),
                    DisplayScope::Qualified => {
                        out.push_str(&self.workspace.import_path(declaration.file)?);
                        out.push('.');
                        out.push_str(&declaration.symbol.name);
                    }
                }
                if !arguments.is_empty() {
                    out.push('[');
                    for (index, argument) in self.arguments[arguments.clone()].iter().enumerate() {
                        if index != 0 {
                            out.push_str(", ");
                        }
                        self.write_in(*argument, consumer, out, depth + 1)?;
                    }
                    out.push(']');
                }
            }
            Ty::Pointer(element) | Ty::Slice(element) => {
                out.push_str(if matches!(self.nodes[usize::from(id.0)], Ty::Pointer(_)) {
                    "*"
                } else {
                    "[]"
                });
                self.write_in(*element, consumer, out, depth + 1)?;
            }
            Ty::Array { length, element } => {
                out.push('[');
                out.push_str(&length.to_string());
                out.push(']');
                self.write_in(*element, consumer, out, depth + 1)?;
            }
            Ty::Map { key, value } => {
                out.push_str("map[");
                self.write_in(*key, consumer, out, depth + 1)?;
                out.push(']');
                self.write_in(*value, consumer, out, depth + 1)?;
            }
            Ty::Chan { direction, element } => {
                out.push_str(match direction {
                    Direction::Both => "chan ",
                    Direction::Send => "chan<- ",
                    Direction::Receive => "<-chan ",
                });
                self.write_in(*element, consumer, out, depth + 1)?;
            }
        }
        (out.len() <= MAX_BYTES).then_some(())
    }
}
