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

impl<'a> TypeArena<'a> {
    pub(super) fn type_parameter_source(
        &self,
        declaration: crate::analysis::engine::obj::DeclId<'_>,
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
        let Some((declaration, arguments, _)) = self.named_info(input) else {
            return Some(());
        };
        if arguments.is_empty() && !declaration.symbol.generic {
            return Some(());
        }
        if let Some(valid) = self.named_check_get(input) {
            return valid.then_some(());
        }
        // Pending checks break cycles; failed non-invalid checks are removed so
        // interdependent parameter-bound passes can retry once earlier bounds land.
        if !self.named_check_set(input, false) {
            return None;
        }
        match self.check_named_arguments_step(input, declaration, arguments) {
            super::CheckOutcome::Ok(()) => {
                self.named_check_set(input, true);
                Some(())
            }
            super::CheckOutcome::Invalid(()) => None,
            super::CheckOutcome::Unknown(_) => {
                self.named_check_remove(input);
                None
            }
        }
    }

    fn check_named_arguments_step(
        &mut self,
        input: TypeId,
        declaration: crate::analysis::engine::obj::DeclId<'_>,
        arguments: super::ty::SliceRef,
    ) -> super::CheckOutcome<()> {
        use super::{CheckOutcome, UnknownReason};
        let Some(source) = self.type_parameter_source(declaration) else {
            return CheckOutcome::Unknown(UnknownReason::MissingMetadata);
        };
        let Some((header, parsed)) = parse_type_parameters_of(&source, declaration.symbol.generic)
        else {
            return CheckOutcome::Unknown(UnknownReason::SyntaxError);
        };
        if !parsed.diagnostics.is_empty() {
            return CheckOutcome::Unknown(UnknownReason::SyntaxError);
        }
        let Some(names) = constraints::parameter_names(header, &parsed) else {
            return CheckOutcome::Unknown(UnknownReason::SyntaxError);
        };
        let arguments = self.arguments_slice(input, arguments).to_vec();
        if names.len() != arguments.len() || names.len() > self.active_node_limit() {
            return CheckOutcome::Unknown(UnknownReason::BudgetExceeded);
        }
        if self.mode.is_symbolic() {
            return CheckOutcome::Ok(());
        }
        let environment = names
            .into_iter()
            .zip(arguments.iter().copied())
            .collect::<Vec<_>>();
        let Some(bounds) = self.constraints(declaration.file, header, &parsed, &environment) else {
            return CheckOutcome::Unknown(UnknownReason::MissingMetadata);
        };
        for (bound, argument) in bounds.iter().zip(arguments) {
            if self.prove_constraint(bound, argument, 0).is_none() {
                return CheckOutcome::Unknown(UnknownReason::Unsupported);
            }
        }
        CheckOutcome::Ok(())
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
        match *self.node(input)? {
            Ty::Invalid => None,
            Ty::Alias { .. } => {
                let target = self.unalias(input)?;
                self.validate_type_inner(target, depth + 1)
            }
            Ty::Named { arguments, .. } => {
                if self.named_info(input).is_some() {
                    let args = self.arguments_slice(input, arguments).to_vec();
                    self.check_named_arguments(input)?;
                    for arg in args {
                        self.validate_type_inner(arg, depth + 1)?;
                    }
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
            Ty::Struct(_) => {
                let fields = self.struct_data(input)?.fields.clone();
                let field_tys: Vec<TypeId> = self
                    .fields_slice(input, fields)?
                    .iter()
                    .map(|field| field.ty)
                    .collect();
                for field_ty in field_tys {
                    self.validate_type_inner(field_ty, depth + 1)?;
                }
                Some(())
            }
            Ty::Interface(_) => {
                let data = self.interface_data(input)?.clone();
                let method_sigs: Vec<TypeId> = self
                    .methods_slice(input, data.methods)
                    .iter()
                    .map(|method| method.signature)
                    .collect();
                for method_sig in method_sigs {
                    self.validate_type_inner(method_sig, depth + 1)?;
                }
                let embeddeds = self.arguments_slice(input, data.embeddeds).to_vec();
                for emb in embeddeds {
                    self.validate_type_inner(emb, depth + 1)?;
                }
                Some(())
            }
            Ty::Union(slice) => {
                let term_tys: Vec<TypeId> = self
                    .terms_slice(input, slice)
                    .iter()
                    .map(|t| t.ty)
                    .collect();
                for ty in term_tys {
                    self.validate_type_inner(ty, depth + 1)?;
                }
                Some(())
            }
            Ty::Signature(_) => {
                let sig = self.signature_data(input)?.clone();
                let params = self.arg_range(input, sig.parameters).to_vec();
                let res = self.arg_range(input, sig.results).to_vec();
                for param in params {
                    self.validate_type_inner(param, depth + 1)?;
                }
                for result in res {
                    self.validate_type_inner(result, depth + 1)?;
                }
                Some(())
            }
            Ty::Tuple(elements) => {
                let elems = self.arguments_slice(input, elements).to_vec();
                for elem in elems {
                    self.validate_type_inner(elem, depth + 1)?;
                }
                Some(())
            }
            Ty::Basic(_) | Ty::TypeParam(_) => Some(()),
        }
    }

    pub(super) fn expand(&mut self, input: TypeId) -> Option<TypeId> {
        self.step()?;
        if let Some(target) = self.expansion_get(input) {
            return Some(target);
        }
        // Constraints may recursively ask for another declaration's underlying
        // type. Bound that call stack as well as the iterative alias chain.
        if self.expansion_depth >= MAX_DEPTH {
            return None;
        }
        self.expansion_depth += 1;
        let result = if input.is_decl() {
            self.with_decl_alloc(|store| store.expand_declaration(input))
        } else {
            self.with_local_alloc(|store| store.expand_declaration(input))
        };
        self.expansion_depth -= 1;
        result
    }

    fn expand_declaration(&mut self, input: TypeId) -> Option<TypeId> {
        let Some((declaration, arguments, _)) = self.named_info(input) else {
            return Some(input);
        };
        let (file, symbol) = (declaration.file, declaration.symbol);
        let arguments = self.arguments_slice(input, arguments).to_vec();
        // Read a bounded declaration slice, not the defining closed file.
        let end = symbol.declaration()?.end;
        let source = if end - symbol.end <= super::MAX_BYTES {
            self.workspace.source_slice(file, symbol.end..end)?
        } else {
            self.type_parameter_source(declaration)?
        };
        let (header, signature) = parse_type_parameters_of(&source, symbol.generic)?;
        if !signature.diagnostics.is_empty() {
            return None;
        }
        let parameters = constraints::parameter_names(header, &signature)?;
        if parameters.len() != arguments.len() || parameters.len() > super::MAX_NODES {
            return None;
        }
        self.check_named_arguments(input)?;
        let bindings: Vec<_> = arguments.iter().copied().map(Some).collect();
        // Check the declaration before concrete arguments can hide an illegal
        // embedded parameter (including one behind a generic pointer alias).
        let mut schema_params = Vec::with_capacity(parameters.len());
        let mut schema = Vec::with_capacity(parameters.len());
        for (index, name) in parameters.into_iter().enumerate() {
            let param = self.with_local_alloc(|store| store.push_type_param(index))?;
            schema_params.push(param);
            schema.push((name, param));
        }
        let (body, body_offset) = if end - symbol.end <= super::MAX_BYTES {
            let body = super::declaration_body(&source)?;
            (
                std::borrow::Cow::Borrowed(body),
                symbol.end + source.len() - body.len(),
            )
        } else {
            let offset = symbol
                .end
                .checked_add(symbol.type_info.as_ref()?.target.as_ref()?.body_start as usize)?;
            (
                std::borrow::Cow::Owned(self.workspace.source_slice(file, offset..end)?),
                offset,
            )
        };
        let result = self.parse_in_at(file, &body, body_offset, &schema)?;
        if (!self.mode.is_symbolic() || !symbol.alias)
            && matches!(self.node(result)?, Ty::TypeParam(_))
        {
            return None;
        }
        let result = if schema.is_empty() {
            result
        } else {
            self.substitute_for_params(result, &schema_params, &bindings, 0)?
        };
        if let Some(methods) = self.interface_data(result).map(|data| data.methods.clone()) {
            for method in self.methods_slice_mut(result, methods) {
                method.declared_on.get_or_insert(input);
            }
        }
        self.expansion_insert(input, result);
        Some(result)
    }

    pub(crate) fn underlying(&mut self, mut input: TypeId) -> Option<TypeId> {
        for _ in 0..MAX_DEPTH {
            if input == TypeId::INVALID {
                return None;
            }
            if self.local_decl_info(input).is_some() {
                // Local defined types expand through their own bounded fact table;
                // a package/defined RHS then keeps the existing lazy expansion.
                input = self.local_declaration_underlying(input)?;
                continue;
            }
            if let Some(interface) = self.error_interface(input) {
                return Some(interface);
            }
            if self.named_info(input).is_none() {
                return Some(input);
            }
            input = self.expand(input)?;
        }
        None
    }

    pub(crate) fn unalias(&mut self, mut input: TypeId) -> Option<TypeId> {
        for _ in 0..MAX_DEPTH {
            if input == TypeId::INVALID {
                return None;
            }
            // Local aliases are transparent too; a missing fact stays unknown.
            if let Some(expanded) = self.local_alias(input)? {
                input = expanded;
                continue;
            }
            if !matches!(self.node(input)?, Ty::Alias { .. }) {
                return Some(input);
            }
            input = self.expand(input)?;
        }
        None
    }

    pub(crate) fn named_declaration(
        &self,
        input: TypeId,
    ) -> Option<(crate::analysis::engine::obj::DeclId<'a>, Vec<TypeId>, bool)> {
        let (declaration, arguments, alias) = self.named_info(input)?;
        Some((
            declaration,
            self.arguments_slice(input, arguments).to_vec(),
            alias,
        ))
    }
}
