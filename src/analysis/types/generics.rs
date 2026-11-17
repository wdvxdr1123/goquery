//! Bounded call inference: typed unification, untyped defaulting, substitution.
use super::constraints::{self, Constraint};
use super::{BasicKind, MAX_DEPTH, MAX_NODES, Operand, Ty, TypeArena, TypeId};
use crate::syntax::Signature;
use std::path::Path;

pub(crate) fn completion_inferable(
    arena: &mut TypeArena<'_>,
    file: &Path,
    parameters: &[(&str, &str)],
    inputs: &[&str],
) -> Option<Vec<bool>> {
    arena.with_projection(true, |arena| {
        completion_inferable_inner(arena, file, parameters, inputs)
    })
}

fn completion_inferable_inner(
    arena: &mut TypeArena<'_>,
    file: &Path,
    parameters: &[(&str, &str)],
    inputs: &[&str],
) -> Option<Vec<bool>> {
    if parameters.len() > 32 {
        return None;
    }
    let base = arena.local_parameters.len();
    let mut environment = Vec::with_capacity(parameters.len());
    for (name, _) in parameters {
        let ty = arena.push_symbolic_parameter(file, name)?;
        if *name != "_" {
            environment.push((*name, ty));
        }
    }
    let mut inferred = vec![false; base + parameters.len()];
    for input in inputs {
        let ty = arena.parse_in(file, input, &environment)?;
        collect_free_params(arena, ty, &mut inferred, 0)?;
    }
    for _ in 0..=parameters.len() {
        let before = inferred.clone();
        for (index, (_, constraint)) in parameters.iter().enumerate() {
            if inferred[base + index] {
                let constraint = arena.constraint(file, constraint, &environment)?;
                collect_constraint_params(arena, &constraint, &mut inferred, 0)?;
            }
        }
        if before == inferred {
            return Some(inferred[base..].to_vec());
        }
    }
    None
}

fn collect_free_params(
    arena: &mut TypeArena<'_>,
    input: TypeId,
    inferred: &mut [bool],
    depth: usize,
) -> Option<()> {
    arena.step()?;
    if depth >= MAX_DEPTH {
        return None;
    }
    let input = arena.unalias(input)?;
    match arena.nodes.get(usize::from(input.0))?.clone() {
        Ty::Invalid => None,
        Ty::Parameter(index) => {
            if let Some(slot) = inferred.get_mut(index) {
                *slot = true;
            }
            Some(())
        }
        Ty::Pointer(element)
        | Ty::Slice(element)
        | Ty::Array { element, .. }
        | Ty::Chan { element, .. } => collect_free_params(arena, element, inferred, depth + 1),
        Ty::Map { key, value } => {
            collect_free_params(arena, key, inferred, depth + 1)?;
            collect_free_params(arena, value, inferred, depth + 1)
        }
        Ty::Tuple(items) => {
            let items = arena.arguments[items.range()].to_vec();
            for item in items {
                collect_free_params(arena, item, inferred, depth + 1)?;
            }
            Some(())
        }
        Ty::Named { arguments, .. } => {
            let arguments = arena.arguments[arguments.range()].to_vec();
            for argument in arguments {
                collect_free_params(arena, argument, inferred, depth + 1)?;
            }
            Some(())
        }
        Ty::Signature {
            parameters,
            results,
            ..
        } => {
            let items = arena.arguments[parameters]
                .iter()
                .chain(&arena.arguments[results])
                .copied()
                .collect::<Vec<_>>();
            for item in items {
                collect_free_params(arena, item, inferred, depth + 1)?;
            }
            Some(())
        }
        Ty::Struct { fields, .. } => {
            let field_tys: Vec<_> = arena.fields[fields].iter().map(|f| f.ty).collect();
            for ty in field_tys {
                collect_free_params(arena, ty, inferred, depth + 1)?;
            }
            Some(())
        }
        Ty::Interface { methods, .. } => {
            let sigs: Vec<_> = arena.methods[methods].iter().map(|m| m.signature).collect();
            for sig in sigs {
                collect_free_params(arena, sig, inferred, depth + 1)?;
            }
            Some(())
        }
        Ty::Basic(_) | Ty::Error(_) | Ty::Alias { .. } | Ty::LocalDeclaration { .. } => Some(()),
    }
}

fn collect_constraint_params(
    arena: &mut TypeArena<'_>,
    constraint: &Constraint,
    inferred: &mut [bool],
    depth: usize,
) -> Option<()> {
    if depth >= MAX_DEPTH {
        return None;
    }
    match constraint {
        Constraint::Any | Constraint::Comparable => Some(()),
        Constraint::Exact(ty) | Constraint::Approximate(ty) => {
            collect_free_params(arena, *ty, inferred, depth + 1)
        }
        Constraint::Set(set) => {
            if let Some(terms) = &set.terms {
                for term in terms {
                    collect_free_params(arena, term.ty, inferred, depth + 1)?;
                }
            }
            if let Some(methods) = set.methods {
                collect_free_params(arena, methods, inferred, depth + 1)?;
            }
            for (nested, actual) in &set.checks {
                collect_free_params(arena, *actual, inferred, depth + 1)?;
                collect_constraint_params(arena, nested, inferred, depth + 1)?;
            }
            Some(())
        }
    }
}

impl TypeArena<'_> {
    /// Infer and prove the complete type argument list once, for result types
    /// and call-site signature presentation alike.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn infer_call_arguments(
        &mut self,
        origin: &Path,
        header: &str,
        signature: &Signature,
        actuals: &[Operand],
        explicit: &[TypeId],
        int_bits: u32,
        spread: bool,
    ) -> Option<Vec<TypeId>> {
        let names = constraints::parameter_names(header, signature)?;
        if names.is_empty()
            || names.len() > 32
            || explicit.len() > names.len()
            || actuals.len() > MAX_NODES
        {
            return None;
        }
        let mut environment = Vec::new();
        for (index, name) in names.into_iter().enumerate() {
            environment.push((name, self.push(Ty::Parameter(index))?));
        }
        let constraints = self.constraints(origin, header, signature, &environment)?;
        for ((_, ty), constraint) in environment.iter().zip(&constraints) {
            if self.parameter_constraints.len() < self.node_limit {
                self.parameter_constraints.push((*ty, constraint.clone()));
            }
        }
        let variadic = signature.parameters.last().is_some_and(|p| p.variadic);
        let fixed = signature
            .parameters
            .len()
            .checked_sub(usize::from(variadic))?;
        if (!variadic && (spread || actuals.len() != fixed))
            || (variadic && (actuals.len() < fixed || (spread && actuals.len() != fixed + 1)))
        {
            return None;
        }
        let mut formals = Vec::new();
        for index in 0..actuals.len() {
            let parameter = signature.parameters.get(index.min(fixed))?;
            let annotation = &header[parameter.annotation.clone()];
            let mut ty = self.parse_in(origin, annotation, &environment)?;
            if parameter.variadic && spread {
                ty = self.push(Ty::Slice(ty))?;
            }
            formals.push(ty);
        }
        let mut bindings = vec![None; environment.len()];
        for (slot, argument) in bindings.iter_mut().zip(explicit) {
            *slot = Some(*argument);
        }
        // Typed arguments constrain parameters before untyped constants default.
        for (formal, actual) in formals.iter().zip(actuals) {
            if !self
                .basic_kind(actual.ty)
                .is_some_and(|kind| kind.untyped())
            {
                self.unify(*formal, actual.ty, &mut bindings, 0)?;
            }
        }
        self.infer_constraints(&constraints, &mut bindings)?;
        let mut untyped: Vec<Option<BasicKind>> = vec![None; bindings.len()];
        for (formal, actual) in formals.iter().zip(actuals) {
            let Some(kind) = self
                .basic_kind(actual.ty)
                .filter(|kind| kind.untyped() && *kind != BasicKind::UntypedNil)
            else {
                continue;
            };
            if let Ty::Parameter(index) = self.nodes[usize::from(formal.0)]
                && bindings[index].is_none()
            {
                untyped[index] = Some(match untyped[index] {
                    None => kind,
                    Some(previous) if previous == kind => kind,
                    Some(previous) => {
                        if previous.numeric_rank()? > kind.numeric_rank()? {
                            previous
                        } else {
                            kind
                        }
                    }
                });
            }
        }
        for (slot, kind) in bindings.iter_mut().zip(untyped) {
            if slot.is_none()
                && let Some(kind) = kind
            {
                *slot = Some(self.basic(kind.default_type())?);
            }
        }
        self.infer_constraints(&constraints, &mut bindings)?;
        self.prove_constraints(&constraints, &bindings)?;
        // Inference is not a proof of assignment or constant representability.
        for (formal, actual) in formals.into_iter().zip(actuals) {
            let target = self.substitute(formal, &bindings, 0)?;
            self.assignable(*actual, target, int_bits)?.then_some(())?;
        }
        bindings.into_iter().collect()
    }

    pub(super) fn unify(
        &mut self,
        formal: TypeId,
        actual: TypeId,
        bindings: &mut [Option<TypeId>],
        depth: usize,
    ) -> Option<()> {
        self.step()?;
        if depth >= MAX_DEPTH {
            return None;
        }
        let (mut formal, mut actual) = (self.unalias(formal)?, self.unalias(actual)?);
        if let Ty::Parameter(index) = self.nodes[usize::from(formal.0)] {
            if let Some(previous) = bindings[index] {
                return self.identical(previous, actual)?.then_some(());
            }
            bindings[index] = Some(actual);
            return Some(());
        }
        if depth == 0 && self.interface_constructor(formal)? {
            let root = self.underlying(formal)?;
            let Ty::Interface {
                methods: required, ..
            } = self.nodes[usize::from(root.0)].clone()
            else {
                return None;
            };
            for index in required {
                let method = self.methods[index].clone();
                let signature = self.lookup_method(actual, &method)??;
                self.unify(method.signature, signature, bindings, depth + 1)?;
            }
            return Some(());
        }
        // Assignment may compare an unnamed outer constructor with a named
        // argument's underlying type. Nested constructors require exact identity.
        if depth == 0
            && matches!(
                self.nodes[usize::from(formal.0)],
                Ty::Pointer(_)
                    | Ty::Slice(_)
                    | Ty::Array { .. }
                    | Ty::Map { .. }
                    | Ty::Chan { .. }
                    | Ty::Struct { .. }
                    | Ty::Signature { .. }
            )
        {
            actual = self.underlying(actual)?;
        }
        if depth == 0
            && matches!(self.nodes[usize::from(formal.0)], Ty::Named { .. })
            && matches!(
                self.nodes[usize::from(actual.0)],
                Ty::Pointer(_)
                    | Ty::Slice(_)
                    | Ty::Array { .. }
                    | Ty::Map { .. }
                    | Ty::Chan { .. }
                    | Ty::Struct { .. }
                    | Ty::Signature { .. }
            )
        {
            formal = self.underlying(formal)?;
        }
        let pairs = match (
            self.nodes[usize::from(formal.0)].clone(),
            self.nodes[usize::from(actual.0)].clone(),
        ) {
            (Ty::Error(_), Ty::Error(_)) => Vec::new(),
            (Ty::Interface { methods: a, .. }, Ty::Interface { methods: b, .. }) => {
                self.same_interface_methods(a.clone(), b.clone())?
                    .then_some(())?;
                self.methods[a]
                    .iter()
                    .map(|m| m.signature)
                    .zip(self.methods[b].iter().map(|m| m.signature))
                    .collect()
            }
            (Ty::Pointer(a), Ty::Pointer(b)) | (Ty::Slice(a), Ty::Slice(b)) => vec![(a, b)],
            (
                Ty::Signature {
                    parameters: a,
                    results: ar,
                    variadic: av,
                },
                Ty::Signature {
                    parameters: b,
                    results: br,
                    variadic: bv,
                },
            ) if av == bv && a.len() == b.len() && ar.len() == br.len() => self.arguments[a]
                .iter()
                .chain(&self.arguments[ar])
                .copied()
                .zip(self.arguments[b].iter().chain(&self.arguments[br]).copied())
                .collect(),
            (
                Ty::Struct {
                    origin: a,
                    fields: aa,
                },
                Ty::Struct {
                    origin: b,
                    fields: ba,
                },
            ) => {
                self.same_struct_fields(a, aa.clone(), b, ba.clone())?
                    .then_some(())?;
                self.fields[aa]
                    .iter()
                    .map(|f| f.ty)
                    .zip(self.fields[ba].iter().map(|f| f.ty))
                    .collect()
            }
            (
                Ty::Array {
                    length: a,
                    element: ae,
                },
                Ty::Array {
                    length: b,
                    element: be,
                },
            ) if a == b => vec![(ae, be)],
            (Ty::Map { key: a, value: av }, Ty::Map { key: b, value: bv }) => {
                vec![(a, b), (av, bv)]
            }
            (
                Ty::Chan {
                    direction: a,
                    element: ae,
                },
                Ty::Chan {
                    direction: b,
                    element: be,
                },
            ) if a == b || (depth == 0 && b == super::Direction::Both) => vec![(ae, be)],
            (
                Ty::Named {
                    declaration: a,
                    arguments: aa,
                },
                Ty::Named {
                    declaration: b,
                    arguments: ba,
                },
            ) if a.file == b.file && a.symbol.start == b.symbol.start && aa.len() == ba.len() => {
                if a.symbol.generic && aa.is_empty() {
                    return None;
                }
                self.arguments[aa.range()]
                    .iter()
                    .copied()
                    .zip(self.arguments[ba.range()].iter().copied())
                    .collect()
            }
            (Ty::Tuple(a), Ty::Tuple(b)) if a.len() == b.len() => self.arguments[a.range()]
                .iter()
                .copied()
                .zip(self.arguments[b.range()].iter().copied())
                .collect(),
            _ => return self.identical(formal, actual)?.then_some(()),
        };
        for (a, b) in pairs {
            self.unify(a, b, bindings, depth + 1)?;
        }
        Some(())
    }

    pub(super) fn substitute(
        &mut self,
        input: TypeId,
        bindings: &[Option<TypeId>],
        depth: usize,
    ) -> Option<TypeId> {
        self.step()?;
        if depth >= MAX_DEPTH {
            return None;
        }
        let node = match self.nodes[usize::from(input.0)].clone() {
            Ty::Invalid => return None,
            Ty::Parameter(index) => return *bindings.get(index)?,
            Ty::Basic(_) | Ty::Error(_) | Ty::LocalDeclaration { .. } => return Some(input),
            Ty::Interface {
                origin,
                methods,
                builtin_any,
            } => {
                if builtin_any {
                    return Some(input);
                }
                let mut methods = self.methods[methods].to_vec();
                for method in &mut methods {
                    method.signature = self.substitute(method.signature, bindings, depth + 1)?;
                    if let Some(receiver) = method.declared_on {
                        method.declared_on =
                            Some(self.substitute(receiver, bindings, depth + 1)?);
                    }
                }
                return self.store_interface(origin, methods);
            }
            Ty::Signature {
                parameters,
                results,
                variadic,
            } => {
                let (parameters, results) = (
                    self.arguments[parameters].to_vec(),
                    self.arguments[results].to_vec(),
                );
                let parameters = parameters
                    .into_iter()
                    .map(|ty| self.substitute(ty, bindings, depth + 1))
                    .collect::<Option<_>>()?;
                let results = results
                    .into_iter()
                    .map(|ty| self.substitute(ty, bindings, depth + 1))
                    .collect::<Option<_>>()?;
                let ty = self.store_signature(parameters, results, variadic)?;
                self.copy_signature_names(input, ty, false);
                return Some(ty);
            }
            Ty::Struct { origin, fields } => {
                let mut fields = self.fields[fields].to_vec();
                for field in &mut fields {
                    field.ty = self.substitute(field.ty, bindings, depth + 1)?;
                }
                return self.store_struct(origin, fields);
            }
            Ty::Named {
                declaration,
                arguments,
            } => {
                let arguments = self.arguments[arguments.range()].to_vec();
                let mut substituted = Vec::with_capacity(arguments.len());
                for argument in arguments {
                    substituted.push(self.substitute(argument, bindings, depth + 1)?);
                }
                return self.push_named(declaration, &substituted, false);
            }
            Ty::Alias {
                declaration,
                arguments,
            } => {
                let arguments = self.arguments[arguments.range()].to_vec();
                let mut substituted = Vec::with_capacity(arguments.len());
                for argument in arguments {
                    substituted.push(self.substitute(argument, bindings, depth + 1)?);
                }
                return self.push_named(declaration, &substituted, true);
            }
            Ty::Tuple(elements) => {
                let elements = self.arguments[elements.range()].to_vec();
                let mut substituted = Vec::with_capacity(elements.len());
                for element in elements {
                    substituted.push(self.substitute(element, bindings, depth + 1)?);
                }
                return self.tuple(&substituted);
            }
            Ty::Pointer(element) => Ty::Pointer(self.substitute(element, bindings, depth + 1)?),
            Ty::Slice(element) => Ty::Slice(self.substitute(element, bindings, depth + 1)?),
            Ty::Array { length, element } => Ty::Array {
                length,
                element: self.substitute(element, bindings, depth + 1)?,
            },
            Ty::Map { key, value } => Ty::Map {
                key: self.substitute(key, bindings, depth + 1)?,
                value: self.substitute(value, bindings, depth + 1)?,
            },
            Ty::Chan { direction, element } => Ty::Chan {
                direction,
                element: self.substitute(element, bindings, depth + 1)?,
            },
        };
        self.push(node)
    }
}
