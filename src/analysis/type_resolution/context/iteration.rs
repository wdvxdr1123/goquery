//! Scoped range projections; generic arguments remain arena nodes until rendering.
use super::{Resolved, TypeContext};
use crate::analysis::scope as locals;
use crate::analysis::type_resolution::declaration_body;
use crate::syntax::{Lexer, TokenKind};
use crate::text::SourceMap;
use std::path::Path;

#[derive(Clone, Copy)]
struct Slot {
    index: usize,
    count: usize,
    original: usize,
    pointer: bool,
}

impl TypeContext<'_> {
    pub(in crate::analysis::type_resolution) fn range_type(
        &mut self,
        input: usize,
        index: usize,
        count: usize,
        file: &Path,
    ) -> Option<String> {
        self.nodes[input].argument = true;
        self.range_slot(
            input,
            Slot {
                index,
                count,
                original: input,
                pointer: false,
            },
            file,
            0,
        )
    }

    fn range_slot(
        &mut self,
        input: usize,
        slot: Slot,
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
                return self.range_slot(next, slot, consumer, depth + 1);
            }
            TokenKind::Mul if !slot.pointer => {
                let next = self.push(file, &text[first.span.end..], environment)?;
                self.nodes[next].argument = argument;
                return self.range_slot(
                    next,
                    Slot {
                        pointer: true,
                        ..slot
                    },
                    consumer,
                    depth + 1,
                );
            }
            TokenKind::LBrack => {
                let end = bracket_end(&text, first.span.start)?;
                let slice = Lexer::new(&text[first.span.end..end]).count() == 1;
                if slot.pointer && slice {
                    return None;
                }
                if slot.index == 0 {
                    return Some("int".to_owned());
                }
                &text[end..]
            }
            TokenKind::Map if !slot.pointer => {
                let open = tokens.next()?;
                if open.kind != TokenKind::LBrack {
                    return None;
                }
                let end = bracket_end(&text, open.span.start)?;
                if slot.index == 0 {
                    &text[open.span.end..end - 1]
                } else {
                    &text[end..]
                }
            }
            TokenKind::Chan | TokenKind::Arrow if !slot.pointer && slot.count == 1 => {
                let next = tokens.next()?;
                if first.kind == TokenKind::Arrow {
                    if next.kind != TokenKind::Chan {
                        return None;
                    }
                    &text[next.span.end..]
                } else {
                    if next.kind == TokenKind::Arrow {
                        return None;
                    }
                    &text[first.span.end..]
                }
            }
            TokenKind::Func if !slot.pointer => {
                let signature = crate::syntax::parse_literal_signature(&text);
                if !signature.diagnostics.is_empty()
                    || signature.parameters.len() != 1
                    || signature.parameters[0].variadic
                    || !signature.results.is_empty()
                {
                    return None;
                }
                let yield_type = text.get(signature.parameters[0].annotation.clone())?;
                let yield_signature = crate::syntax::parse_literal_signature(yield_type);
                if !yield_signature.diagnostics.is_empty()
                    || yield_signature.results.len() != 1
                    || slot.count > yield_signature.parameters.len()
                    || yield_signature.parameters.len() > 2
                    || yield_signature
                        .parameters
                        .iter()
                        .any(|parameter| parameter.variadic)
                    || yield_type
                        .get(yield_signature.results[0].annotation.clone())?
                        .trim()
                        != "bool"
                {
                    return None;
                }
                yield_type.get(
                    yield_signature
                        .parameters
                        .get(slot.index)?
                        .annotation
                        .clone(),
                )?
            }
            TokenKind::Ident => {
                let name = locals::named_type(&text)?;
                if name == locals::named_root(&name)
                    && let Some(next) = self.binding(environment, &name)
                {
                    return self.range_slot(next, slot, consumer, depth + 1);
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
                    if slot.pointer {
                        return None;
                    }
                    return match name.as_str() {
                        "string" => Some(if slot.index == 0 { "int" } else { "rune" }.to_owned()),
                        "byte" | "rune" | "int" | "int8" | "int16" | "int32" | "int64" | "uint"
                        | "uint8" | "uint16" | "uint32" | "uint64" | "uintptr"
                            if slot.count == 1 =>
                        {
                            self.render(slot.original, consumer, 0)
                        }
                        _ => None,
                    };
                };
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
                    let source = self.workspace.source(origin)?;
                    let map = SourceMap::new(&source);
                    let declaration = source.get(symbol.end..map.offset(symbol.range.end)?)?;
                    if declaration.len() > 16 * 1024 {
                        return None;
                    }
                    self.push(origin, declaration_body(declaration)?, environment)?
                };
                return self.range_slot(next, slot, consumer, depth + 1);
            }
            _ => return None,
        };
        self.render_text(file, projected, environment, consumer)
    }
}

pub(super) fn bracket_end(text: &str, start: usize) -> Option<usize> {
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
