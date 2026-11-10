//! Bounded, request-local matching of scoped method-signature fingerprints.
use std::collections::HashMap;
use std::ops::Range;

pub(super) fn pack(tag: &str, parts: &[String]) -> Option<String> {
    let mut result = format!("{tag};");
    for part in parts {
        if result.len().saturating_add(part.len()).saturating_add(24) > 16 * 1024 {
            return None;
        }
        result.push_str(&format!("{}:{part}", part.len()));
    }
    Some(result)
}

type Parameter = (u8, usize);

enum Term<'a> {
    Atom(&'a str),
    Parameter(Parameter),
    Constructor(&'a str, Range<usize>),
}

struct Matcher<'a> {
    terms: Vec<Term<'a>>,
    edges: Vec<usize>,
    bindings: HashMap<Parameter, usize>,
    work: usize,
}

impl<'a> Matcher<'a> {
    fn parse(&mut self, text: &'a str, side: u8, depth: usize) -> Option<usize> {
        if depth > 64 || text.len() > 16 * 1024 || self.terms.len() >= 4096 {
            return None;
        }
        let (tag, mut tail) = text.split_once(';')?;
        let mut parts = Vec::new();
        while !tail.is_empty() {
            let (length, rest) = tail.split_once(':')?;
            let length: usize = length.parse().ok()?;
            parts.push(rest.get(..length)?);
            tail = rest.get(length..)?;
            if parts.len() > 4096 {
                return None;
            }
        }
        let term = match tag {
            "atom" if parts.len() == 1 => Term::Atom(parts[0]),
            "parameter" if parts.len() == 1 => Term::Parameter((side, parts[0].parse().ok()?)),
            "atom" | "parameter" => return None,
            _ => {
                let mut children = Vec::new();
                for part in parts {
                    children.push(self.parse(part, side, depth + 1)?);
                }
                let start = self.edges.len();
                self.edges.extend(children);
                Term::Constructor(tag, start..self.edges.len())
            }
        };
        if self.terms.len() >= 4096 {
            return None;
        }
        let id = self.terms.len();
        self.terms.push(term);
        Some(id)
    }

    fn step(&mut self) -> Option<()> {
        self.work = self.work.checked_sub(1)?;
        Some(())
    }

    fn resolve(&mut self, mut id: usize) -> Option<usize> {
        let mut path = Vec::new();
        while let Term::Parameter(parameter) = self.terms[id] {
            self.step()?;
            let Some(&next) = self.bindings.get(&parameter) else {
                break;
            };
            path.push(parameter);
            id = next;
        }
        for parameter in path {
            self.bindings.insert(parameter, id);
        }
        Some(id)
    }

    fn occurs(&mut self, parameter: Parameter, input: usize) -> Option<bool> {
        let mut pending = vec![input];
        let mut seen = vec![false; self.terms.len()];
        while let Some(id) = pending.pop() {
            self.step()?;
            let id = self.resolve(id)?;
            if seen[id] {
                continue;
            }
            seen[id] = true;
            match &self.terms[id] {
                Term::Parameter(other) if *other == parameter => return Some(true),
                Term::Constructor(_, children) => {
                    pending.extend_from_slice(&self.edges[children.clone()])
                }
                _ => {}
            }
        }
        Some(false)
    }

    fn unify(&mut self, left: usize, right: usize) -> Option<bool> {
        let mut pending = vec![(left, right)];
        while let Some((left, right)) = pending.pop() {
            self.step()?;
            let left = self.resolve(left)?;
            let right = self.resolve(right)?;
            if left == right {
                continue;
            }
            match (&self.terms[left], &self.terms[right]) {
                (Term::Parameter(a), Term::Parameter(b)) if a == b => {}
                (Term::Parameter(parameter), _) => {
                    let parameter = *parameter;
                    if self.occurs(parameter, right)? {
                        return Some(false);
                    }
                    self.bindings.insert(parameter, right);
                }
                (_, Term::Parameter(parameter)) => {
                    let parameter = *parameter;
                    if self.occurs(parameter, left)? {
                        return Some(false);
                    }
                    self.bindings.insert(parameter, left);
                }
                (Term::Atom(a), Term::Atom(b)) if a == b => {}
                (Term::Constructor(a, ac), Term::Constructor(b, bc))
                    if a == b && ac.len() == bc.len() =>
                {
                    pending.extend(
                        self.edges[ac.clone()]
                            .iter()
                            .copied()
                            .zip(self.edges[bc.clone()].iter().copied()),
                    );
                }
                _ => return Some(false),
            }
        }
        Some(true)
    }
}

/// Potential signature identity under some instantiation. Like gopls's method
/// fingerprints, this ignores constraints and starts fresh for each method.
pub(crate) fn matches(left: &str, right: &str) -> bool {
    if left == right {
        return true;
    }
    if !left.contains("parameter;") && !right.contains("parameter;") {
        return false;
    }
    let mut matcher = Matcher {
        terms: Vec::new(),
        edges: Vec::new(),
        bindings: HashMap::new(),
        work: 65_536,
    };
    let Some((left, right)) = matcher.parse(left, 0, 0).zip(matcher.parse(right, 1, 0)) else {
        return false;
    };
    matcher.unify(left, right) == Some(true)
}
