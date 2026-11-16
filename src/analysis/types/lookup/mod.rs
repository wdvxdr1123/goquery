//! Scoped type-tree target lookup, projection, and owner environments.
//!
//! Used by symbol/completion/hierarchy/hover queries where Go syntax spelling or
//! incomplete/huge declarations (>16KiB) must stay lightweight without retaining ASTs.

mod members;
pub(crate) use members::*;

use super::{TypeArena, TypeForm, TypeId, TypePresentation, declaration_body, type_parentheses};
use crate::analysis::budget::{MAX_BYTES, MAX_DEPTH};
use crate::analysis::obj::MemberDeclaration;
use crate::analysis::resolve::{resolve_type_declaration, result_type};
use crate::analysis::scope as locals;
use crate::index::IndexedSymbol as Symbol;
use crate::syntax::{Lexer, TokenKind};
use crate::workspace::Workspace;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy)]
enum Resolution {
    Aliases,
    Underlying,
    Composite,
}

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
pub(super) struct TypeTree<'a> {
    workspace: &'a Workspace,
    nodes: Vec<TypeNode<'a>>,
    environments: Vec<Vec<(String, usize)>>,
    parameters: HashMap<(&'a PathBuf, usize), Vec<String>>,
    pub(super) argument_roots: Vec<(&'a PathBuf, String)>,
    bytes: usize,
    symbolic: bool,
}

impl<'a> TypeTree<'a> {
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
        if text.len() > MAX_BYTES || self.nodes.len() >= 4096 {
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

    fn type_parameters(&mut self, origin: &'a PathBuf, symbol: &Symbol) -> Option<Vec<String>> {
        if !symbol.generic {
            return Some(Vec::new());
        }
        let key = (origin, symbol.start);
        if let Some(parameters) = self.parameters.get(&key) {
            return Some(parameters.clone());
        }
        let end = symbol
            .declaration()?
            .end
            .min(symbol.end.saturating_add(MAX_BYTES));
        let header = self.workspace.source_slice(origin, symbol.end..end)?;
        let parameters = locals::type_parameters(&header).or_else(|| {
            let source = self.workspace.source(origin)?;
            locals::type_parameters(source.get(symbol.end..)?)
        })?;
        self.bytes = self
            .bytes
            .checked_add(parameters.iter().map(String::len).sum())?;
        if self.bytes > 256 * 1024 {
            return None;
        }
        self.parameters.insert(key, parameters.clone());
        Some(parameters)
    }

    fn environment(&mut self, resolved: &Resolved<'a>) -> Option<Option<usize>> {
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

    fn renamed_environment(
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

    fn name_environment(&mut self, file: &'a PathBuf, names: &[String]) -> Option<usize> {
        let mut bindings = Vec::new();
        for name in names {
            let id = self.push(file, name, None)?;
            self.nodes[id].formal = true;
            self.bytes = self.bytes.checked_add(name.len())?;
            if self.bytes > 256 * 1024 {
                return None;
            }
            bindings.push((name.clone(), id));
        }
        let id = self.environments.len();
        self.environments.push(bindings);
        Some(id)
    }

    fn binding(&self, environment: Option<usize>, name: &str) -> Option<usize> {
        self.environments
            .get(environment?)?
            .iter()
            .find_map(|(parameter, argument)| (parameter == name).then_some(*argument))
    }

    fn resolve(&mut self, mut input: usize, resolution: Resolution) -> Option<Resolved<'a>> {
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
            let (origin, symbol) = resolve_type_declaration(self.workspace, file, &name)?;
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
                && Lexer::new(&target.name).next()?.kind == TokenKind::Mul
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
            if Lexer::new(&target.name).next()?.kind == TokenKind::Ident
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

    fn owner_environment(
        &mut self,
        input: usize,
        owner_file: &Path,
        owner: &Symbol,
        kind: OwnerKind,
    ) -> Option<(&'a PathBuf, Option<usize>)> {
        struct Step<'a> {
            node: usize,
            parent: Option<usize>,
            identity: Option<(&'a PathBuf, usize)>,
            interface_group: Option<usize>,
        }
        let mut steps = vec![Step {
            node: input,
            parent: None,
            identity: None,
            interface_group: None,
        }];
        let mut level = VecDeque::from([0]);
        for _ in 0..128 {
            let mut next = Vec::new();
            let mut pending = Vec::new();
            let mut found = None;
            while let Some(id) = level.pop_front() {
                if matches!(kind, OwnerKind::Receiver)
                    && let Some(resolved) = self.resolve(steps[id].node, Resolution::Aliases)
                    && resolved.file == owner_file
                    && resolved.symbol.start == owner.start
                {
                    let environment = self.environment(&resolved)?;
                    if found.replace((resolved.file, environment, None)).is_some() {
                        return None;
                    }
                    continue;
                }
                let resolved = match self.resolve(steps[id].node, Resolution::Underlying) {
                    Some(resolved) => resolved,
                    None => continue,
                };
                let identity = (resolved.file, resolved.symbol.start);
                let mut ancestor = steps[id].parent;
                let mut cycle = false;
                while let Some(parent) = ancestor {
                    if steps[parent].identity == Some(identity) {
                        cycle = true;
                        break;
                    }
                    ancestor = steps[parent].parent;
                }
                if cycle {
                    continue;
                }
                steps[id].identity = Some(identity);
                if matches!(kind, OwnerKind::Aggregate)
                    && resolved.file == owner_file
                    && resolved.symbol.start == owner.start
                {
                    let environment = self.environment(&resolved)?;
                    let group = steps[id].interface_group.or_else(|| {
                        resolved
                            .symbol
                            .type_info
                            .as_ref()
                            .is_some_and(|info| info.is_interface())
                            .then_some(id)
                    });
                    if let Some((_, _, previous)) = found {
                        if group.is_none() || group != previous {
                            return None;
                        }
                    } else {
                        found = Some((resolved.file, environment, group));
                    }
                    continue;
                }
                if let Some(info) = resolved
                    .symbol
                    .type_info
                    .as_ref()
                    .filter(|info| info.is_interface())
                {
                    let environment = self.environment(&resolved)?;
                    let group = steps[id].interface_group.or(Some(id));
                    for embedded in info.embedded.iter().rev() {
                        let node = self.push(resolved.file, embedded, environment)?;
                        level.push_front(steps.len());
                        steps.push(Step {
                            node,
                            parent: Some(id),
                            identity: None,
                            interface_group: group,
                        });
                        if steps.len() > 4096 {
                            return None;
                        }
                    }
                } else {
                    pending.push((id, resolved));
                }
            }
            if let Some((origin, environment, _)) = found {
                return Some((origin, environment));
            }
            for (id, resolved) in pending {
                let environment = self.environment(&resolved)?;
                if let Some(info) = &resolved.symbol.type_info {
                    for embedded in &info.embedded {
                        let node = self.push(resolved.file, embedded, environment)?;
                        next.push(steps.len());
                        steps.push(Step {
                            node,
                            parent: Some(id),
                            identity: None,
                            interface_group: None,
                        });
                        if steps.len() > 4096 {
                            return None;
                        }
                    }
                }
            }
            if next.is_empty() {
                return None;
            }
            level = next.into();
        }
        None
    }

    pub(super) fn render_text(
        &self,
        origin: &'a PathBuf,
        text: &str,
        environment: Option<usize>,
        file: &Path,
    ) -> Option<String> {
        if text.len() > MAX_BYTES {
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
        if depth >= MAX_DEPTH {
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
                if resolve_type_declaration(self.workspace, node.file, &spelling).is_some() {
                    result_type(self.workspace, file, node.file, &spelling)?
                } else if node.file == file
                    || (crate::analysis::scope::universe::builtin_type(name)
                        && reference.qualifier.is_none())
                {
                    spelling
                } else {
                    return None;
                }
            };
            substitution_bytes = substitution_bytes.checked_add(replacement.len())?;
            if substitution_bytes > MAX_BYTES {
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
                > MAX_BYTES
            {
                return None;
            }
            output.push_str(&node.text[end..span.start]);
            output.push_str(&replacement);
            end = span.end;
        }
        if output.len().saturating_add(node.text.len() - end) > MAX_BYTES {
            return None;
        }
        output.push_str(&node.text[end..]);
        Some(output)
    }

    pub(super) fn form(&mut self, input: usize, depth: usize) -> Option<TypeForm> {
        if depth >= 64 {
            return None;
        }
        let node = &self.nodes[input];
        let (file, environment, argument) = (node.file, node.environment, node.argument);
        if let Some(form) = TypeForm::constructor(&node.text) {
            return Some(form);
        }
        if let Some(span) = type_parentheses(&node.text)? {
            let text = node.text.get(span)?.to_owned();
            let next = self.push(file, &text, environment)?;
            self.nodes[next].argument = argument;
            return self.form(next, depth + 1);
        }
        let name = locals::named_type(&node.text)?;
        if name == locals::named_root(&name)
            && let Some(argument) = self.binding(environment, &name)
        {
            return self.form(argument, depth + 1);
        }
        if argument {
            let root = locals::named_root(&name);
            self.bytes = self.bytes.checked_add(root.len())?;
            if self.bytes > 256 * 1024 {
                return None;
            }
            self.argument_roots.push((file, root.to_owned()));
        }
        let Some((origin, symbol)) = resolve_type_declaration(self.workspace, file, &name) else {
            return crate::analysis::scope::universe::visible(self.workspace, file, None, 0, &name)
                .then(|| TypeForm::builtin(&name))
                .flatten();
        };
        let mut arguments = Vec::new();
        for text in locals::type_arguments(&name).unwrap_or_default() {
            let id = self.push(file, text, environment)?;
            self.nodes[id].argument = true;
            arguments.push(id);
        }
        let mut resolved = Resolved {
            file: origin,
            symbol,
            arguments,
        };
        let next = if let Some(target) = symbol
            .type_info
            .as_ref()
            .and_then(|info| info.target.as_ref())
        {
            if let Some(form) = TypeForm::constructor(&target.name) {
                return Some(form);
            }
            let environment = self.form_environment(&mut resolved)?;
            self.push(origin, &target.name, environment)?
        } else if let Some(info) = &symbol.type_info {
            return Some(if info.is_interface() {
                TypeForm::Interface
            } else {
                TypeForm::Struct
            });
        } else {
            let declaration = self
                .workspace
                .source_slice(origin, symbol.end..symbol.declaration()?.end)?;
            let body = declaration_body(&declaration)?;
            if let Some(form) = TypeForm::constructor(body) {
                return Some(form);
            }
            let environment = self.form_environment(&mut resolved)?;
            self.push(origin, body, environment)?
        };
        self.form(next, depth + 1)
    }

    fn form_environment(&mut self, resolved: &mut Resolved<'a>) -> Option<Option<usize>> {
        if resolved.arguments.is_empty() {
            for _ in self.type_parameters(resolved.file, resolved.symbol)? {
                resolved.arguments.push(self.push(resolved.file, "", None)?);
            }
        }
        self.environment(resolved)
    }

    pub(super) fn declaration_presentation(
        &mut self,
        origin: &'a PathBuf,
        declaration: &str,
        consumer: &Path,
    ) -> Option<TypePresentation> {
        let names = locals::type_parameters(declaration)?;
        let environment = self.name_environment(origin, &names)?;
        let input = self.push(origin, declaration_body(declaration)?, Some(environment))?;
        self.presentation(input, consumer, 0)
    }

    pub(super) fn local_presentation(
        &mut self,
        file: &'a PathBuf,
        annotation: &str,
        names: &[String],
    ) -> Option<TypePresentation> {
        let environment = self.name_environment(file, names)?;
        let input = self.push(file, annotation, Some(environment))?;
        self.presentation(input, file, 0)
    }

    fn presentation(
        &mut self,
        input: usize,
        consumer: &Path,
        depth: usize,
    ) -> Option<TypePresentation> {
        if depth >= 64 || self.nodes[input].formal {
            return None;
        }
        let node = &self.nodes[input];
        let (file, environment) = (node.file, node.environment);
        let text = node.text.clone();
        let first = Lexer::new(&text).next()?;
        if first.kind == TokenKind::LParen {
            let last = Lexer::new(&text)
                .filter(|token| token.kind != TokenKind::Semicolon)
                .last()?;
            if last.kind != TokenKind::RParen {
                return None;
            }
            let next = self.push(
                file,
                text.get(first.span.end..last.span.start)?,
                environment,
            )?;
            return self.presentation(next, consumer, depth + 1);
        }
        if first.kind == TokenKind::Ident
            && let Some(name) = locals::named_type(&text)
        {
            if name == locals::named_root(&name)
                && let Some(next) = self.binding(environment, &name)
            {
                return self.presentation(next, consumer, depth + 1);
            }
            if let Some((origin, symbol)) = resolve_type_declaration(self.workspace, file, &name) {
                let mut arguments = Vec::new();
                for text in locals::type_arguments(&name).unwrap_or_default() {
                    arguments.push(self.push(file, text, environment)?);
                }
                let resolved = Resolved {
                    file: origin,
                    symbol,
                    arguments,
                };
                let environment = self.environment(&resolved)?;
                if let Some(info) = symbol
                    .type_info
                    .as_ref()
                    .filter(|info| info.target.is_none())
                {
                    return Some(TypePresentation {
                        form: if info.is_interface() {
                            TypeForm::Interface
                        } else {
                            TypeForm::Struct
                        },
                        text: if info.is_interface() {
                            "interface{...}"
                        } else {
                            "struct{...}"
                        }
                        .to_owned(),
                    });
                }
                let next = self.declaration_body_node(origin, symbol, environment)?;
                return self.presentation(next, consumer, depth + 1);
            }
        }
        let form = self.form(input, 0)?;
        let text = match form {
            TypeForm::Struct => "struct{...}".to_owned(),
            TypeForm::Interface => "interface{...}".to_owned(),
            _ => self.render(input, consumer, 0)?.trim().to_owned(),
        };
        Some(TypePresentation { form, text })
    }

    fn declaration_body_node(
        &mut self,
        origin: &'a PathBuf,
        symbol: &Symbol,
        environment: Option<usize>,
    ) -> Option<usize> {
        let span = symbol.end..symbol.declaration()?.end;
        let declaration = if let Some(slice) = self.workspace.source_slice(origin, span.clone()) {
            slice
        } else {
            let source = self.workspace.source(origin)?;
            source.get(span)?.to_owned()
        };
        if declaration.len() > MAX_BYTES {
            return None;
        }
        self.push(origin, declaration_body(&declaration)?, environment)
    }

    fn range_slot(
        &mut self,
        input: usize,
        key: bool,
        consumer: &Path,
        depth: usize,
    ) -> Option<String> {
        if depth >= 64 {
            return None;
        }
        let node = &self.nodes[input];
        let (file, environment, argument) = (node.file, node.environment, node.argument);
        let text = node.text.clone();
        let mut tokens = Lexer::new(&text);
        let first = tokens.next()?;
        let projected = match first.kind {
            TokenKind::LParen => {
                let last = tokens
                    .filter(|token| token.kind != TokenKind::Semicolon)
                    .last()?;
                if last.kind != TokenKind::RParen {
                    return None;
                }
                let next = self.push(
                    file,
                    text.get(first.span.end..last.span.start)?,
                    environment,
                )?;
                self.nodes[next].argument = argument;
                return self.range_slot(next, key, consumer, depth + 1);
            }
            TokenKind::LBrack if !key => {
                let end = bracket_end(&text, first.span.start)?;
                &text[end..]
            }
            TokenKind::Map => {
                let open = tokens.next()?;
                if open.kind != TokenKind::LBrack {
                    return None;
                }
                let end = bracket_end(&text, open.span.start)?;
                if key {
                    &text[open.span.end..end - 1]
                } else {
                    &text[end..]
                }
            }
            TokenKind::Ident => {
                let name = locals::named_type(&text)?;
                if name == locals::named_root(&name)
                    && let Some(next) = self.binding(environment, &name)
                {
                    return self.range_slot(next, key, consumer, depth + 1);
                }
                if argument {
                    let root = locals::named_root(&name);
                    self.bytes = self.bytes.checked_add(root.len())?;
                    if self.bytes > 256 * 1024 {
                        return None;
                    }
                    self.argument_roots.push((file, root.to_owned()));
                }
                let (origin, symbol) = resolve_type_declaration(self.workspace, file, &name)?;
                let mut arguments = Vec::new();
                for text in locals::type_arguments(&name).unwrap_or_default() {
                    let next = self.push(file, text, environment)?;
                    self.nodes[next].argument = true;
                    arguments.push(next);
                }
                let resolved = Resolved {
                    file: origin,
                    symbol,
                    arguments,
                };
                let environment = self.environment(&resolved)?;
                let next = if let Some(target) = symbol
                    .type_info
                    .as_ref()
                    .and_then(|info| info.target.as_ref())
                {
                    self.push(origin, &target.name, environment)?
                } else {
                    let span = symbol.end..symbol.declaration()?.end;
                    let declaration =
                        if let Some(slice) = self.workspace.source_slice(origin, span.clone()) {
                            slice
                        } else {
                            let source = self.workspace.source(origin)?;
                            source.get(span)?.to_owned()
                        };
                    if declaration.len() > MAX_BYTES {
                        return None;
                    }
                    self.push(origin, declaration_body(&declaration)?, environment)?
                };
                return self.range_slot(next, key, consumer, depth + 1);
            }
            _ => return None,
        };
        self.render_text(file, projected, environment, consumer)
    }

    fn convert_node(
        &self,
        input: usize,
        arena: &mut TypeArena<'a>,
        root_env: &[(&str, TypeId)],
        depth: usize,
    ) -> Option<TypeId> {
        if depth >= MAX_DEPTH {
            return None;
        }
        let node = &self.nodes[input];
        if node.formal {
            return root_env
                .iter()
                .find(|(name, _)| *name == node.text)
                .map(|(_, ty)| *ty);
        }
        let mut local_owned = Vec::new();
        if let Some(env_id) = node.environment {
            for (name, arg) in &self.environments[env_id] {
                let ty = self.convert_node(*arg, arena, root_env, depth + 1)?;
                local_owned.push((name.as_str(), ty));
            }
        }
        arena.parse_in(node.file, &node.text, &local_owned)
    }
}

/// Member lookup follows aliases but not distinct defined types.
pub(crate) fn embedded_target<'a>(
    workspace: &'a Workspace,
    file: &Path,
    name: &str,
) -> Option<(&'a PathBuf, &'a Symbol)> {
    let direct = resolve_type_declaration(workspace, file, name)?;
    if direct
        .1
        .type_info
        .as_ref()
        .and_then(|info| info.target.as_ref())
        .filter(|target| target.alias)
        .is_none()
    {
        return Some(direct);
    }
    let mut tree = TypeTree::new(workspace);
    let file = workspace.files.get_key_value(file)?.0;
    let input = tree.push(file, name, None)?;
    let resolved = tree.resolve(input, Resolution::Aliases)?;
    Some((resolved.file, resolved.symbol))
}

/// Structural fields/interface methods follow definitions without inheriting concrete methods.
pub(crate) fn underlying_target<'a>(
    workspace: &'a Workspace,
    file: &Path,
    name: &str,
) -> Option<(&'a PathBuf, &'a Symbol)> {
    structural_target(workspace, file, name, Resolution::Underlying)
}

/// Composite keys require a non-pointer aggregate underlying type.
pub(crate) fn composite_target<'a>(
    workspace: &'a Workspace,
    file: &Path,
    name: &str,
) -> Option<(&'a PathBuf, &'a Symbol)> {
    structural_target(workspace, file, name, Resolution::Composite)
}

fn structural_target<'a>(
    workspace: &'a Workspace,
    file: &Path,
    name: &str,
    resolution: Resolution,
) -> Option<(&'a PathBuf, &'a Symbol)> {
    let direct = resolve_type_declaration(workspace, file, name)?;
    if direct
        .1
        .type_info
        .as_ref()
        .and_then(|info| info.target.as_ref())
        .is_none()
    {
        return Some(direct);
    }
    let mut tree = TypeTree::new(workspace);
    let file = workspace.files.get_key_value(file)?.0;
    let input = tree.push(file, name, None)?;
    let resolved = tree.resolve(input, resolution)?;
    Some((resolved.file, resolved.symbol))
}

/// An argument used as the receiver root must not resolve through a local
/// type parameter to an unrelated same-spelled package declaration.
pub(crate) fn receiver_in_scope(
    workspace: &Workspace,
    file: &Path,
    name: &str,
    locals: &locals::Locals,
    offset: usize,
) -> bool {
    if let Some(name) = locals::named_type(name)
        && locals
            .resolve_at(
                offset,
                locals::named_root(&name).split('.').next().unwrap_or(""),
            )
            .is_some_and(|binding| binding.object_kind.is_type())
    {
        return false;
    }
    if !name.contains('[')
        || !locals
            .visible_bindings(offset)
            .any(|binding| binding.object_kind.is_type())
        || !resolve_type_declaration(workspace, file, name).is_some_and(|(_, symbol)| {
            symbol
                .type_info
                .as_ref()
                .is_some_and(|info| info.target.is_some())
        })
    {
        return true;
    }
    let Some((file, _)) = workspace.files.get_key_value(file) else {
        return false;
    };
    let mut tree = TypeTree::new(workspace);
    let Some(input) = tree.push(file, name, None) else {
        return false;
    };
    let _ = tree.resolve(input, Resolution::Underlying);
    !tree.argument_roots.iter().any(|(origin, name)| {
        *origin == file
            && locals
                .resolve_at(offset, name.split('.').next().expect("named root"))
                .is_some_and(|binding| binding.object_kind.is_type())
    })
}

/// Generic named indirections need written arguments during metadata lookup.
/// Simple aggregate declarations can use their stub names without a read.
pub(crate) fn implementation_spelling(
    workspace: &Workspace,
    file: &Path,
    owner: &Symbol,
) -> Option<String> {
    if !owner
        .type_info
        .as_ref()
        .and_then(|info| info.target.as_ref())
        .is_some_and(|target| target.generic)
    {
        return Some(owner.name.clone());
    }
    let end = owner
        .declaration()?
        .end
        .min(owner.end.saturating_add(MAX_BYTES));
    let names = workspace
        .source_slice(file, owner.end..end)
        .and_then(|slice| locals::type_parameters(&slice))
        .or_else(|| {
            let source = workspace.source(file)?;
            locals::type_parameters(source.get(owner.end..)?)
        })?;
    let result = format!("{}[{}]", owner.name, names.join(","));
    (result.len() <= MAX_BYTES).then_some(result)
}

pub(crate) fn composite_element_type(
    workspace: &Workspace,
    file: &Path,
    text: &str,
    locals: &locals::Locals,
    offset: usize,
    key: bool,
) -> Option<String> {
    if text.len() > MAX_BYTES {
        return None;
    }
    if let Some(name) = locals::named_type(text)
        && locals
            .resolve_at(offset, locals::named_root(&name).split('.').next()?)
            .is_some()
    {
        return None;
    }
    let file = workspace.files.get_key_value(file)?.0;
    let mut tree = TypeTree::new(workspace);
    let input = tree.push(file, text, None)?;
    tree.nodes[input].argument = true;
    let ty = tree.range_slot(input, key, file, 0)?.trim().to_owned();
    if tree
        .argument_roots
        .iter()
        .any(|(origin, root)| *origin == file && locals.resolve_at(offset, root).is_some())
    {
        return None;
    }
    if let Some(name) = locals::named_type(&ty)
        && locals
            .resolve_at(offset, locals::named_root(&name).split('.').next()?)
            .is_some()
    {
        return None;
    }
    Some(ty)
}

/// Prepared only for one selected owner during a completion/hover/signature batch.
pub(crate) struct ScopedOwnerEnv<'a> {
    tree: TypeTree<'a>,
    origin: &'a PathBuf,
    environment: Option<usize>,
    file: &'a PathBuf,
}

impl<'a> ScopedOwnerEnv<'a> {
    pub(crate) fn for_member(
        workspace: &'a Workspace,
        file: &Path,
        scope: &Path,
        receiver: &str,
        member: MemberDeclaration<'_>,
    ) -> Option<Self> {
        let mut tree = TypeTree::new(workspace);
        let file = workspace.files.get_key_value(file)?.0;
        let scope = workspace.files.get_key_value(scope)?.0;
        let input = tree.push(scope, receiver, None)?;
        let (origin, environment) =
            tree.owner_environment(input, member.file, member.owner, OwnerKind::Aggregate)?;
        Some(Self {
            tree,
            origin,
            environment,
            file,
        })
    }

    pub(crate) fn for_method(
        workspace: &'a Workspace,
        file: &Path,
        scope: &Path,
        receiver: &str,
        origin: &Path,
        method: &Symbol,
        names: Vec<String>,
    ) -> Option<Self> {
        let (owner_file, owner) =
            resolve_type_declaration(workspace, origin, method.receiver.as_deref()?)?;
        let mut tree = TypeTree::new(workspace);
        let file = workspace.files.get_key_value(file)?.0;
        let scope = workspace.files.get_key_value(scope)?.0;
        let input = tree.push(scope, receiver, None)?;
        let (_, environment) =
            tree.owner_environment(input, owner_file, owner, OwnerKind::Receiver)?;
        let environment = tree.renamed_environment(environment, names)?;
        let origin = workspace.files.get_key_value(origin)?.0;
        Some(Self {
            tree,
            origin,
            environment,
            file,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn owner_type_arguments(
        workspace: &'a Workspace,
        set_file: &'a PathBuf,
        set_owner: &Symbol,
        set_param_names: &[String],
        owner_file: &Path,
        owner: &Symbol,
        is_receiver_method: bool,
        receiver_param_names: &[String],
        arena: &mut TypeArena<'a>,
        root_env: &[(&str, TypeId)],
    ) -> Option<Vec<(String, TypeId)>> {
        let mut tree = TypeTree::new(workspace);
        tree.symbolic = true;
        let input = if set_param_names.is_empty() {
            tree.push(set_file, &set_owner.name, None)?
        } else {
            let env = tree.name_environment(set_file, set_param_names)?;
            tree.push(
                set_file,
                &format!("{}[{}]", set_owner.name, set_param_names.join(",")),
                Some(env),
            )?
        };
        let kind = if is_receiver_method {
            OwnerKind::Receiver
        } else {
            OwnerKind::Aggregate
        };
        let mut environment = tree.owner_environment(input, owner_file, owner, kind)?.1;
        if is_receiver_method {
            environment = tree.renamed_environment(environment, receiver_param_names.to_vec())?;
        }
        let Some(env_id) = environment else {
            return Some(Vec::new());
        };
        let mut result = Vec::with_capacity(tree.environments[env_id].len());
        for (name, arg) in &tree.environments[env_id] {
            let ty = tree.convert_node(*arg, arena, root_env, 0)?;
            result.push((name.clone(), ty));
        }
        Some(result)
    }

    pub(crate) fn render(&self, annotation: &str) -> Option<String> {
        self.tree
            .render_text(self.origin, annotation, self.environment, self.file)
    }

    pub(crate) fn rename_parameters(&mut self, names: Vec<String>) -> Option<()> {
        self.environment = self.tree.renamed_environment(self.environment, names)?;
        Some(())
    }
}

fn bracket_end(text: &str, start: usize) -> Option<usize> {
    let mut depth = 0usize;
    for token in Lexer::new(&text[start..]) {
        match token.kind {
            TokenKind::LBrack => depth += 1,
            TokenKind::RBrack => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(start + token.span.end);
                }
            }
            _ => {}
        }
    }
    None
}
