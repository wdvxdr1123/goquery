//! Resolve only the outer constructor, retaining scoped generic arguments.
use super::{Resolved, TypeContext};
use crate::analysis::scope as locals;
use crate::analysis::types::{TypeForm, declaration_body};

use crate::syntax::{Lexer, TokenKind};

impl<'a> TypeContext<'a> {
    pub(in crate::analysis::type_resolution) fn form(
        &mut self,
        input: usize,
        depth: usize,
    ) -> Option<TypeForm> {
        if depth >= 64 {
            return None;
        }
        let node = &self.nodes[input];
        let (file, environment, argument) = (node.file, node.environment, node.argument);
        if let Some(form) = TypeForm::constructor(&node.text) {
            return Some(form);
        }
        if Lexer::new(&node.text).next()?.kind == TokenKind::LParen {
            let first = Lexer::new(&node.text).next()?;
            let last = Lexer::new(&node.text)
                .filter(|token| token.kind != TokenKind::Semicolon)
                .last()?;
            if last.kind != TokenKind::RParen {
                return None;
            }
            let text = node.text.get(first.span.end..last.span.start)?.to_owned();
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
        let Some((origin, symbol)) = self.workspace.type_declaration(file, &name) else {
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
            // Drop each closed source before expanding the next named type.
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
            // A declaration such as Forward[A,B] = Box[B] has a known
            // outer struct shape even before its arguments are supplied.
            // Empty nodes keep parameter-dependent roots explicitly unknown.
            for _ in self.type_parameters(resolved.file, resolved.symbol)? {
                resolved.arguments.push(self.push(resolved.file, "", None)?);
            }
        }
        self.environment(resolved)
    }
}
