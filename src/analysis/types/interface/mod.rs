//! Complete unified interfaces (methods, embedded interfaces, unions, type terms).
mod lower;

use super::{BasicKind, CheckOutcome, MAX_DEPTH, Operand, Ty, TypeArena, TypeId, UnknownReason};
use crate::syntax::{Lexer, TokenKind};
use std::collections::BTreeMap;
use std::ops::Range;
use std::path::Path;

#[derive(Clone)]
pub(in crate::analysis::engine::types) struct Method<'a> {
    pub(in crate::analysis::engine::types) name: String,
    pub(in crate::analysis::engine::types) origin: &'a Path,
    pub(in crate::analysis::engine::types) signature: TypeId,
    /// Declaring interface instance for display after method-set flattening.
    /// This provenance does not participate in type identity.
    pub(in crate::analysis::engine::types) declared_on: Option<TypeId>,
    pub(in crate::analysis::engine::types) selection: [u32; 2],
    pub(in crate::analysis::engine::types) builtin: bool,
}

impl<'a> TypeArena<'a> {
    pub(in crate::analysis::engine::types) fn builtin_comparable_interface(
        &mut self,
        origin: &Path,
    ) -> Option<TypeId> {
        let origin = self.workspace.files.get_key_value(origin)?.0.as_path();
        self.store_interface_nodes(origin, Vec::new(), Vec::new(), true, true, false, false)
    }

    pub(in crate::analysis::engine::types) fn builtin_interface(
        &mut self,
        origin: &Path,
        error: bool,
    ) -> Option<TypeId> {
        let origin = self.workspace.files.get_key_value(origin)?.0.as_path();
        let methods = if error {
            let result = self.basic(BasicKind::String)?;
            let signature = self.store_signature(Vec::new(), vec![result], false)?;
            vec![Method {
                name: "Error".to_owned(),
                origin,
                signature,
                declared_on: None,
                selection: [0, 0],
                builtin: true,
            }]
        } else {
            Vec::new()
        };
        let interface =
            self.store_interface_nodes(origin, methods, Vec::new(), false, false, false, !error)?;
        if error {
            self.push_builtin_error(interface)
        } else {
            Some(interface)
        }
    }

    pub(in crate::analysis::engine::types) fn store_interface(
        &mut self,
        origin: &'a Path,
        methods: Vec<Method<'a>>,
    ) -> Option<TypeId> {
        self.store_interface_checked(origin, methods).ok()
    }

    pub(in crate::analysis::engine::types) fn store_interface_checked(
        &mut self,
        origin: &'a Path,
        methods: Vec<Method<'a>>,
    ) -> CheckOutcome<TypeId> {
        self.store_unified_interface_checked(origin, methods, Vec::new(), false, false, false)
    }

    pub(in crate::analysis::engine::types) fn store_unified_interface_checked(
        &mut self,
        origin: &'a Path,
        methods: Vec<Method<'a>>,
        embeddeds: Vec<TypeId>,
        has_type_elements: bool,
        comparable: bool,
        implicit: bool,
    ) -> CheckOutcome<TypeId> {
        let mut sorted: BTreeMap<(String, String), Method<'a>> = BTreeMap::new();
        for method in methods {
            let package = if crate::syntax::is_exported_name(&method.name) {
                String::new()
            } else if let Some(path) = self.workspace.import_path(method.origin) {
                path
            } else if self.mode.is_symbolic() {
                let Some(parent) = method.origin.parent().and_then(Path::to_str) else {
                    return CheckOutcome::Unknown(UnknownReason::MissingMetadata);
                };
                let Some(stub) = self.workspace.files.get(method.origin) else {
                    return CheckOutcome::Unknown(UnknownReason::MissingMetadata);
                };
                format!("{}:{}", parent, stub.package)
            } else {
                return CheckOutcome::Unknown(UnknownReason::MissingMetadata);
            };
            let key = (method.name.clone(), package);
            if let Some(previous) = sorted.get(&key) {
                let Some(same) = self.identical(previous.signature, method.signature) else {
                    return CheckOutcome::Unknown(UnknownReason::BudgetExceeded);
                };
                if !same {
                    return CheckOutcome::Invalid(());
                }
            } else {
                sorted.insert(key, method);
            }
        }
        match self.store_interface_nodes(
            origin,
            sorted.into_values().collect(),
            embeddeds,
            has_type_elements,
            comparable,
            implicit,
            false,
        ) {
            Some(id) => CheckOutcome::Ok(id),
            None => CheckOutcome::Unknown(UnknownReason::BudgetExceeded),
        }
    }

    pub(in crate::analysis::engine::types) fn same_interface_methods(
        &self,
        left: TypeId,
        a: Range<usize>,
        right: TypeId,
        b: Range<usize>,
    ) -> Option<bool> {
        if a.len() != b.len() {
            return Some(false);
        }
        for (a, b) in self
            .methods_slice(left, a)
            .iter()
            .zip(self.methods_slice(right, b))
        {
            if a.name != b.name
                || !crate::syntax::is_exported_name(&a.name)
                    && !self.same_field_package(a.origin, b.origin)?
            {
                return Some(false);
            }
        }
        Some(true)
    }

    pub(crate) fn is_interface(&mut self, input: TypeId) -> Option<bool> {
        let input = self.underlying(input)?;
        Some(matches!(self.node(input)?, Ty::Interface(_)))
    }

    /// Inference must not instantiate constrained named formals before their
    /// arguments have been inferred. Constructor metadata is sufficient here;
    /// assignment checks the concrete instantiated type afterward.
    pub(in crate::analysis::engine::types) fn interface_constructor(
        &mut self,
        input: TypeId,
    ) -> Option<bool> {
        if let Some((declaration, _, _)) = self.named_info(input) {
            let (mut file, mut symbol) = (declaration.file, declaration.symbol);
            for _ in 0..MAX_DEPTH {
                let Some(info) = &symbol.type_info else {
                    return Some(false);
                };
                if info.is_interface() {
                    return Some(true);
                }
                let Some(target) = &info.target else {
                    return Some(false);
                };
                if Lexer::new(&target.name).next()?.kind != TokenKind::Ident {
                    return Some(false);
                }
                let Some((next_file, next_symbol)) =
                    self.resolver().resolve_type_declaration(file, &target.name)
                else {
                    return Some(matches!(target.name.as_str(), "any" | "error"));
                };
                (file, symbol) = (next_file, next_symbol);
            }
            return None;
        }
        self.is_interface(input)
    }

    pub(in crate::analysis::engine::types) fn assign_interface(
        &mut self,
        source: Operand,
        target: TypeId,
        int_bits: u32,
    ) -> Option<bool> {
        if self.basic_kind(source.ty) == Some(BasicKind::UntypedNil) {
            return Some(true);
        }
        let source = self.default_operand(source, int_bits)?;
        let target_root = self.underlying(target)?;
        let data = self.interface_data(target_root)?;
        if data.has_type_elements || data.comparable {
            return None;
        }
        let required = data.methods.clone();
        if required.is_empty() {
            return Some(true);
        }
        let methods = self.methods_slice(target_root, required).to_vec();
        for method in methods {
            let Some(signature) = self.lookup_method(source, &method)? else {
                return Some(false);
            };
            if !self.identical(method.signature, signature)? {
                return Some(false);
            }
        }
        Some(true)
    }

    /// A concrete asserted type must implement the source interface. An
    /// interface target follows go/types assertableTo even when methods
    /// conflict. The compiler's stricter impossible-interface diagnostics are
    /// not modeled here. Do not apply selector auto-addressing.
    pub(crate) fn assertion(
        &mut self,
        source: Operand,
        target: TypeId,
        int_bits: u32,
    ) -> Option<Operand> {
        self.step()?;
        if !self.is_interface(source.ty)? {
            return None;
        }
        if !self.is_interface(target)?
            && !self.assign_interface(
                Operand {
                    mode: super::Mode::Value,
                    ty: target,
                    value: None,
                },
                source.ty,
                int_bits,
            )?
        {
            return None;
        }
        Some(Operand {
            mode: super::Mode::CommaOk,
            ty: target,
            value: None,
        })
    }

    /// Case variables narrow only for exactly one non-nil type. Duplicate
    /// aliases/structural identities and nil across the entire switch, as well
    /// as repeated defaults, are invalid. Only the selected clause projects.
    pub(crate) fn type_switch(
        &mut self,
        source: Operand,
        targets: &[Option<TypeId>],
        selected: Range<usize>,
        defaults: usize,
        int_bits: u32,
    ) -> Option<Operand> {
        self.step()?;
        if targets.len() > self.active_node_limit()
            || defaults > 1
            || !self.is_interface(source.ty)?
        {
            return None;
        }
        for (index, target) in targets.iter().enumerate() {
            for previous in &targets[..index] {
                self.step()?;
                if match (target, previous) {
                    (Some(a), Some(b)) => self.identical(*a, *b)?,
                    (None, None) => true,
                    _ => false,
                } {
                    return None;
                }
            }
            if let Some(target) = target {
                self.assertion(source, *target, int_bits)?;
            }
        }
        Some(Operand {
            mode: super::Mode::Variable,
            ty: if let [Some(target)] = targets.get(selected)? {
                *target
            } else {
                source.ty
            },
            value: None,
        })
    }
}
