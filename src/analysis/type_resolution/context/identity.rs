//! Scoped signature fingerprints: aliases are transparent, defined types nominal.
use super::{Resolved, TypeContext};
use crate::analysis::scope as locals;
use crate::analysis::type_resolution::signature;
use crate::syntax::{Lexer, TokenKind};
use std::path::PathBuf;

impl<'a> TypeContext<'a> {
    pub(in crate::analysis::type_resolution) fn implementation_input(
        &mut self,
        file: &'a PathBuf,
        owner: &crate::index::IndexedSymbol,
    ) -> Option<usize> {
        self.symbolic = true;
        let names = self.type_parameters(file, owner)?;
        if names.is_empty() {
            return self.push(file, &owner.name, None);
        }
        let environment = self.name_environment(file, &names)?;
        self.push(
            file,
            &format!("{}[{}]", owner.name, names.join(",")),
            Some(environment),
        )
    }

    pub(in crate::analysis::type_resolution) fn signature_identity(
        &mut self,
        file: &'a PathBuf,
        header: &str,
        environment: Option<usize>,
        depth: usize,
    ) -> Option<String> {
        if depth >= 32 || header.len() > 16 * 1024 {
            return None;
        }
        let signature = crate::syntax::parse_literal_signature(header);
        if !signature.diagnostics.is_empty() || !signature.type_parameters.is_empty() {
            return None;
        }
        let mut parameters = Vec::new();
        let mut results = Vec::new();
        let mut bytes = 0usize;
        for (items, output) in [
            (&signature.parameters, &mut parameters),
            (&signature.results, &mut results),
        ] {
            for parameter in items {
                let node = self.push(file, &header[parameter.annotation.clone()], environment)?;
                let identity = self.identity(node, depth + 1)?;
                bytes = bytes.checked_add(identity.len() + 24)?;
                if bytes > 16 * 1024 {
                    return None;
                }
                output.push(signature::pack(
                    if parameter.variadic {
                        "variadic"
                    } else {
                        "type"
                    },
                    &[identity],
                )?);
            }
        }
        signature::pack(
            "func",
            &[
                signature::pack("in", &parameters)?,
                signature::pack("out", &results)?,
            ],
        )
    }

    pub(super) fn identity(&mut self, input: usize, depth: usize) -> Option<String> {
        if depth >= 32 {
            return None;
        }
        if self.nodes[input].formal {
            return self
                .symbolic
                .then(|| signature::pack("parameter", &[input.to_string()]))
                .flatten();
        }
        let node = &self.nodes[input];
        let (file, environment) = (node.file, node.environment);
        let text = node.text.clone();
        let parsed = crate::syntax::parse_type(&text);
        if !parsed.diagnostics.is_empty() {
            return None;
        }
        let mut tokens = Lexer::new(&text);
        let first = tokens.next()?;
        match first.kind {
            TokenKind::LParen => {
                let last = tokens
                    .filter(|token| token.kind != TokenKind::Semicolon)
                    .last()?;
                let node = self.push(
                    file,
                    text.get(first.span.end..last.span.start)?,
                    environment,
                )?;
                self.identity(node, depth + 1)
            }
            TokenKind::Mul => {
                let node = self.push(file, &text[first.span.end..], environment)?;
                signature::pack("ptr", &[self.identity(node, depth + 1)?])
            }
            TokenKind::LBrack => {
                let end = super::iteration::bracket_end(&text, first.span.start)?;
                let length = text.get(first.span.end..end - 1)?.trim();
                let node = self.push(file, &text[end..], environment)?;
                let element = self.identity(node, depth + 1)?;
                if length.is_empty() {
                    signature::pack("slice", &[element])
                } else {
                    signature::pack(
                        "array",
                        &[
                            signature::pack("atom", &[integer(length)?.to_string()])?,
                            element,
                        ],
                    )
                }
            }
            TokenKind::Map => {
                let opening = tokens.next()?;
                let end = super::iteration::bracket_end(&text, opening.span.start)?;
                let key = self.push(file, &text[opening.span.end..end - 1], environment)?;
                let value = self.push(file, &text[end..], environment)?;
                signature::pack(
                    "map",
                    &[
                        self.identity(key, depth + 1)?,
                        self.identity(value, depth + 1)?,
                    ],
                )
            }
            TokenKind::Chan | TokenKind::Arrow => {
                let next = tokens.next()?;
                let (direction, start) = if first.kind == TokenKind::Arrow {
                    ("recv", next.span.end)
                } else if next.kind == TokenKind::Arrow {
                    ("send", next.span.end)
                } else {
                    ("both", first.span.end)
                };
                let element = self.push(file, &text[start..], environment)?;
                signature::pack(direction, &[self.identity(element, depth + 1)?])
            }
            TokenKind::Func => self.signature_identity(file, &text, environment, depth + 1),
            TokenKind::Struct => self.struct_identity(file, &text, &parsed, environment, depth + 1),
            TokenKind::Interface => self.interface_identity(input, depth + 1),
            TokenKind::Ident => {
                let name = locals::named_type(&text)?;
                if name == locals::named_root(&name)
                    && let Some(argument) = self.binding(environment, &name)
                {
                    return self.identity(argument, depth + 1);
                }
                let Some((origin, symbol)) = self.workspace.type_declaration(file, &name) else {
                    return signature::pack("atom", &[builtin(&name)?.to_owned()]);
                };
                let mut arguments = Vec::new();
                for argument in locals::type_arguments(&name).unwrap_or_default() {
                    arguments.push(self.push(file, argument, environment)?);
                }
                let resolved = Resolved {
                    file: origin,
                    symbol,
                    arguments,
                };
                // Aliases expand; defined types stay nominal without reading
                // their source merely to rediscover the declaration category.
                if symbol.alias {
                    let span = symbol.end..symbol.declaration()?.end;
                    let source;
                    let declaration = if span.len() <= 16 * 1024 {
                        source = self.workspace.source_slice(origin, span)?;
                        source.as_str()
                    } else {
                        // Large trivia before '=' is not part of the aliased
                        // type's existing 16KiB bound. Preserve that acceptance.
                        source = self.workspace.source(origin)?;
                        source.get(span)?
                    };
                    let after_parameters =
                        crate::syntax::type_parameter_end(declaration).unwrap_or(0);
                    let first = Lexer::new(&declaration[after_parameters..]).next()?;
                    let environment = self.environment(&resolved)?;
                    let body = declaration.get(after_parameters + first.span.end..)?;
                    let next = self.push(origin, body, environment)?;
                    drop(source);
                    return self.identity(next, depth + 1);
                }
                // Arity validation also guards an uninstantiated generic name.
                self.environment(&resolved)?;
                // A lossy path spelling could merge two different declarations.
                let mut parts = vec![signature::pack(
                    "atom",
                    &[format!("{}:{}", origin.to_str()?, symbol.start)],
                )?];
                let mut bytes = parts[0].len();
                for argument in resolved.arguments {
                    let argument = self.identity(argument, depth + 1)?;
                    bytes = bytes.checked_add(argument.len() + 24)?;
                    if bytes > 16 * 1024 {
                        return None;
                    }
                    parts.push(argument);
                }
                signature::pack("named", &parts)
            }
            // Array constant expressions need evaluation; spelling is not identity.
            _ => None,
        }
    }
}

fn integer(text: &str) -> Option<u128> {
    if Lexer::new(text)
        .filter(|token| token.kind != TokenKind::Semicolon)
        .count()
        != 1
    {
        return None;
    }
    let digits = text.replace('_', "");
    let (radix, body) = if digits.starts_with("0x") || digits.starts_with("0X") {
        (16, &digits[2..])
    } else if digits.starts_with("0b") || digits.starts_with("0B") {
        (2, &digits[2..])
    } else if digits.starts_with("0o") || digits.starts_with("0O") {
        (8, &digits[2..])
    } else if digits.starts_with('0') && digits.len() > 1 {
        (8, &digits[1..])
    } else {
        (10, digits.as_str())
    };
    u128::from_str_radix(body, radix).ok()
}

fn builtin(name: &str) -> Option<&str> {
    Some(match name {
        "byte" => "uint8",
        "rune" => "int32",
        "bool" | "string" | "int" | "int8" | "int16" | "int32" | "int64" | "uint" | "uint8"
        | "uint16" | "uint32" | "uint64" | "uintptr" | "float32" | "float64" | "complex64"
        | "complex128" | "any" | "error" => name,
        _ => return None,
    })
}
