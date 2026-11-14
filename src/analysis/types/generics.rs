//! Bounded call inference: typed unification, untyped defaulting, substitution.
use super::constraints;
use super::{BasicKind, MAX_DEPTH, MAX_NODES, Operand, Ty, TypeArena, TypeId};
use crate::syntax::Signature;
use std::path::Path;

impl TypeArena<'_> {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn infer_call(
        &mut self,
        origin: &Path,
        header: &str,
        signature: &Signature,
        actuals: &[Operand],
        explicit: &[TypeId],
        result: usize,
        int_bits: u32,
        spread: bool,
    ) -> Option<TypeId> {
        let arguments = self.infer_call_arguments(
            origin, header, signature, actuals, explicit, int_bits, spread,
        )?;
        let names = constraints::parameter_names(header, signature)?;
        let environment: Vec<_> = names.into_iter().zip(arguments).collect();
        let result = &header[signature.results.get(result)?.annotation.clone()];
        self.parse_in(origin, result, &environment)
    }

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
                self.arguments[aa]
                    .iter()
                    .copied()
                    .zip(self.arguments[ba].iter().copied())
                    .collect()
            }
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
                return self.store_signature(parameters, results, variadic);
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
                let arguments = self.arguments[arguments].to_vec();
                let mut substituted = Vec::new();
                for argument in arguments {
                    substituted.push(self.substitute(argument, bindings, depth + 1)?);
                }
                if self.arguments.len() + substituted.len() > MAX_NODES {
                    return None;
                }
                let start = self.arguments.len();
                self.arguments.extend(substituted);
                Ty::Named {
                    declaration,
                    arguments: start..self.arguments.len(),
                }
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
