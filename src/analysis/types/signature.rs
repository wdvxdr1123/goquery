//! Function signatures use bounded shared type-ID lists, without names or bodies.
use super::constraints;
use super::{MAX_BYTES, MAX_DEPTH, MAX_NODES, Operand, Ty, TypeArena, TypeId};
use crate::syntax::Signature;
use std::path::Path;

impl TypeArena<'_> {
    /// Declaration-only generic functions use their true formal environment;
    /// calls use the instantiated checked signature instead.
    pub(crate) fn formal_function_signature(
        &mut self,
        declaration: crate::analysis::engine::obj::DeclId<'_>,
    ) -> Option<TypeId> {
        let span = declaration.symbol.declaration()?;
        let end = declaration
            .symbol
            .body()
            .map_or(span.end, |body| body.start);
        let header = self
            .workspace
            .source_slice(declaration.file, span.start..end)?;
        let signature = crate::syntax::parse_signature(&header);
        if !signature.diagnostics.is_empty()
            || !signature.receiver.is_empty()
            || signature.type_parameters.len() > self.active_node_limit()
        {
            return None;
        }
        let names = constraints::parameter_names(&header, &signature)?;
        let mut environment = Vec::new();
        for name in names {
            environment.push((name, self.push_symbolic_parameter(declaration.file, name)?));
        }
        let bounds: Vec<_> = signature
            .type_parameters
            .iter()
            .zip(&environment)
            .map(|(parameter, (_, ty))| (*ty, &header[parameter.annotation.clone()]))
            .collect();
        self.import_parameter_bounds(declaration.file, &bounds, &environment)?;
        self.signature(
            declaration.file,
            &header,
            Some(span.start),
            &signature,
            &environment,
            0,
        )
    }

    /// Receiver names are fresh formals; owner bounds use the declaring file's imports.
    pub(super) fn import_receiver_parameters(
        &mut self,
        origin: &Path,
        receiver: &str,
        environment: &[(&str, TypeId)],
    ) -> Option<TypeId> {
        let receiver = self.parse_in(origin, receiver, environment)?;
        let receiver = self.unalias(receiver)?;
        let base = match self.node(receiver)? {
            Ty::Pointer(base) => self.unalias(*base)?,
            _ => receiver,
        };
        let (origin_id, declaration, arguments, _) = self.named_origin_info(base)?;
        let (file, symbol) = (declaration.file, declaration.symbol);
        if symbol.alias || !symbol.generic || !self.same_field_package(file, origin)? {
            return None;
        }
        let arguments = self.arguments_slice(base, arguments).to_vec();
        if arguments.len() > self.active_node_limit()
            || arguments.iter().enumerate().any(|(index, ty)| {
                !matches!(self.node(*ty), Some(Ty::TypeParam(_))) || arguments[..index].contains(ty)
            })
        {
            return None;
        }
        // Reuse the cached DeclArena origin template instead of re-reading and
        // re-parsing the owner type's parameter list from disk.
        let owner_tparams = self.ensure_origin_tparams(origin_id, true)?;
        if owner_tparams.len() != arguments.len() {
            return None;
        }
        let bindings: Vec<Option<TypeId>> = arguments.iter().copied().map(Some).collect();
        let mut bound_pairs = Vec::with_capacity(arguments.len());
        for (&owner_tp, &recv_tp) in owner_tparams.iter().zip(&arguments) {
            if self.type_param_bound(recv_tp).is_some() {
                continue;
            }
            let owner_bound = self.type_param_bound(owner_tp)?;
            let inst_bound =
                self.substitute_for_params(owner_bound, &owner_tparams, &bindings, 0)?;
            if !self.set_type_param_bound(recv_tp, inst_bound) {
                return None;
            }
            bound_pairs.push((recv_tp, inst_bound));
        }
        for &(_, bound) in &bound_pairs {
            if !matches!(
                self.constraint_type_set_checked(bound, 0),
                super::CheckOutcome::Ok(_)
            ) {
                for &(tp, _) in &bound_pairs {
                    self.set_type_param_bound(tp, TypeId::INVALID);
                }
                return None;
            }
        }
        Some(base)
    }

    fn read_signature_parts(
        &mut self,
        origin: &Path,
        header: &str,
        file_offset: Option<usize>,
        signature: &Signature,
        environment: &[(&str, TypeId)],
        depth: usize,
    ) -> Option<(Vec<TypeId>, Vec<TypeId>, bool)> {
        if depth >= MAX_DEPTH || header.len() > MAX_BYTES || !signature.diagnostics.is_empty() {
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
                let mut ty = self
                    .parse_in_checked(
                        origin,
                        source,
                        file_offset.map(|base| base + item.annotation.start),
                        environment,
                    )
                    .ok()?;
                if item.variadic {
                    ty = self.push(Ty::Slice(ty))?;
                }
                output.push(ty);
            }
        }
        Some((parameters, results, variadic))
    }

    pub(crate) fn signature(
        &mut self,
        origin: &Path,
        header: &str,
        file_offset: Option<usize>,
        signature: &Signature,
        environment: &[(&str, TypeId)],
        depth: usize,
    ) -> Option<TypeId> {
        if signature.type_parameters.iter().any(|parameter| {
            parameter.name.as_ref().is_none_or(|name| {
                !environment
                    .iter()
                    .any(|(key, _)| *key == &header[name.clone()])
            })
        }) {
            return None;
        }
        let (parameters, results, variadic) =
            self.read_signature_parts(origin, header, file_offset, signature, environment, depth)?;
        let ty = self.store_signature(parameters, results, variadic)?;
        self.attach_signature_names(ty, header, signature);
        Some(ty)
    }

    /// Resolve a generic function signature directly into the active pool so
    /// `DeclCache` caches formal type parameters, bounds, and parts together.
    pub(crate) fn resolve_generic_signature(
        &mut self,
        origin: &Path,
        header: &str,
        file_offset: Option<usize>,
        signature: &Signature,
    ) -> Option<TypeId> {
        if !signature.diagnostics.is_empty()
            || !signature.receiver.is_empty()
            || signature.type_parameters.is_empty()
            || signature.type_parameters.len() > self.active_node_limit()
        {
            return None;
        }
        let names = constraints::parameter_names(header, signature)?;
        let mut type_params = Vec::with_capacity(names.len());
        let mut environment = Vec::with_capacity(names.len());
        for (index, name) in names.into_iter().enumerate() {
            let param = self.push_type_param(index)?;
            type_params.push(param);
            environment.push((name, param));
        }
        let bounds: Vec<_> = signature
            .type_parameters
            .iter()
            .zip(&type_params)
            .map(|(parameter, &param)| (param, &header[parameter.annotation.clone()]))
            .collect();
        self.import_parameter_bounds(origin, &bounds, &environment)?;
        let (parameters, results, variadic) =
            self.read_signature_parts(origin, header, file_offset, signature, &environment, 0)?;
        let ty = self.store_generic_signature(type_params, parameters, results, variadic)?;
        self.attach_signature_names(ty, header, signature);
        Some(ty)
    }

    pub(crate) fn store_signature(
        &mut self,
        parameters: Vec<TypeId>,
        results: Vec<TypeId>,
        variadic: bool,
    ) -> Option<TypeId> {
        self.store_signature_nodes(Vec::new(), parameters, results, variadic)
    }

    pub(crate) fn store_generic_signature(
        &mut self,
        type_params: Vec<TypeId>,
        parameters: Vec<TypeId>,
        results: Vec<TypeId>,
        variadic: bool,
    ) -> Option<TypeId> {
        self.store_signature_nodes(type_params, parameters, results, variadic)
    }

    pub(crate) fn signature_type_param_count(&self, ty: TypeId) -> Option<usize> {
        self.signature_data(ty).map(|sig| sig.type_params.len())
    }

    pub(super) fn instantiate_signature_nodes(
        &mut self,
        generic_ty: TypeId,
        type_params: &[TypeId],
        bindings: &[Option<TypeId>],
    ) -> Option<TypeId> {
        let sig = self.signature_data(generic_ty)?.clone();
        let parameters = self.arg_range(generic_ty, sig.parameters).to_vec();
        let results = self.arg_range(generic_ty, sig.results).to_vec();
        let mut new_params = Vec::with_capacity(parameters.len());
        for param in parameters {
            new_params.push(self.substitute_for_params(param, type_params, bindings, 0)?);
        }
        let mut new_results = Vec::with_capacity(results.len());
        for result in results {
            new_results.push(self.substitute_for_params(result, type_params, bindings, 0)?);
        }
        let inst_ty = self.store_signature(new_params, new_results, sig.variadic)?;
        self.copy_signature_names(generic_ty, inst_ty, false);
        Some(inst_ty)
    }

    pub(crate) fn instantiate_generic_signature(
        &mut self,
        generic_ty: TypeId,
        arguments: &[TypeId],
    ) -> Option<TypeId> {
        let sig = self.signature_data(generic_ty)?.clone();
        let type_params = self.arguments_slice(generic_ty, sig.type_params).to_vec();
        if type_params.len() != arguments.len() {
            return None;
        }
        let bounds: Vec<TypeId> = type_params
            .iter()
            .map(|&tp| self.type_param_bound(tp))
            .collect::<Option<_>>()?;
        let bindings: Vec<_> = arguments.iter().copied().map(Some).collect();
        self.prove_constraints(&type_params, &bounds, &bindings)?;
        self.instantiate_signature_nodes(generic_ty, &type_params, &bindings)
    }

    pub(crate) fn is_callable(&mut self, input: TypeId) -> Option<bool> {
        let input = self.underlying(input)?;
        Some(matches!(self.node(input)?, Ty::Signature(_)))
    }

    /// Last fixed parameter of a variadic callable, as used by gopls for
    /// format-literal tokens and argument highlights. No naming convention is
    /// involved and the actual argument need not be assignable to prove arity.
    pub(crate) fn format_parameter(&self, input: TypeId) -> Option<usize> {
        // The reference requires a signature itself, rather than the
        // underlying signature of a defined function type.
        let sig = self.signature_data(input)?;
        sig.variadic
            .then(|| sig.parameters.len().checked_sub(2))
            .flatten()
    }

    /// Validate the arguments once, then borrow the result types. An empty
    /// result slice is a valid call with no value.
    pub(crate) fn call_results(
        &mut self,
        callee: TypeId,
        actuals: &[Operand],
        spread: bool,
        count: usize,
        int_bits: u32,
    ) -> Option<&[TypeId]> {
        let callee = self.underlying(callee)?;
        let sig = self.signature_data(callee)?.clone();
        let parameters = sig.parameters;
        let results = sig.results;
        let variadic = sig.variadic;
        if results.len() != count || actuals.len() > MAX_NODES {
            return None;
        }
        let fixed = parameters.len().checked_sub(usize::from(variadic))?;
        if (!variadic && (spread || actuals.len() != fixed))
            || (variadic && (actuals.len() < fixed || spread && actuals.len() != fixed + 1))
        {
            return None;
        }
        for (index, actual) in actuals.iter().enumerate() {
            let mut target = *self
                .arg_range(callee, parameters.clone())
                .get(index.min(fixed))?;
            if variadic && index >= fixed && !spread {
                let Ty::Slice(element) = *self.node(target)? else {
                    return None;
                };
                target = element;
            }
            self.assignable(*actual, target, int_bits)?.then_some(())?;
        }
        self.pool_for(callee).arguments.get(results)
    }

    pub(crate) fn signature_parts(
        &mut self,
        input: TypeId,
    ) -> Option<(Vec<TypeId>, Vec<TypeId>, bool)> {
        let input = self.underlying(input)?;
        let sig = self.signature_data(input)?.clone();
        Some((
            self.arg_range(input, sig.parameters).to_vec(),
            self.arg_range(input, sig.results).to_vec(),
            sig.variadic,
        ))
    }
}
