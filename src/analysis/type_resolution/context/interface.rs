//! Complete basic-interface method unions for symbolic signature identity.
use super::{Resolved, TypeContext};
use crate::analysis::resolve;
use crate::analysis::scope as locals;
use crate::analysis::type_resolution;
use crate::analysis::type_resolution::signature;
use crate::syntax::{Lexer, TokenKind};
use crate::text::SourceMap;
use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

struct InterfaceSet<'a> {
    methods: BTreeMap<(String, String), String>,
    active: HashSet<(&'a PathBuf, usize)>,
    work: usize,
    bytes: usize,
    limit: usize,
}

impl InterfaceSet<'_> {
    fn new(limit: usize) -> Self {
        Self {
            methods: BTreeMap::new(),
            active: HashSet::new(),
            work: 4096,
            bytes: 0,
            limit,
        }
    }

    fn insert(&mut self, name: String, package: String, signature: String) -> Option<()> {
        let key = (name, package);
        if let Some(previous) = self.methods.get(&key) {
            return (previous == &signature).then_some(());
        }
        self.bytes = self
            .bytes
            .checked_add(key.0.len() + key.1.len() + signature.len())?;
        if self.bytes > 16 * 1024 || self.methods.len() >= self.limit {
            return None;
        }
        self.methods.insert(key, signature);
        Some(())
    }
}

impl<'a> TypeContext<'a> {
    pub(super) fn interface_identity(&mut self, input: usize, depth: usize) -> Option<String> {
        let mut set = InterfaceSet::new(4096);
        self.interface_methods(input, depth, &mut set)?;
        if set.methods.is_empty() {
            return signature::pack("atom", &["any".to_owned()]);
        }
        let mut methods = Vec::new();
        for ((name, package), identity) in set.methods {
            methods.push(signature::pack(
                "method",
                &[
                    signature::pack("atom", &[name])?,
                    signature::pack("atom", &[package])?,
                    identity,
                ],
            )?);
        }
        signature::pack("interface", &methods)
    }

    fn interface_methods(
        &mut self,
        input: usize,
        depth: usize,
        set: &mut InterfaceSet<'a>,
    ) -> Option<()> {
        if depth >= 32 || self.nodes[input].formal {
            return None;
        }
        set.work = set.work.checked_sub(1)?;
        let node = &self.nodes[input];
        let (file, environment) = (node.file, node.environment);
        let text = node.text.clone();
        let parsed = crate::syntax::parse_type(&text);
        if !parsed.diagnostics.is_empty() {
            return None;
        }
        let mut tokens = Lexer::new(&text);
        match tokens.next()?.kind {
            TokenKind::Interface => {
                let container = tokens.next()?.span.start;
                for member in parsed
                    .interface_members
                    .iter()
                    .filter(|m| m.container == container)
                {
                    if let Some(name) = &member.name {
                        let name = text.get(name.clone())?;
                        let package = if resolve::exported(name) {
                            String::new()
                        } else {
                            format!(
                                "{}:{}",
                                file.parent()?.to_str()?,
                                self.workspace.files[file].package
                            )
                        };
                        let identity = self.signature_identity(
                            file,
                            &format!("func{}", text.get(member.annotation.clone())?),
                            environment,
                            depth + 1,
                        )?;
                        set.insert(name.to_owned(), package, identity)?;
                    } else {
                        let next =
                            self.push(file, text.get(member.annotation.clone())?, environment)?;
                        self.interface_methods(next, depth + 1, set)?;
                    }
                }
                Some(())
            }
            TokenKind::LParen => {
                let start = Lexer::new(&text).next()?.span.end;
                let end = tokens
                    .filter(|token| token.kind != TokenKind::Semicolon)
                    .last()?
                    .span
                    .start;
                let next = self.push(file, text.get(start..end)?, environment)?;
                self.interface_methods(next, depth + 1, set)
            }
            TokenKind::Ident => {
                let name = locals::named_type(&text)?;
                if name == locals::named_root(&name)
                    && let Some(argument) = self.binding(environment, &name)
                {
                    return self.interface_methods(argument, depth + 1, set);
                }
                let Some((origin, symbol)) = self.workspace.type_declaration(file, &name) else {
                    return match name.as_str() {
                        "any" => Some(()),
                        "error" => {
                            // The result belongs to the universe scope, even
                            // when this source declares its own string type.
                            let result = signature::pack(
                                "type",
                                &[signature::pack("atom", &["string".to_owned()])?],
                            )?;
                            let identity = signature::pack(
                                "func",
                                &[
                                    signature::pack("in", &[])?,
                                    signature::pack("out", &[result])?,
                                ],
                            )?;
                            set.insert("Error".to_owned(), String::new(), identity)
                        }
                        _ => None,
                    };
                };
                if !set.active.insert((origin, symbol.start)) {
                    return None;
                }
                let mut arguments = Vec::new();
                for argument in locals::type_arguments(&name).unwrap_or_default() {
                    arguments.push(self.push(file, argument, environment)?);
                }
                let resolved = Resolved {
                    file: origin,
                    symbol,
                    arguments,
                };
                let environment = self.environment(&resolved)?;
                let source = self.workspace.source(origin)?;
                let map = SourceMap::new(&source);
                let declaration = source.get(symbol.end..map.offset(symbol.range.end)?)?;
                if declaration.len() > 16 * 1024 {
                    return None;
                }
                let body = type_resolution::declaration_body(declaration)?.to_owned();
                drop(source);
                let next = self.push(origin, &body, environment)?;
                self.interface_methods(next, depth + 1, set)?;
                set.active.remove(&(origin, symbol.start));
                Some(())
            }
            // Non-basic type terms and unresolved embeddings are not empty sets.
            _ => None,
        }
    }
}
