//! Function signatures use bounded shared type-ID lists, without names or bodies.
use super::arena::DisplayScope;
use super::constraints;
use super::{
    BasicKind, Direction, MAX_BYTES, MAX_DEPTH, MAX_NODES, Operand, Ty, TypeArena, TypeId,
};
use crate::syntax::{Lexer, Signature, TokenKind};
use std::ops::Range;
use std::path::Path;

impl TypeArena<'_> {
    /// Receiver names are fresh formals; owner bounds use the declaring file's imports.
    pub(super) fn import_receiver_parameters(
        &mut self,
        origin: &Path,
        receiver: &str,
        environment: &[(&str, TypeId)],
    ) -> Option<TypeId> {
        let receiver = self.parse_in(origin, receiver, environment)?;
        let base = match self.nodes[usize::from(receiver.0)] {
            Ty::Pointer(base) => base,
            _ => receiver,
        };
        let Ty::Named {
            declaration,
            arguments,
        } = self.nodes[usize::from(base.0)].clone()
        else {
            return None;
        };
        let (file, symbol) = (declaration.file, declaration.symbol);
        if symbol.alias || !symbol.generic || !self.same_field_package(file, origin)? {
            return None;
        }
        let arguments = self.arguments[arguments].to_vec();
        if arguments.len() > 32
            || arguments.iter().enumerate().any(|(index, ty)| {
                !matches!(self.nodes[usize::from(ty.0)], Ty::Parameter(_))
                    || arguments[..index].contains(ty)
            })
        {
            return None;
        }
        let source = self
            .workspace
            .source_slice(file, symbol.end..symbol.declaration()?.end)?;
        let end = crate::syntax::type_parameter_end(&source)?;
        let header = format!("func _{}()", source.get(..end)?);
        let signature = crate::syntax::parse_signature(&header);
        if !signature.diagnostics.is_empty() {
            return None;
        }
        let names = constraints::parameter_names(&header, &signature)?;
        if names.len() != arguments.len() {
            return None;
        }
        let environment: Vec<_> = names.into_iter().zip(arguments.iter().copied()).collect();
        let bounds: Vec<_> = signature
            .type_parameters
            .iter()
            .zip(&arguments)
            .map(|(parameter, ty)| (*ty, &header[parameter.annotation.clone()]))
            .collect();
        self.import_parameter_bounds(file, &bounds, &environment)?;
        Some(base)
    }

    pub(crate) fn signature(
        &mut self,
        origin: &Path,
        header: &str,
        signature: &Signature,
        environment: &[(&str, TypeId)],
        depth: usize,
    ) -> Option<TypeId> {
        if depth >= MAX_DEPTH || header.len() > MAX_BYTES || !signature.diagnostics.is_empty() {
            return None;
        }
        if signature.type_parameters.iter().any(|parameter| {
            parameter.name.as_ref().is_none_or(|name| {
                !environment
                    .iter()
                    .any(|(key, _)| *key == &header[name.clone()])
            })
        }) {
            return None;
        }
        let variadic = signature.parameters.last().is_some_and(|p| p.variadic);
        let mut parameters = Vec::new();
        let mut results = Vec::new();
        for (items, output) in [
            (&signature.parameters, &mut parameters),
            (&signature.results, &mut results),
        ] {
            if items.len() > MAX_NODES {
                return None;
            }
            for item in items {
                let source = &header[item.annotation.clone()];
                let mut tokens = Lexer::new(source).peekable();
                let mut ty = self.read(origin, source, &mut tokens, environment, depth + 1)?;
                if !tokens.all(|token| token.kind == TokenKind::Semicolon) {
                    return None;
                }
                if item.variadic {
                    ty = self.push(Ty::Slice(ty))?;
                }
                output.push(ty);
            }
        }
        self.store_signature(parameters, results, variadic)
    }

    pub(super) fn store_signature(
        &mut self,
        parameters: Vec<TypeId>,
        results: Vec<TypeId>,
        variadic: bool,
    ) -> Option<TypeId> {
        if self.arguments.len() + parameters.len() + results.len() > MAX_NODES {
            return None;
        }
        let start = self.arguments.len();
        self.arguments.extend(parameters);
        let middle = self.arguments.len();
        self.arguments.extend(results);
        self.push(Ty::Signature {
            parameters: start..middle,
            results: middle..self.arguments.len(),
            variadic,
        })
    }

    pub(crate) fn instantiated_signature(
        &mut self,
        origin: &Path,
        header: &str,
        signature: &Signature,
        arguments: &[TypeId],
    ) -> Option<TypeId> {
        let names = constraints::parameter_names(header, signature)?;
        if names.len() != arguments.len() {
            return None;
        }
        let environment: Vec<_> = names.into_iter().zip(arguments.iter().copied()).collect();
        let constraints = self.constraints(origin, header, signature, &environment)?;
        let bindings: Vec<_> = arguments.iter().copied().map(Some).collect();
        self.prove_constraints(&constraints, &bindings)?;
        self.signature(origin, header, signature, &environment, 0)
    }

    pub(crate) fn is_callable(&mut self, input: TypeId) -> Option<bool> {
        let input = self.underlying(input)?;
        Some(matches!(
            self.nodes[usize::from(input.0)],
            Ty::Signature { .. }
        ))
    }

    pub(crate) fn result_count(&mut self, input: TypeId) -> Option<usize> {
        let input = self.underlying(input)?;
        match &self.nodes[usize::from(input.0)] {
            Ty::Signature { results, .. } => Some(results.len()),
            _ => None,
        }
    }

    /// Last fixed parameter of a variadic callable, as used by gopls for
    /// format-literal tokens and argument highlights. No naming convention is
    /// involved and the actual argument need not be assignable to prove arity.
    pub(crate) fn format_parameter(&self, input: TypeId) -> Option<usize> {
        // The reference requires a signature itself, rather than the
        // underlying signature of a defined function type.
        match &self.nodes[usize::from(input.0)] {
            Ty::Signature {
                parameters,
                variadic: true,
                ..
            } => parameters.len().checked_sub(2),
            _ => None,
        }
    }

    pub(crate) fn call_signature(
        &mut self,
        callee: TypeId,
        actuals: &[Operand],
        spread: bool,
        result: usize,
        count: usize,
        int_bits: u32,
    ) -> Option<TypeId> {
        let callee = self.underlying(callee)?;
        let Ty::Signature {
            parameters,
            results,
            variadic,
        } = self.nodes[usize::from(callee.0)].clone()
        else {
            return None;
        };
        if results.len() != count || result >= count || actuals.len() > MAX_NODES {
            return None;
        }
        let parameters = self.arguments[parameters].to_vec();
        let fixed = parameters.len().checked_sub(usize::from(variadic))?;
        if (!variadic && (spread || actuals.len() != fixed))
            || (variadic && (actuals.len() < fixed || spread && actuals.len() != fixed + 1))
        {
            return None;
        }
        for (index, actual) in actuals.iter().enumerate() {
            let mut target = *parameters.get(index.min(fixed))?;
            if variadic && index >= fixed && !spread {
                let Ty::Slice(element) = self.nodes[usize::from(target.0)] else {
                    return None;
                };
                target = element;
            }
            self.assignable(*actual, target, int_bits)?.then_some(())?;
        }
        self.arguments.get(results.start + result).copied()
    }

    pub(super) fn assignable(
        &mut self,
        source: Operand,
        target: TypeId,
        int_bits: u32,
    ) -> Option<bool> {
        self.step()?;
        // Nominal equality does not require expanding a possibly large body.
        // Interface constructors still need their complete basic method set.
        if !self.interface_constructor(target)? && self.identical(source.ty, target)? {
            return Some(true);
        }
        if self.is_interface(target)? {
            return self.assign_interface(source, target, int_bits);
        }
        if let Some(kind) = self.basic_kind(source.ty).filter(|kind| kind.untyped()) {
            if kind == BasicKind::UntypedNil {
                return self.nilable(target);
            }
            return self.assign_basic(source, target, int_bits).map(|_| true);
        }
        if self.identical(source.ty, target)? {
            return Some(true);
        }
        let (a, b) = (self.unalias(source.ty)?, self.unalias(target)?);
        if matches!(
            self.nodes[usize::from(a.0)],
            Ty::Named { .. } | Ty::Basic(_) | Ty::Parameter(_)
        ) && matches!(
            self.nodes[usize::from(b.0)],
            Ty::Named { .. } | Ty::Basic(_) | Ty::Parameter(_)
        ) {
            return Some(false);
        }
        let (a, b) = (self.underlying(a)?, self.underlying(b)?);
        if self.identical(a, b)? {
            return Some(true);
        }
        match (
            self.nodes[usize::from(a.0)].clone(),
            self.nodes[usize::from(b.0)].clone(),
        ) {
            (
                Ty::Chan {
                    direction: Direction::Both,
                    element: a,
                },
                Ty::Chan { element: b, .. },
            ) => self.identical(a, b),
            _ => Some(false),
        }
    }

    pub(super) fn write_signature(
        &self,
        parameters: Range<usize>,
        results: Range<usize>,
        variadic: bool,
        consumer: DisplayScope<'_>,
        out: &mut String,
        depth: usize,
    ) -> Option<()> {
        out.push_str("func(");
        let count = parameters.len();
        for (index, ty) in self.arguments[parameters].iter().enumerate() {
            if index > 0 {
                out.push_str(", ");
            }
            if variadic && index + 1 == count {
                let Ty::Slice(element) = self.nodes[usize::from(ty.0)] else {
                    return None;
                };
                out.push_str("...");
                self.write_in(element, consumer, out, depth + 1)?;
            } else {
                self.write_in(*ty, consumer, out, depth + 1)?;
            }
        }
        out.push(')');
        if !results.is_empty() {
            out.push_str(" (");
            for (index, ty) in self.arguments[results].iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                self.write_in(*ty, consumer, out, depth + 1)?;
            }
            out.push(')');
        }
        (out.len() <= MAX_BYTES).then_some(())
    }
}
