//! Lazy named-type expansion and identity; all state dies with the type arena.
use super::constraints;
use super::{MAX_DEPTH, MAX_NODES, Tags, Ty, TypeArena, TypeId};
use crate::analysis::types::declaration_body;

impl TypeArena<'_> {
    /// Test-only structural projection; no JSON machinery enters runtime queries.
    #[cfg(test)]
    pub(crate) fn test_identity(&mut self, input: TypeId) -> Option<serde_json::Value> {
        self.identity_projection(input, 0)
    }

    #[cfg(test)]
    fn identity_projection(&mut self, input: TypeId, depth: usize) -> Option<serde_json::Value> {
        use serde_json::json;
        self.step()?;
        if depth >= MAX_DEPTH {
            return None;
        }
        let input = self.unalias(input)?;
        Some(match self.nodes.get(usize::from(input.0))? {
            Ty::Parameter(_) => {
                let (origin, selection, _) = self.local_parameter(input)?;
                json!(["parameter", origin, selection])
            }
            Ty::LocalDeclaration { .. } => return self.local_declaration_identity(input),
            Ty::Error(_) => json!(["universe", "error"]),
            Ty::Interface { methods, .. } => {
                let methods = self.methods[methods.clone()].to_vec();
                let mut values = Vec::new();
                for method in methods {
                    let package = if crate::syntax::is_exported_name(&method.name) {
                        String::new()
                    } else {
                        self.workspace.import_path(method.origin)?
                    };
                    values.push(json!([
                        method.name,
                        package,
                        self.identity_projection(method.signature, depth + 1)?
                    ]));
                }
                json!(["interface", values])
            }
            Ty::Signature {
                parameters,
                results,
                variadic,
            } => {
                let (parameters, results, variadic) = (
                    self.arguments[parameters.clone()].to_vec(),
                    self.arguments[results.clone()].to_vec(),
                    *variadic,
                );
                let parameters: Vec<_> = parameters
                    .into_iter()
                    .map(|ty| self.identity_projection(ty, depth + 1))
                    .collect::<Option<_>>()?;
                let results: Vec<_> = results
                    .into_iter()
                    .map(|ty| self.identity_projection(ty, depth + 1))
                    .collect::<Option<_>>()?;
                json!(["signature", variadic, parameters, results])
            }
            Ty::Struct { origin, fields } => {
                let origin = *origin;
                let fields = self.fields[fields.clone()].to_vec();
                let mut values = Vec::new();
                for field in fields {
                    let package = if crate::syntax::is_exported_name(&field.name) {
                        String::new()
                    } else {
                        self.workspace.import_path(origin)?
                    };
                    values.push(json!([
                        field.name,
                        package,
                        field.embedded,
                        field.tag,
                        self.identity_projection(field.ty, depth + 1)?
                    ]));
                }
                json!(["struct", values])
            }
            Ty::Basic(kind) => json!(["basic", kind.canonical().name()]),
            Ty::Named {
                declaration,
                arguments,
            } => {
                let package = self.workspace.import_path(declaration.file)?;
                let name = declaration.symbol.name.clone();
                let arguments = self.arguments[arguments.clone()].to_vec();
                let arguments: Vec<_> = arguments
                    .into_iter()
                    .map(|argument| self.identity_projection(argument, depth + 1))
                    .collect::<Option<_>>()?;
                json!(["named", package, name, arguments])
            }
            Ty::Pointer(element) | Ty::Slice(element) => {
                let (tag, element) = (
                    if matches!(self.nodes[usize::from(input.0)], Ty::Pointer(_)) {
                        "pointer"
                    } else {
                        "slice"
                    },
                    *element,
                );
                json!([tag, self.identity_projection(element, depth + 1)?])
            }
            Ty::Array { length, element } => {
                let (length, element) = (*length, *element);
                json!([
                    "array",
                    length,
                    self.identity_projection(element, depth + 1)?
                ])
            }
            Ty::Map { key, value } => {
                let (key, value) = (*key, *value);
                json!([
                    "map",
                    self.identity_projection(key, depth + 1)?,
                    self.identity_projection(value, depth + 1)?
                ])
            }
            Ty::Chan { direction, element } => {
                let (direction, element) = (
                    match direction {
                        super::Direction::Both => "both",
                        super::Direction::Send => "send",
                        super::Direction::Receive => "receive",
                    },
                    *element,
                );
                json!([
                    "channel",
                    direction,
                    self.identity_projection(element, depth + 1)?
                ])
            }
        })
    }

    pub(super) fn step(&mut self) -> Option<()> {
        self.relation_steps = self.relation_steps.checked_sub(1)?;
        Some(())
    }

    pub(super) fn expand(&mut self, input: TypeId) -> Option<TypeId> {
        self.step()?;
        if let Some((_, target)) = self.expansions.iter().find(|(id, _)| *id == input) {
            return Some(*target);
        }
        // Constraints may recursively ask for another declaration's underlying
        // type. Bound that call stack as well as the iterative alias chain.
        if self.expansion_depth >= MAX_DEPTH {
            return None;
        }
        self.expansion_depth += 1;
        let result = self.expand_declaration(input);
        self.expansion_depth -= 1;
        result
    }

    fn expand_declaration(&mut self, input: TypeId) -> Option<TypeId> {
        let Ty::Named {
            declaration,
            arguments,
        } = self.nodes.get(usize::from(input.0))?
        else {
            return Some(input);
        };
        let (file, symbol) = (declaration.file, declaration.symbol);
        let arguments = self.arguments[arguments.clone()].to_vec();
        // Read a bounded declaration slice, not the defining closed file.
        let source = self
            .workspace
            .source_slice(file, symbol.end..symbol.declaration()?.end)?;
        let header = super::named::parameter_header(&source, symbol.generic)?;
        let signature = crate::syntax::parse_signature(&header);
        if !signature.diagnostics.is_empty() {
            return None;
        }
        let parameters = constraints::parameter_names(&header, &signature)?;
        if parameters.len() != arguments.len() || parameters.len() > MAX_NODES {
            return None;
        }
        self.check_named_arguments(input)?;
        let bindings: Vec<_> = arguments.iter().copied().map(Some).collect();
        // Check the declaration before concrete arguments can hide an illegal
        // embedded parameter (including one behind a generic pointer alias).
        let mut schema = Vec::new();
        for (index, name) in parameters.into_iter().enumerate() {
            schema.push((name, self.push(Ty::Parameter(index))?));
        }
        let result = self.parse_in(file, declaration_body(&source)?, &schema)?;
        if matches!(self.nodes[usize::from(result.0)], Ty::Parameter(_)) {
            return None;
        }
        let result = if schema.is_empty() {
            result
        } else {
            self.substitute(result, &bindings, 0)?
        };
        self.expansions.push((input, result));
        Some(result)
    }

    pub(super) fn underlying(&mut self, mut input: TypeId) -> Option<TypeId> {
        for _ in 0..MAX_DEPTH {
            if matches!(
                self.nodes.get(usize::from(input.0))?,
                Ty::LocalDeclaration { .. }
            ) {
                // Local defined types expand through their own bounded fact table;
                // a package/defined RHS then keeps the existing lazy expansion.
                input = self.local_declaration_underlying(input)?;
                continue;
            }
            if let Ty::Error(interface) = self.nodes[usize::from(input.0)] {
                return Some(interface);
            }
            if !matches!(self.nodes.get(usize::from(input.0))?, Ty::Named { .. }) {
                return Some(input);
            }
            input = self.expand(input)?;
        }
        None
    }

    pub(super) fn unalias(&mut self, mut input: TypeId) -> Option<TypeId> {
        for _ in 0..MAX_DEPTH {
            // Local aliases are transparent too; a missing fact stays unknown.
            if let Some(expanded) = self.local_alias(input)? {
                input = expanded;
                continue;
            }
            if !matches!(self.nodes.get(usize::from(input.0))?, Ty::Named { declaration, .. } if declaration.symbol.alias)
            {
                return Some(input);
            }
            input = self.expand(input)?;
        }
        None
    }

    pub(crate) fn identical(&mut self, left: TypeId, right: TypeId) -> Option<bool> {
        self.identical_with_tags(left, right, Tags::Include)
    }

    pub(super) fn identical_with_tags(
        &mut self,
        left: TypeId,
        right: TypeId,
        tags: Tags,
    ) -> Option<bool> {
        let mut pending = vec![(left, right, 0usize)];
        while let Some((left, right, depth)) = pending.pop() {
            self.step()?;
            if depth >= MAX_DEPTH {
                return None;
            }
            let (left, right) = (self.unalias(left)?, self.unalias(right)?);
            let pairs = match (
                &self.nodes[usize::from(left.0)],
                &self.nodes[usize::from(right.0)],
            ) {
                (
                    Ty::LocalDeclaration {
                        origin: a,
                        selection: aa,
                        ..
                    },
                    Ty::LocalDeclaration {
                        origin: b,
                        selection: bb,
                        ..
                    },
                ) if a == b && aa == bb => Vec::new(),
                (Ty::LocalDeclaration { .. }, _) | (_, Ty::LocalDeclaration { .. }) => return None,
                (Ty::Basic(a), Ty::Basic(b)) => {
                    if a.canonical() != b.canonical() {
                        return Some(false);
                    }
                    Vec::new()
                }
                (Ty::Parameter(_), Ty::Parameter(_)) => {
                    if left != right {
                        return Some(false);
                    }
                    Vec::new()
                }
                (
                    Ty::Named {
                        declaration: a,
                        arguments: aa,
                    },
                    Ty::Named {
                        declaration: b,
                        arguments: ba,
                    },
                ) => {
                    if a.file != b.file || a.symbol.start != b.symbol.start || aa.len() != ba.len()
                    {
                        return Some(false);
                    }
                    // A generic declaration used as a value type must be instantiated.
                    if a.symbol.generic && aa.is_empty() {
                        return None;
                    }
                    self.arguments[aa.clone()]
                        .iter()
                        .copied()
                        .zip(self.arguments[ba.clone()].iter().copied())
                        .collect()
                }
                (Ty::Pointer(a), Ty::Pointer(b)) | (Ty::Slice(a), Ty::Slice(b)) => vec![(*a, *b)],
                (Ty::Error(_), Ty::Error(_)) => Vec::new(),
                (Ty::Interface { methods: a, .. }, Ty::Interface { methods: b, .. }) => {
                    if !self.same_interface_methods(a.clone(), b.clone())? {
                        return Some(false);
                    }
                    self.methods[a.clone()]
                        .iter()
                        .map(|m| m.signature)
                        .zip(self.methods[b.clone()].iter().map(|m| m.signature))
                        .collect()
                }
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
                ) => {
                    if av != bv || a.len() != b.len() || ar.len() != br.len() {
                        return Some(false);
                    }
                    self.arguments[a.clone()]
                        .iter()
                        .chain(&self.arguments[ar.clone()])
                        .copied()
                        .zip(
                            self.arguments[b.clone()]
                                .iter()
                                .chain(&self.arguments[br.clone()])
                                .copied(),
                        )
                        .collect()
                }
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
                    if !self.struct_fields_with_tags(a, aa.clone(), b, ba.clone(), tags)? {
                        return Some(false);
                    }
                    self.fields[aa.clone()]
                        .iter()
                        .map(|f| f.ty)
                        .zip(self.fields[ba.clone()].iter().map(|f| f.ty))
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
                ) if a == b => vec![(*ae, *be)],
                (Ty::Map { key: a, value: av }, Ty::Map { key: b, value: bv }) => {
                    vec![(*a, *b), (*av, *bv)]
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
                ) if a == b => vec![(*ae, *be)],
                _ => return Some(false),
            };
            if pending.len() + pairs.len() > MAX_NODES {
                return None;
            }
            pending.extend(pairs.into_iter().map(|(a, b)| (a, b, depth + 1)));
        }
        Some(true)
    }
}
