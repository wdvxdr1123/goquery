//! Bounded field/method lookup shares visibility, promotion and ambiguity checks.
use super::{DeclRef, MAX_DEPTH, MAX_NODES, Method, Mode, Operand, Ty, TypeArena, TypeId};
use crate::analysis::scope as locals;
use std::path::Path;

#[derive(Clone, Copy)]
enum Member<'a> {
    Method {
        signature: TypeId,
        declaration: Option<DeclRef<'a>>,
    },
    Field {
        ty: TypeId,
        indirect: bool,
    },
}

struct Step<'a> {
    ty: TypeId,
    indirect: bool,
    parent: Option<usize>,
    declaration: Option<DeclRef<'a>>,
}

impl<'a> TypeArena<'a> {
    /// Resolve a value selector once, sharing the field/method breadth-first
    /// walk and retaining addressability. None means unrepresented or exhausted;
    /// Some(None) is a checked missing/ineligible/ambiguous selector.
    pub(crate) fn selector_operand(
        &mut self,
        input: Operand,
        origin: &Path,
        name: &str,
    ) -> Option<Option<Operand>> {
        if name == "_" {
            return Some(None);
        }
        let mut member = self.lookup_member(input.ty, origin, name)?;
        if member.is_none() && input.mode == Mode::Variable {
            let pointer = self.pointer(input.ty)?;
            member = self.lookup_member(pointer, origin, name)?;
        }
        let operand = match member {
            Some(Member::Method { signature, .. }) => Some(Operand {
                mode: Mode::Value,
                ty: signature,
                value: None,
            }),
            Some(Member::Field { ty, indirect }) => Some(Operand {
                mode: if indirect || input.mode == Mode::Variable {
                    Mode::Variable
                } else {
                    Mode::Value
                },
                ty,
                value: None,
            }),
            None => {
                // Defined pointers permit fields, but must not acquire methods
                // from their underlying pointer or from selector auto-addressing.
                let root = self.unalias(input.ty)?;
                if matches!(self.nodes[usize::from(root.0)], Ty::Named { .. })
                    && self
                        .underlying(root)
                        .is_some_and(|ty| matches!(self.nodes[usize::from(ty.0)], Ty::Pointer(_)))
                {
                    self.field_operand(input, origin, name)
                } else {
                    None
                }
            }
        };
        Some(operand)
    }

    pub(crate) fn method_declaration(
        &mut self,
        receiver: TypeId,
        origin: &Path,
        name: &str,
    ) -> Option<(&'a Path, &'a crate::index::IndexedSymbol)> {
        let Member::Method {
            declaration: Some(declaration),
            ..
        } = self.lookup_member(receiver, origin, name)??
        else {
            return None;
        };
        Some((declaration.file, declaration.symbol))
    }
    /// Method expressions use the exact written receiver, including promotion;
    /// they do not get selector auto-addressing or the embedded owner's type.
    pub(crate) fn method_expression(
        &mut self,
        receiver: TypeId,
        origin: &Path,
        name: &str,
    ) -> Option<Operand> {
        if name == "_" {
            return None;
        }
        let Member::Method { signature, .. } = self.lookup_member(receiver, origin, name)?? else {
            return None;
        };
        let Ty::Signature {
            parameters,
            results,
            variadic,
        } = self.nodes[usize::from(signature.0)].clone()
        else {
            return None;
        };
        let mut arguments = vec![receiver];
        arguments.extend_from_slice(&self.arguments[parameters]);
        let results = self.arguments[results].to_vec();
        let ty = self.store_signature(arguments, results, variadic)?;
        Some(Operand {
            mode: Mode::Value,
            ty,
            value: None,
        })
    }

    pub(super) fn lookup_method(
        &mut self,
        source: TypeId,
        required: &Method<'_>,
    ) -> Option<Option<TypeId>> {
        Some(
            match self.lookup_member(source, required.origin, &required.name)? {
                Some(Member::Method { signature, .. }) => Some(signature),
                _ => None,
            },
        )
    }

    pub(crate) fn field_operand(
        &mut self,
        input: Operand,
        origin: &Path,
        name: &str,
    ) -> Option<Operand> {
        if name == "_" {
            return None;
        }
        let mut source = self.unalias(input.ty)?;
        // Go permits field selectors through a defined pointer, but that does
        // not add methods to its method set.
        if matches!(self.nodes[usize::from(source.0)], Ty::Named { .. }) {
            let root = self.underlying(source)?;
            if matches!(self.nodes[usize::from(root.0)], Ty::Pointer(_)) {
                source = root;
            }
        }
        let Member::Field { ty, indirect } = self.lookup_member(source, origin, name)?? else {
            return None;
        };
        Some(Operand {
            mode: if indirect || input.mode == Mode::Variable {
                Mode::Variable
            } else {
                Mode::Value
            },
            ty,
            value: None,
        })
    }

    fn lookup_member(
        &mut self,
        source: TypeId,
        origin: &Path,
        name: &str,
    ) -> Option<Option<Member<'a>>> {
        let mut steps = vec![Step {
            ty: source,
            indirect: false,
            parent: None,
            declaration: None,
        }];
        let mut level = vec![0];
        for _ in 0..MAX_DEPTH {
            let mut next = Vec::new();
            let mut count = 0;
            let mut found = None;
            for index in level {
                self.step()?;
                let mut input = self.unalias(steps[index].ty)?;
                let pointer = if let Ty::Pointer(element) = self.nodes[usize::from(input.0)] {
                    input = self.unalias(element)?;
                    true
                } else {
                    false
                };
                let indirect = steps[index].indirect || pointer;
                if matches!(self.nodes[usize::from(input.0)], Ty::Pointer(_)) {
                    continue;
                }
                if self.interface_constructor(input)? {
                    if pointer {
                        continue;
                    }
                    let root = self.underlying(input)?;
                    let Ty::Interface { methods, .. } = self.nodes[usize::from(root.0)].clone()
                    else {
                        return None;
                    };
                    for method in &self.methods[methods] {
                        if method.name == name
                            && (crate::syntax::is_exported_name(name)
                                || self.same_field_package(method.origin, origin)?)
                        {
                            count += 1;
                            found = Some(Member::Method {
                                signature: method.signature,
                                declaration: None,
                            });
                            break;
                        }
                    }
                    continue;
                }
                let previous_count = count;
                if let Ty::Named {
                    declaration,
                    arguments,
                } = self.nodes[usize::from(input.0)].clone()
                {
                    self.check_named_arguments(input)?;
                    let mut ancestor = steps[index].parent;
                    let mut cycle = false;
                    while let Some(parent) = ancestor {
                        if steps[parent].declaration.is_some_and(|previous| {
                            previous.file == declaration.file
                                && previous.symbol.start == declaration.symbol.start
                        }) {
                            cycle = true;
                            break;
                        }
                        ancestor = steps[parent].parent;
                    }
                    if cycle {
                        continue;
                    }
                    steps[index].declaration = Some(declaration);
                    let arguments = self.arguments[arguments].to_vec();
                    let workspace = self.workspace;
                    let probe = crate::bloom::Fingerprint::new(name.as_bytes());
                    for (file, method) in
                        workspace.package_method_candidates(declaration.file, probe)
                    {
                        if !method.is_method()
                            || method.name != name
                            || !crate::syntax::is_exported_name(&method.name)
                                && !self.same_field_package(file, origin)?
                        {
                            continue;
                        }
                        self.step()?;
                        let receiver = method.receiver.as_deref()?;
                        if receiver != declaration.symbol.name {
                            if let Some((file, symbol)) = workspace.type_declaration(file, receiver)
                                && !symbol.alias
                                && (file.as_path() != declaration.file
                                    || symbol.start != declaration.symbol.start)
                            {
                                continue;
                            }
                            let receiver = self.parse(file, receiver)?;
                            let mut receiver = self.unalias(receiver)?;
                            if let Ty::Pointer(element) = self.nodes[usize::from(receiver.0)] {
                                receiver = self.unalias(element)?;
                            }
                            let Ty::Named {
                                declaration: candidate,
                                ..
                            } = self.nodes[usize::from(receiver.0)]
                            else {
                                continue;
                            };
                            if candidate.file != declaration.file
                                || candidate.symbol.start != declaration.symbol.start
                            {
                                continue;
                            }
                        }
                        let span = method.declaration()?;
                        let end = method.body().map_or(span.end, |body| body.start);
                        let header = workspace.source_slice(file, span.start..end)?;
                        let parsed = crate::syntax::parse_signature(&header);
                        if !parsed.diagnostics.is_empty() || !parsed.type_parameters.is_empty() {
                            return None;
                        }
                        let receiver = parsed.receiver.first()?;
                        let names = locals::receiver_parameters(&header, &receiver.annotation);
                        if names.len() != arguments.len() {
                            return None;
                        }
                        let environment: Vec<_> = names
                            .iter()
                            .zip(&arguments)
                            .map(|(name, ty)| (&header[name.clone()], *ty))
                            .collect();
                        let receiver = self.parse_in(
                            file,
                            &header[receiver.annotation.clone()],
                            &environment,
                        )?;
                        let receiver = self.unalias(receiver)?;
                        let pointer_receiver =
                            matches!(self.nodes[usize::from(receiver.0)], Ty::Pointer(_));
                        count += 1;
                        if !pointer_receiver || indirect {
                            found = Some(Member::Method {
                                signature: self.signature(
                                    file,
                                    &header,
                                    &parsed,
                                    &environment,
                                    0,
                                )?,
                                declaration: Some(DeclRef {
                                    file,
                                    symbol: method,
                                }),
                            });
                        }
                    }
                }
                // A declaration's own matching method hides all deeper methods;
                // only expand a concrete body if promotion is still needed.
                if count > previous_count {
                    continue;
                }
                let root = self.underlying(input)?;
                let Ty::Struct {
                    origin: field_origin,
                    fields,
                } = self.nodes[usize::from(root.0)].clone()
                else {
                    continue;
                };
                for field_index in fields {
                    let field = self.fields[field_index].clone();
                    self.step()?;
                    if field.name == name
                        && (crate::syntax::is_exported_name(&field.name)
                            || self.same_field_package(field_origin, origin)?)
                    {
                        count += 1;
                        found = Some(Member::Field {
                            ty: field.ty,
                            indirect,
                        });
                    }
                    if field.embedded {
                        if steps.len() >= MAX_NODES {
                            return None;
                        }
                        next.push(steps.len());
                        steps.push(Step {
                            ty: field.ty,
                            indirect,
                            parent: Some(index),
                            declaration: None,
                        });
                    }
                }
            }
            if count > 0 {
                return Some(if count == 1 { found } else { None });
            }
            if next.is_empty() {
                return Some(None);
            }
            level = next;
        }
        None
    }
}
