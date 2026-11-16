//! Bounded, query-local structural types. No closed source is retained.
use super::{BasicKind, Field, MAX_NODES, Method, Qualifier};

use crate::analysis::budget::MAX_SHARED_ARENA_NODES;
use crate::analysis::obj::DeclId;
use crate::workspace::Workspace;
use std::collections::HashMap;
use std::ops::Range;
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct TypeId(pub(super) u16);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(super) struct SliceRef {
    pub(super) start: u32,
    pub(super) len: u16,
}

impl SliceRef {
    pub(super) fn from_range(range: Range<usize>) -> Option<Self> {
        Some(Self {
            start: u32::try_from(range.start).ok()?,
            len: u16::try_from(range.len()).ok()?,
        })
    }

    pub(super) fn range(self) -> Range<usize> {
        let start = self.start as usize;
        start..start + usize::from(self.len)
    }

    pub(super) const fn len(self) -> usize {
        self.len as usize
    }

    pub(super) const fn is_empty(self) -> bool {
        self.len == 0
    }
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
        declaration: DeclId<'a>,
        arguments: SliceRef,
    },
    Alias {
        declaration: DeclId<'a>,
        arguments: SliceRef,
    },
    Tuple(SliceRef),
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum Direction {
    Both,
    Send,
    Receive,
}

// Keep the 64-bit per-node ceiling unchanged when adding projection metadata.
#[cfg(target_pointer_width = "64")]
const _: () = assert!(std::mem::size_of::<Ty<'static>>() <= 48);

#[derive(Clone, PartialEq, Eq, Hash)]
enum InternArgs {
    Inline { len: u8, items: [TypeId; 2] },
    Heap(Box<[TypeId]>),
}

impl InternArgs {
    fn from_slice(slice: &[TypeId]) -> Self {
        match slice {
            [] => Self::Inline {
                len: 0,
                items: [TypeId(0), TypeId(0)],
            },
            [a] => Self::Inline {
                len: 1,
                items: [*a, TypeId(0)],
            },
            [a, b] => Self::Inline {
                len: 2,
                items: [*a, *b],
            },
            many => Self::Heap(many.into()),
        }
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
enum InternKey<'a> {
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
    Named {
        declaration: DeclId<'a>,
        arguments: InternArgs,
    },
    Alias {
        declaration: DeclId<'a>,
        arguments: InternArgs,
    },
    Tuple(InternArgs),
}

/// Snapshot of arena state for speculative checks (`Checker`/`Semantics`).
#[derive(Clone, Copy)]
pub(crate) struct ArenaWatermark {
    nodes: usize,
    arguments: usize,
    expansions: usize,
    named_checks: usize,
    constraint_terms: usize,
    parameter_constraints: usize,
    local_parameters: usize,
    local_types: usize,
    local_type_bytes: usize,
    fields: usize,
    field_bytes: usize,
    methods: usize,
    method_bytes: usize,
    invalid_type: bool,
}

/// Nodes and argument lists live only as long as the current type operation.
/// Unsupported/invalid/budget-limited inputs return None, never an `any` type.
pub(crate) struct TypeArena<'a> {
    pub(super) workspace: &'a Workspace,
    pub(super) node_limit: usize,
    pub(super) byte_limit: usize,
    basics: [u16; BasicKind::COUNT],
    interner: HashMap<InternKey<'a>, TypeId>,
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
    pub(super) fields: Vec<Field>,
    pub(super) field_bytes: usize,
    pub(super) methods: Vec<Method<'a>>,
    pub(super) method_bytes: usize,
    pub(super) pkg_methods:
        HashMap<(&'a Path, &'a str, bool), Option<super::methods::PkgMethodIndex<'a>>>,
    pub(super) concrete_methods: HashMap<DeclId<'a>, super::methods::ConcreteMethodMemo>,
    pub(super) invalid_type: bool,
    pub(super) symbolic: bool,
}

impl<'a> TypeArena<'a> {
    pub(crate) fn has_type_error(&self) -> bool {
        self.invalid_type
    }

    pub(crate) fn basic(&mut self, kind: BasicKind) -> Option<TypeId> {
        let cached = self.basics[kind.index()];
        if cached != u16::MAX {
            return Some(TypeId(cached));
        }
        let id = self.push_raw(Ty::Basic(kind))?;
        self.basics[kind.index()] = id.0;
        Some(id)
    }

    pub(crate) fn basic_kind(&mut self, id: TypeId) -> Option<BasicKind> {
        let id = self.underlying(id)?;
        match self.nodes.get(usize::from(id.0))? {
            Ty::Basic(kind) => Some(*kind),
            _ => None,
        }
    }

    pub(crate) fn new(workspace: &'a Workspace) -> Self {
        Self::with_limit(workspace, MAX_NODES, super::MAX_BYTES)
    }

    pub(crate) fn new_symbolic(workspace: &'a Workspace) -> Self {
        Self {
            symbolic: true,
            ..Self::new(workspace)
        }
    }

    pub(crate) fn workspace(&self) -> &'a Workspace {
        self.workspace
    }

    pub(crate) fn node_count(&self) -> u16 {
        u16::try_from(self.nodes.len()).unwrap_or(u16::MAX)
    }

    pub(crate) fn reset_steps(&mut self) {
        self.relation_steps = crate::analysis::budget::RELATION_STEPS;
        self.invalid_type = false;
    }

    pub(crate) fn needs_rewind(&self) -> bool {
        self.nodes.len() > self.node_limit / 2
            || self.constraint_terms > self.node_limit / 2
            || self.parameter_constraints.len() > self.node_limit / 2
            || self.local_parameters.len() >= (self.node_limit / 16).max(16)
            || self.local_types.len() >= (self.node_limit / 8).max(32)
            || self.local_type_bytes > super::MAX_BYTES / 2
            || self.field_bytes > self.byte_limit / 2
            || self.method_bytes > self.byte_limit / 2
    }

    pub(crate) fn with_shared_budget(workspace: &'a Workspace) -> Self {
        Self::with_limit(workspace, MAX_SHARED_ARENA_NODES, super::MAX_BYTES * 4)
    }

    fn with_limit(workspace: &'a Workspace, node_limit: usize, byte_limit: usize) -> Self {
        Self {
            workspace,
            node_limit,
            byte_limit,
            basics: [u16::MAX; BasicKind::COUNT],
            interner: HashMap::new(),
            nodes: Vec::new(),
            arguments: Vec::new(),
            expansions: Vec::new(),
            named_checks: Vec::new(),
            named_check_depth: 0,
            relation_steps: crate::analysis::budget::RELATION_STEPS,
            expansion_depth: 0,
            constraint_depth: 0,
            constraint_terms: 0,
            parameter_constraints: Vec::new(),
            local_parameters: Vec::new(),
            local_types: Vec::new(),
            local_type_bytes: 0,
            fields: Vec::new(),
            field_bytes: 0,
            methods: Vec::new(),
            method_bytes: 0,
            pkg_methods: HashMap::new(),
            concrete_methods: HashMap::new(),
            invalid_type: false,
            symbolic: false,
        }
    }

    pub(crate) fn mark(&self) -> ArenaWatermark {
        ArenaWatermark {
            nodes: self.nodes.len(),
            arguments: self.arguments.len(),
            expansions: self.expansions.len(),
            named_checks: self.named_checks.len(),
            constraint_terms: self.constraint_terms,
            parameter_constraints: self.parameter_constraints.len(),
            local_parameters: self.local_parameters.len(),
            local_types: self.local_types.len(),
            local_type_bytes: self.local_type_bytes,
            fields: self.fields.len(),
            field_bytes: self.field_bytes,
            methods: self.methods.len(),
            method_bytes: self.method_bytes,
            invalid_type: self.invalid_type,
        }
    }

    pub(crate) fn rewind(&mut self, mark: ArenaWatermark) {
        self.nodes.truncate(mark.nodes);
        self.arguments.truncate(mark.arguments);
        self.expansions.truncate(mark.expansions);
        self.named_checks.truncate(mark.named_checks);
        self.constraint_terms = mark.constraint_terms;
        self.parameter_constraints
            .truncate(mark.parameter_constraints);
        self.local_parameters.truncate(mark.local_parameters);
        self.local_types.truncate(mark.local_types);
        self.local_type_bytes = mark.local_type_bytes;
        self.fields.truncate(mark.fields);
        self.field_bytes = mark.field_bytes;
        self.methods.truncate(mark.methods);
        self.method_bytes = mark.method_bytes;
        self.invalid_type = mark.invalid_type;
        self.relation_steps = crate::analysis::budget::RELATION_STEPS;
        for slot in &mut self.basics {
            if *slot != u16::MAX && usize::from(*slot) >= mark.nodes {
                *slot = u16::MAX;
            }
        }
        self.interner.retain(|_, id| usize::from(id.0) < mark.nodes);
        self.concrete_methods
            .retain(|_, memo| memo.arena_len <= mark.nodes);
    }

    fn push_raw(&mut self, ty: Ty<'a>) -> Option<TypeId> {
        if self.nodes.len() >= self.node_limit {
            return None;
        }
        let id = TypeId(u16::try_from(self.nodes.len()).ok()?);
        self.nodes.push(ty);
        Some(id)
    }

    pub(super) fn push(&mut self, ty: Ty<'a>) -> Option<TypeId> {
        if let Ty::Basic(kind) = ty {
            return self.basic(kind);
        }
        let key = match &ty {
            Ty::Pointer(element) => Some(InternKey::Pointer(*element)),
            Ty::Slice(element) => Some(InternKey::Slice(*element)),
            Ty::Array { length, element } => Some(InternKey::Array {
                length: *length,
                element: *element,
            }),
            Ty::Map { key, value } => Some(InternKey::Map {
                key: *key,
                value: *value,
            }),
            Ty::Chan { direction, element } => Some(InternKey::Chan {
                direction: *direction,
                element: *element,
            }),
            Ty::Named {
                declaration,
                arguments,
            } => Some(InternKey::Named {
                declaration: *declaration,
                arguments: InternArgs::from_slice(&self.arguments[arguments.range()]),
            }),
            Ty::Alias {
                declaration,
                arguments,
            } => Some(InternKey::Alias {
                declaration: *declaration,
                arguments: InternArgs::from_slice(&self.arguments[arguments.range()]),
            }),
            Ty::Tuple(elements) => Some(InternKey::Tuple(InternArgs::from_slice(
                &self.arguments[elements.range()],
            ))),
            Ty::Basic(_)
            | Ty::Error(_)
            | Ty::Parameter(_)
            | Ty::LocalDeclaration { .. }
            | Ty::Interface { .. }
            | Ty::Signature { .. }
            | Ty::Struct { .. } => None,
        };
        if let Some(key) = key {
            if let Some(&id) = self.interner.get(&key) {
                match &ty {
                    Ty::Named { arguments, .. }
                    | Ty::Alias { arguments, .. }
                    | Ty::Tuple(arguments)
                        if arguments.range().end == self.arguments.len()
                            && !arguments.is_empty() =>
                    {
                        self.arguments.truncate(arguments.start as usize);
                    }
                    _ => {}
                }
                return Some(id);
            }
            let id = self.push_raw(ty)?;
            self.interner.insert(key, id);
            return Some(id);
        }
        self.push_raw(ty)
    }

    pub(crate) fn push_named(
        &mut self,
        declaration: DeclId<'a>,
        arguments: &[TypeId],
        alias: bool,
    ) -> Option<TypeId> {
        let intern_args = InternArgs::from_slice(arguments);
        let key = if alias {
            InternKey::Alias {
                declaration,
                arguments: intern_args,
            }
        } else {
            InternKey::Named {
                declaration,
                arguments: intern_args,
            }
        };
        if let Some(&id) = self.interner.get(&key) {
            return Some(id);
        }
        if self.arguments.len() + arguments.len() > self.node_limit {
            return None;
        }
        let start = self.arguments.len();
        self.arguments.extend_from_slice(arguments);
        let slice = SliceRef::from_range(start..self.arguments.len())?;
        let ty = if alias {
            Ty::Alias {
                declaration,
                arguments: slice,
            }
        } else {
            Ty::Named {
                declaration,
                arguments: slice,
            }
        };
        let Some(id) = self.push_raw(ty) else {
            self.arguments.truncate(start);
            return None;
        };
        self.interner.insert(key, id);
        Some(id)
    }

    pub(crate) fn tuple(&mut self, elements: &[TypeId]) -> Option<TypeId> {
        let key = InternKey::Tuple(InternArgs::from_slice(elements));
        if let Some(&id) = self.interner.get(&key) {
            return Some(id);
        }
        if self.arguments.len() + elements.len() > self.node_limit {
            return None;
        }
        let start = self.arguments.len();
        self.arguments.extend_from_slice(elements);
        let slice = SliceRef::from_range(start..self.arguments.len())?;
        let Some(id) = self.push_raw(Ty::Tuple(slice)) else {
            self.arguments.truncate(start);
            return None;
        };
        self.interner.insert(key, id);
        Some(id)
    }

    pub(crate) fn pointer(&mut self, element: TypeId) -> Option<TypeId> {
        self.push(Ty::Pointer(element))
    }

    pub(crate) fn slice_type(&mut self, element: TypeId) -> Option<TypeId> {
        self.push(Ty::Slice(element))
    }

    pub(crate) fn array(&mut self, element: TypeId, length: u64) -> Option<TypeId> {
        let maximum = (1_u64 << (self.workspace.integer_bits()? - 1)) - 1;
        (length <= maximum).then_some(())?;
        self.push(Ty::Array { length, element })
    }

    pub(crate) fn array_length(&mut self, id: TypeId) -> Option<u64> {
        let root = self.underlying(id)?;
        match self.nodes.get(usize::from(root.0))? {
            Ty::Array { length, .. } => Some(*length),
            _ => None,
        }
    }

    pub(crate) fn render(&self, id: TypeId, consumer: &Path) -> Option<String> {
        self.format_type(id, Qualifier::RelativeTo(consumer))
    }

    /// Labels may mention a known package without adding it to the file imports.
    /// This text must not be parsed as a semantic fact in the consumer's scope.
    pub(crate) fn render_presentation(&self, id: TypeId, consumer: &Path) -> Option<String> {
        self.format_type(id, Qualifier::Presentation(consumer))
    }

    pub(crate) fn diagnostic_type(&self, id: TypeId) -> Option<String> {
        self.format_type(id, Qualifier::FullPath)
    }
}
