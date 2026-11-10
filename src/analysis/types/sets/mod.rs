//! Bounded normalized type sets; None terms is universal, an empty list is empty.
mod implication;
mod parse;
use super::constraints::Constraint;
use super::{Direction, MAX_DEPTH, MAX_NODES, Ty, TypeArena, TypeId};

pub(super) const MAX_TERMS: usize = 100;

#[derive(Clone, Copy)]
pub(super) struct Term {
    pub(super) ty: TypeId,
    pub(super) approximate: bool,
}

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
                let Ty::Interface {
                    origin,
                    methods: left,
                    ..
                } = self.nodes[usize::from(previous.0)].clone()
                else {
                    return None;
                };
                let Ty::Interface { methods: right, .. } =
                    self.nodes[usize::from(methods.0)].clone()
                else {
                    return None;
                };
                let mut values = self.methods[left].to_vec();
                values.extend(self.methods[right].iter().cloned());
                self.store_interface(origin, values)?
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
        match self.nodes[usize::from(input.0)].clone() {
            Ty::Interface { .. } => Some(false),
            Ty::Array { element, .. } => self.strict_comparable(element, depth + 1),
            Ty::Struct { fields, .. } => {
                for ty in self.fields[fields].iter().map(|f| f.ty).collect::<Vec<_>>() {
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
                ) = (
                    self.nodes[usize::from(core.0)].clone(),
                    self.nodes[usize::from(ty.0)].clone(),
                )
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
        bindings: &[Option<TypeId>],
    ) -> Option<TypeSet> {
        let mut result = set.clone();
        if let Some(terms) = &mut result.terms {
            for term in terms.iter_mut() {
                term.ty = self.substitute(term.ty, bindings, 0)?;
            }
            result.terms = Some(self.normalize_terms(terms.clone())?);
        }
        if let Some(methods) = result.methods {
            result.methods = Some(self.substitute(methods, bindings, 0)?);
        }
        for (constraint, ty) in &mut result.checks {
            *constraint = self.substitute_constraint(constraint, bindings)?;
            *ty = self.substitute(*ty, bindings, 0)?;
        }
        self.filter_comparable(result)
    }

    pub(super) fn substitute_constraint(
        &mut self,
        c: &Constraint,
        bindings: &[Option<TypeId>],
    ) -> Option<Constraint> {
        Some(match c {
            Constraint::Any => Constraint::Any,
            Constraint::Comparable => Constraint::Comparable,
            Constraint::Exact(ty) => Constraint::Exact(self.substitute(*ty, bindings, 0)?),
            Constraint::Approximate(ty) => {
                Constraint::Approximate(self.substitute(*ty, bindings, 0)?)
            }
            Constraint::Set(set) => Constraint::Set(self.substitute_set(set, bindings)?),
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
                self.workspace.integer_bits()?,
            )?
            .then_some(())?;
        }
        for (constraint, ty) in &set.checks {
            self.prove_constraint(constraint, *ty, depth + 1)?;
        }
        Some(())
    }
}
