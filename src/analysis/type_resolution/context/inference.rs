//! Bounded free-parameter discovery through aliases and dependent constraints.
use super::iteration;
use super::{Resolved, TypeContext};
use crate::analysis::scope as locals;
use crate::analysis::type_resolution::declaration_body;
use crate::syntax::{Lexer, TokenKind};
use crate::text::SourceMap;
use std::collections::HashMap;
use std::path::PathBuf;

impl<'a> TypeContext<'a> {
    pub(in crate::analysis::type_resolution) fn inferable(
        &mut self,
        file: &'a PathBuf,
        parameters: &[(&str, &str)],
        inputs: &[&str],
    ) -> Option<Vec<bool>> {
        let mut formals = HashMap::new();
        let mut bindings = Vec::new();
        for (index, (name, _)) in parameters.iter().enumerate() {
            self.bytes = self.bytes.checked_add(name.len())?;
            if self.bytes > 256 * 1024 {
                return None;
            }
            let id = self.push(file, "", None)?;
            formals.insert(id, index);
            bindings.push(((*name).to_owned(), id));
        }
        let environment = self.environments.len();
        self.environments.push(bindings);
        let mut inferred = vec![false; parameters.len()];
        for input in inputs {
            let node = self.push(file, input, Some(environment))?;
            self.infer_node(node, &formals, &mut inferred, 0)?;
        }
        for _ in 0..=parameters.len() {
            let before = inferred.clone();
            for (index, (_, constraint)) in parameters.iter().enumerate() {
                if inferred[index] {
                    let constraint = format!("interface{{{constraint}}}");
                    let node = self.push(file, &constraint, Some(environment))?;
                    self.infer_node(node, &formals, &mut inferred, 0)?;
                }
            }
            if before == inferred {
                return Some(inferred);
            }
        }
        None
    }

    fn infer_node(
        &mut self,
        input: usize,
        formals: &HashMap<usize, usize>,
        inferred: &mut [bool],
        depth: usize,
    ) -> Option<()> {
        if depth >= 32 {
            return None;
        }
        if let Some(index) = formals.get(&input) {
            inferred[*index] = true;
            return Some(());
        }
        let node = &self.nodes[input];
        let (file, environment) = (node.file, node.environment);
        let text = node.text.clone();
        if Lexer::new(&text).next()?.kind == TokenKind::Ident
            && let Some(name) = locals::named_type(&text)
        {
            if name == locals::named_root(&name)
                && let Some(argument) = self.binding(environment, &name)
            {
                return self.infer_node(argument, formals, inferred, depth + 1);
            }
            let mut arguments = Vec::new();
            for text in locals::type_arguments(&name).unwrap_or_default() {
                arguments.push(self.push(file, text, environment)?);
            }
            if arguments.is_empty() {
                return Some(());
            }
            let (origin, symbol) = self.workspace.type_declaration(file, &name)?;
            let source = self.workspace.source(origin)?;
            let map = SourceMap::new(&source);
            let declaration = source.get(symbol.end..map.offset(symbol.range.end)?)?;
            if declaration.len() > 16 * 1024 {
                return None;
            }
            let after_parameters = crate::syntax::type_parameter_end(declaration).unwrap_or(0);
            let alias =
                Lexer::new(&declaration[after_parameters..]).next()?.kind == TokenKind::Assign;
            if alias {
                let resolved = Resolved {
                    file: origin,
                    symbol,
                    arguments,
                };
                let environment = self.environment(&resolved)?;
                let next = self.push(origin, declaration_body(declaration)?, environment)?;
                drop(source);
                return self.infer_node(next, formals, inferred, depth + 1);
            }
            drop(source);
            for argument in arguments {
                self.infer_node(argument, formals, inferred, depth + 1)?;
            }
            return Some(());
        }
        let parsed = crate::syntax::parse_type(&text);
        if !parsed.diagnostics.is_empty() {
            return None;
        }
        let mut end = 0;
        for reference in parsed.references {
            let start = reference
                .qualifier
                .as_ref()
                .map_or(reference.name.start, |qualifier| qualifier.start);
            if start < end {
                continue;
            }
            end = reference.name.end;
            if let Some(token) = Lexer::new(&text[end..]).next()
                && token.kind == TokenKind::LBrack
            {
                end = iteration::bracket_end(&text, end + token.span.start)?;
            }
            let next = self.push(file, &text[start..end], environment)?;
            self.infer_node(next, formals, inferred, depth + 1)?;
        }
        Some(())
    }
}
