//! Unified lazy interface `TypeSet` and `TermList` (aligned with `go/types`).
mod implication;
pub(super) use super::ty::Term;
use super::{CheckOutcome, Direction, MAX_DEPTH, Ty, TypeArena, TypeId, UnknownReason};

pub(super) const MAX_TERMS: usize = 100;

/// Normalized type-term list matching `go/types.termlist`:
/// universal (`All`), empty (`Empty`), or a non-empty disjoint list (`Normal`).
#[derive(Clone)]
pub(super) enum TermList {
    All,
    Empty,
    Normal(Box<[Term]>),
}

impl TermList {
    pub(super) fn from_vec(terms: Vec<Term>) -> Self {
        if terms.is_empty() {
            Self::Empty
        } else {
            Self::Normal(terms.into_boxed_slice())
        }
    }

    pub(super) fn is_all(&self) -> bool {
        matches!(self, Self::All)
    }

    pub(super) fn is_empty(&self) -> bool {
        matches!(self, Self::Empty)
    }

    /// Returns `None` for the universal set (`All`), `Some(&[])` for `Empty`,
    /// or `Some(slice)` for a finite normalized term list.
    pub(super) fn as_slice(&self) -> Option<&[Term]> {
        match self {
            Self::All => None,
            Self::Empty => Some(&[]),
            Self::Normal(terms) => Some(terms),
        }
    }
}

/// Lazily computed and cached interface type set matching `go/types._TypeSet`.
#[derive(Clone)]
pub(super) struct TypeSet {
    pub(super) terms: TermList,
    /// Invariant: `!comparable || terms.is_all()`.
    pub(super) comparable: bool,
    /// Pure method-only interface `TypeId` holding the complete sorted method set.
    pub(super) methods: Option<TypeId>,
}

impl TypeSet {
    pub(super) fn all() -> Self {
        Self {
            terms: TermList::All,
            comparable: false,
            methods: None,
        }
    }
}

impl TypeArena<'_> {
    fn term_intersection(&mut self, a: Term, b: Term) -> Option<Option<Term>> {
        self.step()?;
        let (x, y) = match (a.approximate, b.approximate) {
            (false, false) | (true, true) => (a.ty, b.ty),
            (true, false) => (a.ty, self.underlying(b.ty)?),
            (false, true) => (self.underlying(a.ty)?, b.ty),
        };
        Some(
            self.identical(x, y)?
                .then_some(if a.approximate { b } else { a }),
        )
    }

    fn normalize_terms(&mut self, terms: Vec<Term>) -> Option<TermList> {
        let mut result: Vec<Term> = Vec::new();
        for term in terms {
            let mut include = true;
            let mut index = 0;
            while index < result.len() {
                let previous = result[index];
                if self.term_intersection(term, previous)?.is_some() {
                    if previous.approximate || !term.approximate {
                        include = false;
                        break;
                    }
                    result.remove(index);
                } else {
                    index += 1;
                }
            }
            if include {
                result.push(term);
            }
            if result.len() > MAX_TERMS {
                return None;
            }
        }
        Some(TermList::from_vec(result))
    }

    fn intersect_sets(&mut self, mut a: TypeSet, b: TypeSet) -> Option<TypeSet> {
        a.terms = match (a.terms, b.terms) {
            (TermList::All, terms) | (terms, TermList::All) => terms,
            (TermList::Empty, _) | (_, TermList::Empty) => TermList::Empty,
            (TermList::Normal(left), TermList::Normal(right)) => {
                let mut terms = Vec::new();
                for &x in left.iter() {
                    for &y in right.iter() {
                        if let Some(term) = self.term_intersection(x, y)? {
                            terms.push(term);
                        }
                    }
                }
                self.normalize_terms(terms)?
            }
        };
        a.comparable |= b.comparable;
        if let Some(methods) = b.methods {
            a.methods = Some(if let Some(previous) = a.methods {
                let previous = self.underlying(previous)?;
                let methods = self.underlying(methods)?;
                let left_data = self.interface_data(previous)?.clone();
                let right_data = self.interface_data(methods)?.clone();
                let mut values = self.methods_slice(previous, left_data.methods).to_vec();
                values.extend(
                    self.methods_slice(methods, right_data.methods)
                        .iter()
                        .cloned(),
                );
                self.store_interface(left_data.origin, values)?
            } else {
                methods
            });
        }
        self.filter_comparable(a)
    }

    fn filter_comparable(&mut self, mut set: TypeSet) -> Option<TypeSet> {
        if set.comparable {
            match std::mem::replace(&mut set.terms, TermList::All) {
                TermList::All => {}
                TermList::Empty => {
                    set.terms = TermList::Empty;
                    set.comparable = false;
                }
                TermList::Normal(terms) => {
                    let mut kept = Vec::new();
                    for &term in terms.iter() {
                        if self.strict_comparable(term.ty, 0)? {
                            kept.push(term);
                        }
                    }
                    set.terms = TermList::from_vec(kept);
                    set.comparable = false;
                }
            }
        }
        debug_assert!(!set.comparable || set.terms.is_all());
        Some(set)
    }

    pub(super) fn strict_comparable(&mut self, input: TypeId, depth: usize) -> Option<bool> {
        self.step()?;
        if depth >= MAX_DEPTH {
            return None;
        }
        let input = self.underlying(input)?;
        match *self.node(input)? {
            Ty::Interface(_) => Some(false),
            Ty::Array { element, .. } => self.strict_comparable(element, depth + 1),
            Ty::Struct(_) => {
                let fields = self.struct_data(input)?.fields.clone();
                let field_tys = self
                    .fields_slice(input, fields)?
                    .iter()
                    .map(|f| f.ty)
                    .collect::<Vec<_>>();
                for ty in field_tys {
                    if !self.strict_comparable(ty, depth + 1)? {
                        return Some(false);
                    }
                }
                Some(true)
            }
            _ => self.comparable(input, depth + 1),
        }
    }

    pub(super) fn set_core(&mut self, set: &TypeSet) -> Option<(Term, bool)> {
        let terms = set.terms.as_slice()?;
        if terms.is_empty() {
            return None;
        }
        if terms.len() == 1 {
            return Some((terms[0], true));
        }
        let mut core = self.underlying(terms[0].ty)?;
        let mut approximate = false;
        for term in terms {
            approximate |= term.approximate;
            let ty = self.underlying(term.ty)?;
            if !self.identical(core, ty)? {
                let (
                    Ty::Chan {
                        direction: a,
                        element: x,
                    },
                    Ty::Chan {
                        direction: b,
                        element: y,
                    },
                ) = (*self.node(core)?, *self.node(ty)?)
                else {
                    return None;
                };
                if !self.identical(x, y)? {
                    return None;
                }
                let direction = if a == Direction::Both {
                    b
                } else if b == Direction::Both || a == b {
                    a
                } else {
                    return None;
                };
                core = self.push(Ty::Chan {
                    direction,
                    element: x,
                })?;
            }
        }
        Some((
            Term {
                ty: core,
                approximate,
            },
            false,
        ))
    }

    pub(super) fn prove_set(&mut self, set: &TypeSet, actual: TypeId) -> Option<()> {
        if set.comparable {
            self.comparable(actual, 0)?.then_some(())?;
        }
        if let Some(terms) = set.terms.as_slice() {
            let mut matched = false;
            for term in terms {
                let value = if term.approximate {
                    self.underlying(actual)?
                } else {
                    actual
                };
                if self.identical(value, term.ty)? {
                    matched = true;
                    break;
                }
            }
            matched.then_some(())?;
        }
        if let Some(methods) = set.methods {
            self.assignable(
                super::Operand {
                    mode: super::Mode::Value,
                    ty: actual,
                    value: None,
                },
                methods,
                self.integer_bits()?,
            )?
            .then_some(())?;
        }
        Some(())
    }

    /// Resolve a constraint/interface `TypeId` (which may be a `Ty::Interface`,
    /// a named/aliased interface, or a `Ty::Union`) to its normalized [`TypeSet`].
    pub(in crate::analysis::engine::types) fn constraint_type_set_checked(
        &mut self,
        bound: TypeId,
        depth: usize,
    ) -> CheckOutcome<TypeSet> {
        self.embedded_type_set(bound, depth)
    }

    pub(in crate::analysis::engine::types) fn constraint_type_set(
        &mut self,
        bound: TypeId,
        depth: usize,
    ) -> Option<TypeSet> {
        self.constraint_type_set_checked(bound, depth).ok()
    }

    pub(in crate::analysis::engine::types) fn interface_type_set_checked(
        &mut self,
        iface: TypeId,
        depth: usize,
    ) -> CheckOutcome<TypeSet> {
        if self.step().is_none() || depth >= MAX_DEPTH {
            return CheckOutcome::Unknown(UnknownReason::BudgetExceeded);
        }
        if let Some(cached) = self.pool_for(iface).interface_sets.get(&iface) {
            return CheckOutcome::Ok(cached.clone());
        }
        let Some(data) = self.interface_data(iface).cloned() else {
            return CheckOutcome::Unknown(UnknownReason::Unsupported);
        };
        if data.builtin_any
            && data.methods.is_empty()
            && !data.comparable
            && data.embeddeds.is_empty()
        {
            return CheckOutcome::Ok(TypeSet::all());
        }
        let mut set = TypeSet {
            terms: TermList::All,
            comparable: data.comparable,
            methods: None,
        };
        if !data.methods.is_empty() {
            if !data.has_type_elements && !data.comparable {
                set.methods = Some(iface);
            } else {
                let methods = self.methods_slice(iface, data.methods).to_vec();
                set.methods = match self.store_interface_checked(data.origin, methods) {
                    CheckOutcome::Ok(id) => Some(id),
                    CheckOutcome::Invalid(()) => return CheckOutcome::Invalid(()),
                    CheckOutcome::Unknown(reason) => return CheckOutcome::Unknown(reason),
                };
            }
        }
        let embeddeds = self.arguments_slice(iface, data.embeddeds).to_vec();
        for embedded in embeddeds {
            let embedded_set = match self.embedded_type_set(embedded, depth + 1) {
                CheckOutcome::Ok(sub) => sub,
                other => return other,
            };
            let Some(merged) = self.intersect_sets(set, embedded_set) else {
                return CheckOutcome::Unknown(UnknownReason::BudgetExceeded);
            };
            set = merged;
        }
        let Some(set) = self.filter_comparable(set) else {
            return CheckOutcome::Unknown(UnknownReason::BudgetExceeded);
        };
        let pool = self.pool_for_mut(iface);
        if pool.interface_sets.len() < pool.node_limit {
            pool.interface_sets.insert(iface, set.clone());
        }
        CheckOutcome::Ok(set)
    }

    fn embedded_type_set(&mut self, embedded: TypeId, depth: usize) -> CheckOutcome<TypeSet> {
        if self.step().is_none() || depth >= MAX_DEPTH {
            return CheckOutcome::Unknown(UnknownReason::BudgetExceeded);
        }
        match *self.node(embedded).unwrap_or(&Ty::Invalid) {
            Ty::Invalid => CheckOutcome::Unknown(UnknownReason::MissingMetadata),
            Ty::Union(slice) => {
                let terms = self.terms_slice(embedded, slice).to_vec();
                let mut union_terms = TermList::Empty;
                let mut direct: Vec<Term> = Vec::new();
                for term in terms {
                    let sub = if !term.approximate
                        && self.interface_constructor(term.ty) == Some(true)
                    {
                        match self.embedded_type_set(term.ty, depth + 1) {
                            CheckOutcome::Ok(sub) => {
                                if sub.comparable || sub.methods.is_some() {
                                    return CheckOutcome::Invalid(());
                                }
                                sub
                            }
                            other => return other,
                        }
                    } else {
                        for &prev in &direct {
                            match self.term_intersection(term, prev) {
                                Some(Some(_)) => return CheckOutcome::Invalid(()),
                                Some(None) => {}
                                None => {
                                    return CheckOutcome::Unknown(UnknownReason::BudgetExceeded);
                                }
                            }
                        }
                        direct.push(term);
                        TypeSet {
                            terms: TermList::Normal(Box::new([term])),
                            comparable: false,
                            methods: None,
                        }
                    };
                    union_terms = match (union_terms, sub.terms) {
                        (TermList::All, _) | (_, TermList::All) => TermList::All,
                        (TermList::Empty, right) => right,
                        (left, TermList::Empty) => left,
                        (TermList::Normal(left), TermList::Normal(right)) => {
                            let mut combined = Vec::with_capacity(left.len() + right.len());
                            combined.extend_from_slice(&left);
                            combined.extend_from_slice(&right);
                            match self.normalize_terms(combined) {
                                Some(norm) => norm,
                                None => {
                                    return CheckOutcome::Unknown(UnknownReason::BudgetExceeded);
                                }
                            }
                        }
                    };
                }
                CheckOutcome::Ok(TypeSet {
                    terms: union_terms,
                    comparable: false,
                    methods: None,
                })
            }
            Ty::Interface(_) => self.interface_type_set_checked(embedded, depth + 1),
            Ty::Named { .. } | Ty::Alias { .. } => {
                if self.interface_constructor(embedded) == Some(true) {
                    if let Some(iface) = self.error_interface(embedded) {
                        return self.interface_type_set_checked(iface, depth + 1);
                    }
                    if self.named_info(embedded).is_some() {
                        // Expand through the shared DeclArena origin template without
                        // re-entering check_named_arguments (which breaks self-referential
                        // constraints like `func F[T Ordered[T]]()`).
                        let Some(iface) = self.underlying_interface_unchecked(embedded) else {
                            return CheckOutcome::Unknown(UnknownReason::MissingMetadata);
                        };
                        self.interface_type_set_checked(iface, depth + 1)
                    } else if let Some(root) = self.underlying(embedded) {
                        self.interface_type_set_checked(root, depth + 1)
                    } else {
                        CheckOutcome::Unknown(UnknownReason::MissingMetadata)
                    }
                } else {
                    CheckOutcome::Ok(TypeSet {
                        terms: TermList::Normal(Box::new([Term {
                            ty: embedded,
                            approximate: false,
                        }])),
                        comparable: false,
                        methods: None,
                    })
                }
            }
            _ => CheckOutcome::Ok(TypeSet {
                terms: TermList::Normal(Box::new([Term {
                    ty: embedded,
                    approximate: false,
                }])),
                comparable: false,
                methods: None,
            }),
        }
    }
}
