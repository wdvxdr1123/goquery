//! Lazy generic-owner bound checks; no declaration body or source is retained.
use super::constraints;
use super::{MAX_DEPTH, Ty, TypeArena, TypeId};

pub(super) fn parse_type_parameters_of(
    source: &str,
    generic: bool,
) -> Option<(&str, crate::syntax::Signature)> {
    match crate::syntax::type_parameter_end(source) {
        Some(end) => {
            let slice = source.get(..end)?;
            Some((slice, crate::syntax::parse_type_parameters(slice)))
        }
        None if !generic => Some(("", crate::syntax::Signature::default())),
        None => None,
    }
}

impl TypeArena<'_> {
    pub(super) fn type_parameter_source(
        &self,
        declaration: crate::analysis::obj::DeclId<'_>,
    ) -> Option<String> {
        if !declaration.symbol.generic {
            return Some(String::new());
        }
        let start = declaration.symbol.end;
        let length = declaration
            .symbol
            .type_info
            .as_ref()
            .map_or(super::MAX_BYTES, |info| usize::from(info.parameter_end));
        if length == 0 || length > super::MAX_BYTES {
            return None;
        }
        let end = declaration
            .symbol
            .declaration()?
            .end
            .min(start.checked_add(length)?);
        self.workspace.source_slice(declaration.file, start..end)
    }

    pub(crate) fn check_named_arguments(&mut self, input: TypeId) -> Option<()> {
        if self.named_check_depth >= MAX_DEPTH {
            return None;
        }
        self.named_check_depth += 1;
        let result = self.check_named_arguments_inner(input);
        self.named_check_depth -= 1;
        result
    }

    fn check_named_arguments_inner(&mut self, input: TypeId) -> Option<()> {
        self.step()?;
        let (declaration, arguments) = match self.nodes[usize::from(input.0)].clone() {
            Ty::Named {
                declaration,
                arguments,
            }
            | Ty::Alias {
                declaration,
                arguments,
            } => (declaration, arguments),
            _ => return Some(()),
        };
        if arguments.is_empty() && !declaration.symbol.generic {
            return Some(());
        }
        if let Some((_, valid)) = self.named_checks.iter().find(|(ty, _)| *ty == input) {
            return (*valid).then_some(());
        }
        if self.named_checks.len() >= self.node_limit {
            return None;
        }
        // Pending checks break cycles; failed non-invalid checks are removed so
        // interdependent parameter-bound passes can retry once earlier bounds land.
        self.named_checks.push((input, false));
        let ok = self
            .check_named_arguments_step(declaration, arguments)
            .is_some();
        let slot = self.named_checks.iter().position(|(ty, _)| *ty == input);
        if ok {
            if let Some(slot) = slot {
                self.named_checks[slot].1 = true;
            }
            Some(())
        } else {
            if !self.invalid_type
                && let Some(slot) = slot
            {
                self.named_checks.remove(slot);
            }
            None
        }
    }

    fn check_named_arguments_step(
        &mut self,
        declaration: crate::analysis::obj::DeclId<'_>,
        arguments: super::ty::SliceRef,
    ) -> Option<()> {
        let source = self.type_parameter_source(declaration)?;
        let (header, parsed) = parse_type_parameters_of(&source, declaration.symbol.generic)?;
        if !parsed.diagnostics.is_empty() {
            return None;
        }
        let names = constraints::parameter_names(header, &parsed)?;
        let arguments = self.arguments[arguments.range()].to_vec();
        if names.len() != arguments.len() || names.len() > self.node_limit {
            return None;
        }
        if self.symbolic {
            return Some(());
        }
        let environment = names
            .into_iter()
            .zip(arguments.iter().copied())
            .collect::<Vec<_>>();
        let bounds = self.constraints(declaration.file, header, &parsed, &environment)?;
        for (bound, argument) in bounds.iter().zip(arguments) {
            self.prove_constraint(bound, argument, 0)?;
        }
        Some(())
    }

    /// Recursively verify generic type arity and constraints within a type node.
    pub(crate) fn validate_type(&mut self, input: TypeId) -> Option<()> {
        self.validate_type_inner(input, 0)
    }

    fn validate_type_inner(&mut self, input: TypeId, depth: usize) -> Option<()> {
        self.step()?;
        if depth >= MAX_DEPTH {
            return None;
        }
        match self.nodes.get(usize::from(input.0))?.clone() {
            Ty::Invalid => None,
            Ty::Alias { .. } => {
                let target = self.unalias(input)?;
                self.validate_type_inner(target, depth + 1)
            }
            Ty::Named { arguments, .. } => {
                let args = self.arguments[arguments.range()].to_vec();
                self.check_named_arguments(input)?;
                for arg in args {
                    self.validate_type_inner(arg, depth + 1)?;
                }
                Some(())
            }
            Ty::Pointer(element)
            | Ty::Slice(element)
            | Ty::Array { element, .. }
            | Ty::Chan { element, .. } => self.validate_type_inner(element, depth + 1),
            Ty::Map { key, value } => {
                self.validate_type_inner(key, depth + 1)?;
                self.validate_type_inner(value, depth + 1)
            }
            Ty::Struct { fields, .. } => {
                let field_tys: Vec<TypeId> =
                    self.fields[fields].iter().map(|field| field.ty).collect();
                for field_ty in field_tys {
                    self.validate_type_inner(field_ty, depth + 1)?;
                }
                Some(())
            }
            Ty::Interface { methods, .. } => {
                let method_sigs: Vec<TypeId> = self.methods[methods]
                    .iter()
                    .map(|method| method.signature)
                    .collect();
                for method_sig in method_sigs {
                    self.validate_type_inner(method_sig, depth + 1)?;
                }
                Some(())
            }
            Ty::Signature {
                parameters,
                results,
                ..
            } => {
                let params = self.arguments[parameters].to_vec();
                let res = self.arguments[results].to_vec();
                for param in params {
                    self.validate_type_inner(param, depth + 1)?;
                }
                for result in res {
                    self.validate_type_inner(result, depth + 1)?;
                }
                Some(())
            }
            Ty::Tuple(elements) => {
                let elems = self.arguments[elements.range()].to_vec();
                for elem in elems {
                    self.validate_type_inner(elem, depth + 1)?;
                }
                Some(())
            }
            Ty::Basic(_) | Ty::Error(_) | Ty::Parameter(_) | Ty::LocalDeclaration { .. } => {
                Some(())
            }
        }
    }
}
