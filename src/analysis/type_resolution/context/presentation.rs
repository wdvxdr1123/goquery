//! Underlying declaration forms, rendered without retaining source or type trees.
use super::{Resolved, TypeContext};
use crate::analysis::scope as locals;
use crate::analysis::types::{TypeForm, TypePresentation, declaration_body};

use crate::index::IndexedSymbol as Symbol;
use crate::syntax::{Lexer, TokenKind};
use crate::text::SourceMap;
use std::path::{Path, PathBuf};

impl<'a> TypeContext<'a> {
    pub(super) fn name_environment(
        &mut self,
        file: &'a PathBuf,
        names: &[String],
    ) -> Option<usize> {
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

    pub(in crate::analysis::type_resolution) fn declaration_presentation(
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

    pub(in crate::analysis::type_resolution) fn local_presentation(
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
            if let Some((origin, symbol)) = self.workspace.type_declaration(file, &name) {
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
        let source = self.workspace.source(origin)?;
        let map = SourceMap::new(&source);
        let declaration = source.get(symbol.end..map.offset(symbol.range.end)?)?;
        if declaration.len() > 16 * 1024 {
            return None;
        }
        self.push(origin, declaration_body(declaration)?, environment)
    }
}
