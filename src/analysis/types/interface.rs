//! Complete basic-interface method sets stored only in the bounded type arena.
use super::arena::DisplayScope;
use super::{BasicKind, MAX_BYTES, MAX_DEPTH, MAX_NODES, Operand, Ty, TypeArena, TypeId};
use crate::syntax::{Lexer, TokenKind};
use std::collections::{BTreeMap, HashSet};
use std::ops::Range;
use std::path::Path;

#[derive(Clone)]
pub(super) struct Method<'a> {
    pub(super) name: String,
    pub(super) origin: &'a Path,
    pub(super) signature: TypeId,
}

impl<'a> TypeArena<'a> {
    pub(super) fn read_interface(
        &mut self,
        origin: &Path,
        source: &str,
        environment: &[(&str, TypeId)],
        depth: usize,
    ) -> Option<TypeId> {
        if depth >= MAX_DEPTH || source.len() > MAX_BYTES {
            return None;
        }
        let origin = self.workspace.files.get_key_value(origin)?.0.as_path();
        let parsed = crate::syntax::parse_type(source);
        if !parsed.diagnostics.is_empty() {
            return None;
        }
        let container = Lexer::new(source).nth(1)?.span.start;
        let mut methods = Vec::new();
        let mut explicit = HashSet::new();
        for member in parsed
            .interface_members
            .iter()
            .filter(|m| m.container == container)
        {
            if let Some(name) = &member.name {
                let name = source.get(name.clone())?;
                if name == "_" || !explicit.insert(name) {
                    self.invalid_type = true;
                    return None;
                }
                let header = format!("func{}", source.get(member.annotation.clone())?);
                let signature = crate::syntax::parse_literal_signature(&header);
                let signature =
                    self.signature(origin, &header, &signature, environment, depth + 1)?;
                methods.push(Method {
                    name: name.to_owned(),
                    origin,
                    signature,
                });
            } else {
                let text = source.get(member.annotation.clone())?;
                let mut tokens = Lexer::new(text).peekable();
                let ty = self.read(origin, text, &mut tokens, environment, depth + 1)?;
                if !tokens.all(|t| t.kind == TokenKind::Semicolon) {
                    return None;
                }
                let ty = self.underlying(ty)?;
                let Ty::Interface {
                    methods: embedded, ..
                } = &self.nodes[usize::from(ty.0)]
                else {
                    return None;
                };
                methods.extend(self.methods[embedded.clone()].iter().cloned());
            }
            if methods.len() > MAX_NODES {
                return None;
            }
        }
        self.store_interface(origin, methods)
    }

    pub(super) fn builtin_interface(&mut self, origin: &Path, error: bool) -> Option<TypeId> {
        let origin = self.workspace.files.get_key_value(origin)?.0.as_path();
        let methods = if error {
            let result = self.basic(BasicKind::String)?;
            let signature = self.store_signature(Vec::new(), vec![result], false)?;
            vec![Method {
                name: "Error".to_owned(),
                origin,
                signature,
            }]
        } else {
            Vec::new()
        };
        let interface = self.store_interface(origin, methods)?;
        if error {
            self.push(Ty::Error(interface))
        } else {
            if let Ty::Interface { builtin_any, .. } = &mut self.nodes[usize::from(interface.0)] {
                *builtin_any = true;
            }
            Some(interface)
        }
    }

    pub(super) fn store_interface(
        &mut self,
        origin: &'a Path,
        methods: Vec<Method<'a>>,
    ) -> Option<TypeId> {
        let mut sorted: BTreeMap<(String, String), Method<'a>> = BTreeMap::new();
        for method in methods {
            let package = if crate::syntax::is_exported_name(&method.name) {
                String::new()
            } else {
                self.workspace.import_path(method.origin)?
            };
            let key = (method.name.clone(), package);
            if let Some(previous) = sorted.get(&key) {
                if !self.identical(previous.signature, method.signature)? {
                    self.invalid_type = true;
                    return None;
                }
            } else {
                sorted.insert(key, method);
            }
        }
        let bytes = sorted.values().map(|m| m.name.len()).sum::<usize>();
        if self.methods.len() + sorted.len() > MAX_NODES || self.method_bytes + bytes > MAX_BYTES {
            return None;
        }
        self.method_bytes += bytes;
        let start = self.methods.len();
        self.methods.extend(sorted.into_values());
        self.push(Ty::Interface {
            origin,
            methods: start..self.methods.len(),
            builtin_any: false,
        })
    }

    pub(super) fn same_interface_methods(&self, a: Range<usize>, b: Range<usize>) -> Option<bool> {
        if a.len() != b.len() {
            return Some(false);
        }
        for (a, b) in self.methods[a].iter().zip(&self.methods[b]) {
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
        Some(matches!(
            self.nodes[usize::from(input.0)],
            Ty::Interface { .. }
        ))
    }

    /// Check a metadata inventory against the complete typed method set,
    /// including universe methods without indexed source declarations.
    pub(crate) fn interface_methods_covered(
        &mut self,
        input: TypeId,
        mut contains: impl FnMut(&Path, &str) -> bool,
    ) -> Option<bool> {
        let input = self.underlying(input)?;
        let Ty::Interface { methods, .. } = &self.nodes[usize::from(input.0)] else {
            return None;
        };
        Some(
            self.methods[methods.clone()]
                .iter()
                .all(|method| contains(method.origin, &method.name)),
        )
    }

    /// Inference must not instantiate constrained named formals before their
    /// arguments have been inferred. Constructor metadata is sufficient here;
    /// assignment checks the concrete instantiated type afterward.
    pub(super) fn interface_constructor(&mut self, input: TypeId) -> Option<bool> {
        if let Ty::Named { declaration, .. } = self.nodes[usize::from(input.0)].clone() {
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
                    self.workspace.type_declaration(file, &target.name)
                else {
                    return Some(matches!(target.name.as_str(), "any" | "error"));
                };
                (file, symbol) = (next_file, next_symbol);
            }
            return None;
        }
        self.is_interface(input)
    }

    pub(super) fn assign_interface(
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
        let Ty::Interface {
            methods: required, ..
        } = self.nodes[usize::from(target_root.0)].clone()
        else {
            return None;
        };
        if required.is_empty() {
            return Some(true);
        }
        for index in required {
            let method = self.methods[index].clone();
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
        result: usize,
        count: usize,
        int_bits: u32,
    ) -> Option<Operand> {
        self.step()?;
        if !matches!(count, 1 | 2) || result >= count || !self.is_interface(source.ty)? {
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
            mode: super::Mode::Value,
            ty: if result == 0 {
                target
            } else {
                self.basic(BasicKind::Bool)?
            },
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
        if targets.len() > MAX_NODES || defaults > 1 || !self.is_interface(source.ty)? {
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
                self.assertion(source, *target, 0, 1, int_bits)?;
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

    pub(super) fn write_interface(
        &self,
        _origin: &Path,
        methods: Range<usize>,
        builtin_any: bool,
        consumer: DisplayScope<'_>,
        out: &mut String,
        depth: usize,
    ) -> Option<()> {
        // The universe alias keeps its source spelling in types.TypeString.
        if builtin_any && methods.is_empty() {
            out.push_str("any");
            return Some(());
        }
        out.push_str("interface{");
        for method in &self.methods[methods] {
            if !crate::syntax::is_exported_name(&method.name)
                && let DisplayScope::Consumer(file) = consumer
                && !self.same_field_package(method.origin, file)?
            {
                return None;
            }
            out.push_str(&method.name);
            let mut signature = String::new();
            self.write_in(method.signature, consumer, &mut signature, depth + 1)?;
            out.push_str(signature.strip_prefix("func")?);
            out.push(';');
            if out.len() > MAX_BYTES {
                return None;
            }
        }
        out.push('}');
        Some(())
    }
}
