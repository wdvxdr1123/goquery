//! Query-local scoped type arena; no source or expression trees are cached.
mod form;
mod identity;
mod inference;
mod interface;
mod iteration;
mod owners;
mod presentation;
mod structure;
use super::Resolution;
use crate::analysis::scope as locals;
use crate::index::IndexedSymbol as Symbol;
use crate::workspace::Workspace;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy)]
pub(super) enum OwnerKind {
    Aggregate,
    Receiver,
}

struct TypeNode<'a> {
    file: &'a PathBuf,
    text: String,
    environment: Option<usize>,
    argument: bool,
    formal: bool,
}

pub(super) struct Resolved<'a> {
    pub(super) file: &'a PathBuf,
    pub(super) symbol: &'a Symbol,
    arguments: Vec<usize>,
}

/// Environments point to older nodes, avoiding copied recursive substitutions.
pub(super) struct TypeContext<'a> {
    workspace: &'a Workspace,
    nodes: Vec<TypeNode<'a>>,
    environments: Vec<Vec<(String, usize)>>,
    parameters: HashMap<(&'a PathBuf, usize), Vec<String>>,
    pub(super) argument_roots: Vec<(&'a PathBuf, String)>,
    bytes: usize,
    symbolic: bool,
}

impl<'a> TypeContext<'a> {
    fn type_parameters(&mut self, origin: &'a PathBuf, symbol: &Symbol) -> Option<Vec<String>> {
        if !symbol.generic {
            return Some(Vec::new());
        }
        let key = (origin, symbol.start);
        if let Some(parameters) = self.parameters.get(&key) {
            return Some(parameters.clone());
        }
        let source = self.workspace.source(origin)?;
        let parameters = locals::type_parameters(source.get(symbol.end..)?)?;
        self.bytes = self
            .bytes
            .checked_add(parameters.iter().map(String::len).sum())?;
        if self.bytes > 256 * 1024 {
            return None;
        }
        self.parameters.insert(key, parameters.clone());
        Some(parameters)
    }

    pub(super) fn environment(&mut self, resolved: &Resolved<'a>) -> Option<Option<usize>> {
        let parameters = self.type_parameters(resolved.file, resolved.symbol)?;
        if parameters.len() != resolved.arguments.len() {
            return None;
        }
        if parameters.is_empty() {
            return Some(None);
        }
        self.bytes = self
            .bytes
            .checked_add(parameters.iter().map(String::len).sum())?;
        if self.bytes > 256 * 1024 {
            return None;
        }
        let environment = self.environments.len();
        self.environments.push(
            parameters
                .into_iter()
                .zip(resolved.arguments.iter().copied())
                .collect(),
        );
        Some(Some(environment))
    }

    pub(super) fn renamed_environment(
        &mut self,
        environment: Option<usize>,
        names: Vec<String>,
    ) -> Option<Option<usize>> {
        let Some(environment) = environment else {
            return names.is_empty().then_some(None);
        };
        let arguments: Vec<_> = self.environments[environment]
            .iter()
            .map(|(_, argument)| *argument)
            .collect();
        if names.len() != arguments.len() {
            return None;
        }
        self.bytes = self
            .bytes
            .checked_add(names.iter().map(String::len).sum())?;
        if self.bytes > 256 * 1024 {
            return None;
        }
        let environment = self.environments.len();
        self.environments
            .push(names.into_iter().zip(arguments).collect());
        Some(Some(environment))
    }

    pub(super) fn new(workspace: &'a Workspace) -> Self {
        Self {
            workspace,
            nodes: Vec::new(),
            environments: Vec::new(),
            parameters: HashMap::new(),
            argument_roots: Vec::new(),
            bytes: 0,
            symbolic: false,
        }
    }

    pub(super) fn push(
        &mut self,
        file: &'a PathBuf,
        text: &str,
        environment: Option<usize>,
    ) -> Option<usize> {
        if text.len() > 16 * 1024 || self.nodes.len() >= 4096 {
            return None;
        }
        self.bytes = self.bytes.checked_add(text.len())?;
        if self.bytes > 256 * 1024 {
            return None;
        }
        let id = self.nodes.len();
        self.nodes.push(TypeNode {
            file,
            text: text.to_owned(),
            environment,
            argument: false,
            formal: false,
        });
        Some(id)
    }

    fn binding(&self, environment: Option<usize>, name: &str) -> Option<usize> {
        self.environments
            .get(environment?)?
            .iter()
            .find_map(|(parameter, argument)| (parameter == name).then_some(*argument))
    }

    pub(super) fn resolve(
        &mut self,
        mut input: usize,
        resolution: Resolution,
    ) -> Option<Resolved<'a>> {
        for _ in 0..64 {
            let node = &self.nodes[input];
            if self.symbolic && node.formal {
                return None;
            }
            let (file, environment) = (node.file, node.environment);
            let name = locals::named_type(&node.text)?;
            if name == locals::named_root(&name)
                && let Some(argument) = self.binding(environment, &name)
            {
                input = argument;
                continue;
            }
            if node.argument {
                let root = locals::named_root(&name);
                self.bytes = self.bytes.checked_add(root.len())?;
                if self.bytes > 256 * 1024 {
                    return None;
                }
                self.argument_roots.push((file, root.to_owned()));
            }
            let (origin, symbol) = self.workspace.type_declaration(file, &name)?;
            let mut arguments = Vec::new();
            for argument in locals::type_arguments(&name).unwrap_or_default() {
                let id = self.push(file, argument, environment)?;
                self.nodes[id].argument = true;
                arguments.push(id);
            }
            let Some(target) = symbol
                .type_info
                .as_ref()
                .and_then(|info| info.target.as_ref())
                .filter(|target| target.alias || !matches!(resolution, Resolution::Aliases))
            else {
                return Some(Resolved {
                    file: origin,
                    symbol,
                    arguments,
                });
            };
            if matches!(resolution, Resolution::Composite)
                && crate::syntax::Lexer::new(&target.name).next()?.kind
                    == crate::syntax::TokenKind::Mul
            {
                return None;
            }
            let parameters = if target.generic {
                self.type_parameters(origin, symbol)?
            } else {
                Vec::new()
            };
            if parameters.len() != arguments.len() {
                return None;
            }
            // A parameter of this declaration cannot itself be the given type.
            // Pointer/aggregate types containing parameters are distinct cases.
            if crate::syntax::Lexer::new(&target.name).next()?.kind
                == crate::syntax::TokenKind::Ident
                && parameters
                    .iter()
                    .any(|parameter| parameter == locals::named_root(&target.name))
            {
                return None;
            }
            self.bytes = self
                .bytes
                .checked_add(parameters.iter().map(String::len).sum())?;
            if self.bytes > 256 * 1024 {
                return None;
            }
            let environment = self.environments.len();
            self.environments
                .push(parameters.into_iter().zip(arguments).collect());
            input = self.push(origin, &target.name, Some(environment))?;
        }
        None
    }

    /// Re-express arguments only when their names remain valid in the consumer.
    pub(super) fn render_text(
        &self,
        origin: &'a PathBuf,
        text: &str,
        environment: Option<usize>,
        file: &Path,
    ) -> Option<String> {
        if text.len() > 16 * 1024 {
            return None;
        }
        let node = TypeNode {
            file: origin,
            text: text.to_owned(),
            environment,
            argument: false,
            formal: false,
        };
        self.render_node(&node, file, 0)
    }

    fn render(&self, input: usize, file: &Path, depth: usize) -> Option<String> {
        self.render_node(&self.nodes[input], file, depth)
    }

    fn render_node(&self, node: &TypeNode<'a>, file: &Path, depth: usize) -> Option<String> {
        if depth >= 32 {
            return None;
        }
        if node.formal {
            return Some(node.text.clone());
        }
        let parsed = crate::syntax::parse_type(&node.text);
        if !parsed.diagnostics.is_empty() {
            return None;
        }
        let mut substitutions = Vec::new();
        let mut substitution_bytes = 0usize;
        for reference in parsed.references {
            let start = reference
                .qualifier
                .as_ref()
                .map_or(reference.name.start, |qualifier| qualifier.start);
            let name = &node.text[reference.name.clone()];
            let replacement = if reference.qualifier.is_none()
                && let Some(argument) = self.binding(node.environment, name)
            {
                self.render(argument, file, depth + 1)?
            } else {
                let spelling = reference.qualifier.as_ref().map_or_else(
                    || name.to_owned(),
                    |qualifier| format!("{}.{name}", &node.text[qualifier.clone()]),
                );
                if self
                    .workspace
                    .type_declaration(node.file, &spelling)
                    .is_some()
                {
                    self.workspace.result_type(file, node.file, &spelling)?
                } else if node.file == file || builtin(name) && reference.qualifier.is_none() {
                    spelling
                } else {
                    return None;
                }
            };
            substitution_bytes = substitution_bytes.checked_add(replacement.len())?;
            if substitution_bytes > 16 * 1024 {
                return None;
            }
            substitutions.push((start..reference.name.end, replacement));
        }
        substitutions.sort_unstable_by_key(|(span, _)| span.start);
        let mut output = String::new();
        let mut end = 0;
        for (span, replacement) in substitutions {
            if span.start < end {
                return None;
            }
            if output
                .len()
                .saturating_add(span.start - end)
                .saturating_add(replacement.len())
                > 16 * 1024
            {
                return None;
            }
            output.push_str(&node.text[end..span.start]);
            output.push_str(&replacement);
            end = span.end;
        }
        if output.len().saturating_add(node.text.len() - end) > 16 * 1024 {
            return None;
        }
        output.push_str(&node.text[end..]);
        Some(output)
    }
}

fn builtin(name: &str) -> bool {
    matches!(
        name,
        "any"
            | "bool"
            | "byte"
            | "comparable"
            | "complex64"
            | "complex128"
            | "error"
            | "float32"
            | "float64"
            | "int"
            | "int8"
            | "int16"
            | "int32"
            | "int64"
            | "rune"
            | "string"
            | "uint"
            | "uint8"
            | "uint16"
            | "uint32"
            | "uint64"
            | "uintptr"
    )
}
