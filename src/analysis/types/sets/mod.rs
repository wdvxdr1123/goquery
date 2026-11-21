//! Bounded normalized type sets; None terms is universal, an empty list is empty.
mod implication;
mod parse;
use super::constraints::Constraint;
pub(super) use super::ty::Term;
use super::{CheckOutcome, Direction, MAX_DEPTH, MAX_NODES, Ty, TypeArena, TypeId, UnknownReason};

pub(super) const MAX_TERMS: usize = 100;

#[derive(Clone)]
pub(super) struct TypeSet {
    pub(super) terms: Option<Vec<Term>>,
    pub(super) comparable: bool,
    pub(super) methods: Option<TypeId>,
    pub(super) checks: Vec<(Constraint, TypeId)>,
    // Interface operands are exempt from the syntactic union overlap rule.
    pub(super) interface: bool,
}

impl TypeSet {
    fn all() -> Self {
        Self {
            terms: None,
            comparable: false,
            methods: None,
            checks: Vec::new(),
            interface: true,
        }
    }
    pub(super) fn constraint(self) -> Constraint {
        if self.methods.is_none() && self.checks.is_empty() {
            if self.terms.is_none() {
                return if self.comparable {
                    Constraint::Comparable
                } else {
                    Constraint::Any
                };
            }
            if !self.comparable
                && let Some(terms) = &self.terms
                && terms.len() == 1
            {
                return if terms[0].approximate {
                    Constraint::Approximate(terms[0].ty)
                } else {
                    Constraint::Exact(terms[0].ty)
                };
            }
        }
        Constraint::Set(self)
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

    fn normalize_terms(&mut self, terms: Vec<Term>) -> Option<Vec<Term>> {
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
        Some(result)
    }

    fn intersect_sets(&mut self, mut a: TypeSet, b: TypeSet) -> Option<TypeSet> {
        a.terms = match (a.terms, b.terms) {
            (None, terms) | (terms, None) => terms,
            (Some(left), Some(right)) => {
                let mut terms = Vec::new();
                for x in left {
                    for &y in &right {
                        if let Some(term) = self.term_intersection(x, y)? {
                            terms.push(term);
                        }
                    }
                }
                Some(self.normalize_terms(terms)?)
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
        a.checks.extend(b.checks);
        if a.checks.len() > MAX_NODES {
            return None;
        }
        self.filter_comparable(a)
    }

    fn filter_comparable(&mut self, mut set: TypeSet) -> Option<TypeSet> {
        if set.comparable
            && let Some(terms) = set.terms.take()
        {
            let mut kept = Vec::new();
            for term in terms {
                if self.strict_comparable(term.ty, 0)? {
                    kept.push(term);
                }
            }
            set.terms = Some(kept);
            set.comparable = false;
        }
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
        let terms = set.terms.as_ref()?;
        if terms.len() == 1 {
            return Some((terms[0], true));
        }
        let mut core = self.underlying(terms.first()?.ty)?;
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

    pub(super) fn substitute_set(
        &mut self,
        set: &TypeSet,
        tparams: &[TypeId],
        bindings: &[Option<TypeId>],
    ) -> Option<TypeSet> {
        let mut result = set.clone();
        if let Some(terms) = &mut result.terms {
            for term in terms.iter_mut() {
                term.ty = self.substitute_for_params(term.ty, tparams, bindings, 0)?;
            }
            result.terms = Some(self.normalize_terms(terms.clone())?);
        }
        if let Some(methods) = result.methods {
            result.methods = Some(self.substitute_for_params(methods, tparams, bindings, 0)?);
        }
        for (constraint, ty) in &mut result.checks {
            *constraint = self.substitute_constraint(constraint, tparams, bindings)?;
            *ty = self.substitute_for_params(*ty, tparams, bindings, 0)?;
        }
        self.filter_comparable(result)
    }

    pub(super) fn substitute_constraint(
        &mut self,
        c: &Constraint,
        tparams: &[TypeId],
        bindings: &[Option<TypeId>],
    ) -> Option<Constraint> {
        Some(match c {
            Constraint::Any => Constraint::Any,
            Constraint::Comparable => Constraint::Comparable,
            Constraint::Exact(ty) => {
                Constraint::Exact(self.substitute_for_params(*ty, tparams, bindings, 0)?)
            }
            Constraint::Approximate(ty) => {
                Constraint::Approximate(self.substitute_for_params(*ty, tparams, bindings, 0)?)
            }
            Constraint::Set(set) => Constraint::Set(self.substitute_set(set, tparams, bindings)?),
        })
    }

    pub(super) fn prove_set(&mut self, set: &TypeSet, actual: TypeId, depth: usize) -> Option<()> {
        if set.comparable {
            self.comparable(actual, 0)?.then_some(())?;
        }
        if let Some(terms) = &set.terms {
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
        for (constraint, ty) in &set.checks {
            self.prove_constraint(constraint, *ty, depth + 1)?;
        }
        Some(())
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
        if data.builtin_any {
            return CheckOutcome::Ok(TypeSet::all());
        }
        let mut set = TypeSet {
            comparable: data.comparable,
            interface: true,
            ..TypeSet::all()
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
        if depth >= MAX_DEPTH {
            return CheckOutcome::Unknown(UnknownReason::BudgetExceeded);
        }
        match *self.node(embedded).unwrap_or(&Ty::Invalid) {
            Ty::Invalid => CheckOutcome::Unknown(UnknownReason::MissingMetadata),
            Ty::Union(slice) => {
                let terms = self.terms_slice(embedded, slice).to_vec();
                let mut union_set = TypeSet {
                    terms: Some(Vec::new()),
                    ..TypeSet::all()
                };
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
                            terms: Some(vec![term]),
                            interface: false,
                            ..TypeSet::all()
                        }
                    };
                    union_set.terms = match (union_set.terms, sub.terms) {
                        (Some(mut left), Some(right)) => {
                            left.extend(right);
                            match self.normalize_terms(left) {
                                Some(norm) => Some(norm),
                                None => {
                                    return CheckOutcome::Unknown(UnknownReason::BudgetExceeded);
                                }
                            }
                        }
                        _ => None,
                    };
                    union_set.checks.extend(sub.checks);
                    if union_set.checks.len() > MAX_NODES {
                        return CheckOutcome::Unknown(UnknownReason::BudgetExceeded);
                    }
                }
                CheckOutcome::Ok(union_set)
            }
            Ty::Interface(_) => self.interface_type_set_checked(embedded, depth + 1),
            Ty::Named { .. } | Ty::Alias { .. } => {
                if self.interface_constructor(embedded) == Some(true) {
                    if self.named_info(embedded).is_some() {
                        match self.named_set(embedded, depth + 1) {
                            Some(set) => CheckOutcome::Ok(set),
                            None => CheckOutcome::Unknown(UnknownReason::MissingMetadata),
                        }
                    } else if let Some(root) = self.underlying(embedded) {
                        self.interface_type_set_checked(root, depth + 1)
                    } else {
                        CheckOutcome::Unknown(UnknownReason::MissingMetadata)
                    }
                } else {
                    CheckOutcome::Ok(TypeSet {
                        terms: Some(vec![Term {
                            ty: embedded,
                            approximate: false,
                        }]),
                        interface: false,
                        ..TypeSet::all()
                    })
                }
            }
            _ => CheckOutcome::Ok(TypeSet {
                terms: Some(vec![Term {
                    ty: embedded,
                    approximate: false,
                }]),
                interface: false,
                ..TypeSet::all()
            }),
        }
    }
}
