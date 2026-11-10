//! Complete bounded struct fields, shared by identity, substitution and relations.
use super::arena::DisplayScope;
use super::{MAX_BYTES, MAX_DEPTH, MAX_NODES, Tags, Ty, TypeArena, TypeId};
use crate::analysis::types::declaration_body;
use crate::syntax::{Lexer, TokenKind};
use std::ops::Range;
use std::path::Path;

#[derive(Clone)]
pub(super) struct Field {
    pub(super) name: String,
    pub(super) ty: TypeId,
    pub(super) embedded: bool,
    pub(super) tag: Vec<u8>,
}

impl<'a> TypeArena<'a> {
    pub(super) fn read_struct(
        &mut self,
        origin: &Path,
        source: &str,
        environment: &[(&str, TypeId)],
        depth: usize,
    ) -> Option<TypeId> {
        let parsed = crate::syntax::parse_type(source);
        if !parsed.diagnostics.is_empty() {
            return None;
        }
        let brace = Lexer::new(source).nth(1)?.span.start;
        let origin = self.workspace.files.get_key_value(origin)?.0.as_path();
        let mut fields = Vec::new();
        let mut bytes = 0;
        for field in parsed
            .fields
            .iter()
            .filter(|field| field.container == brace)
        {
            if self.fields.len() + fields.len() >= MAX_NODES {
                return None;
            }
            let annotation = &source[field.annotation.clone()];
            if field.embedded {
                let mut root = Lexer::new(annotation).filter(|token| token.kind != TokenKind::Mul);
                let name = root.next()?;
                if name.kind == TokenKind::Ident
                    && root
                        .next()
                        .is_none_or(|token| token.kind != TokenKind::Period)
                    && environment
                        .iter()
                        .any(|(parameter, _)| *parameter == &annotation[name.span.clone()])
                {
                    return None;
                }
            }
            let mut tokens = Lexer::new(annotation).peekable();
            let ty = self.read(origin, annotation, &mut tokens, environment, depth + 1)?;
            if !tokens.all(|token| token.kind == TokenKind::Semicolon) {
                return None;
            }
            let name = &source[field.name.clone()];
            if name != "_" && fields.iter().any(|field: &Field| field.name == name) {
                return None;
            }
            if field.embedded {
                let ty = self.unalias(ty)?;
                let (base, pointer) = match self.nodes[usize::from(ty.0)] {
                    Ty::Pointer(element) => (self.unalias(element)?, true),
                    _ => (ty, false),
                };
                self.embedded_base(base, pointer, 0)?;
            }
            let raw_tag = field.tag.as_ref().map(|span| &source[span.clone()]);
            // Preflight before decoding/duplicating a shared grouped-field tag.
            if self.field_bytes + bytes + name.len() + raw_tag.map_or(0, str::len) > MAX_BYTES {
                return None;
            }
            let tag = match raw_tag {
                Some(tag) => crate::syntax::decode_string_literal(tag)?,
                None => Vec::new(),
            };
            bytes += name.len() + tag.len();
            fields.push(Field {
                name: name.to_owned(),
                ty,
                embedded: field.embedded,
                tag,
            });
        }
        self.store_struct(origin, fields)
    }

    // Match go/types' single dereference followed by underlying classification.
    // Reading only the head avoids expanding the fields of a recursive *Node.
    fn embedded_base(&mut self, ty: TypeId, pointer: bool, depth: usize) -> Option<()> {
        self.step()?;
        if depth >= MAX_DEPTH {
            return None;
        }
        let ty = self.unalias(ty)?;
        match &self.nodes[usize::from(ty.0)] {
            Ty::Parameter(_) | Ty::Pointer(_) => None,
            Ty::Named { declaration, .. } => {
                let (file, symbol) = (declaration.file, declaration.symbol);
                if let Some(info) = &symbol.type_info {
                    if info.is_interface() {
                        return (!pointer).then_some(());
                    }
                    if info.target.is_none() {
                        return Some(());
                    }
                }
                let source = self
                    .workspace
                    .source_slice(file, symbol.end..symbol.declaration()?.end)?;
                let body = declaration_body(&source)?;
                let head = Lexer::new(body)
                    .find(|token| token.kind != TokenKind::LParen)?
                    .kind;
                match head {
                    TokenKind::Mul => None,
                    TokenKind::Interface => (!pointer).then_some(()),
                    TokenKind::Ident => {
                        let target = self.expand(ty)?;
                        self.embedded_base(target, pointer, depth + 1)
                    }
                    TokenKind::Struct
                    | TokenKind::Func
                    | TokenKind::Map
                    | TokenKind::Chan
                    | TokenKind::Arrow
                    | TokenKind::LBrack => Some(()),
                    _ => None,
                }
            }
            _ => Some(()),
        }
    }

    pub(super) fn store_struct(&mut self, origin: &'a Path, fields: Vec<Field>) -> Option<TypeId> {
        let bytes: usize = fields
            .iter()
            .map(|field| field.name.len() + field.tag.len())
            .sum();
        if self.fields.len() + fields.len() > MAX_NODES || self.field_bytes + bytes > MAX_BYTES {
            return None;
        }
        self.field_bytes += bytes;
        let start = self.fields.len();
        self.fields.extend(fields);
        self.push(Ty::Struct {
            origin,
            fields: start..self.fields.len(),
        })
    }

    pub(super) fn same_struct_fields(
        &self,
        a: &Path,
        aa: Range<usize>,
        b: &Path,
        ba: Range<usize>,
    ) -> Option<bool> {
        self.struct_fields_with_tags(a, aa, b, ba, Tags::Include)
    }

    pub(super) fn struct_fields_with_tags(
        &self,
        a: &Path,
        aa: Range<usize>,
        b: &Path,
        ba: Range<usize>,
        tags: Tags,
    ) -> Option<bool> {
        if aa.len() != ba.len() {
            return Some(false);
        }
        for (a_field, b_field) in self.fields[aa].iter().zip(&self.fields[ba]) {
            if a_field.name != b_field.name
                || a_field.embedded != b_field.embedded
                || tags == Tags::Include && a_field.tag != b_field.tag
            {
                return Some(false);
            }
            if !crate::syntax::is_exported_name(&a_field.name) && !self.same_field_package(a, b)? {
                return Some(false);
            }
        }
        Some(true)
    }

    pub(super) fn same_field_package(&self, a: &Path, b: &Path) -> Option<bool> {
        if self.workspace.files.get(a)?.package != self.workspace.files.get(b)?.package {
            return Some(false);
        }
        Some(
            a.parent() == b.parent()
                || self.workspace.import_path(a)? == self.workspace.import_path(b)?,
        )
    }

    pub(super) fn write_struct(
        &self,
        origin: &Path,
        fields: Range<usize>,
        consumer: DisplayScope<'_>,
        out: &mut String,
        depth: usize,
    ) -> Option<()> {
        if self.fields[fields.clone()]
            .iter()
            .any(|field| !crate::syntax::is_exported_name(&field.name))
            && let DisplayScope::Consumer(file) = consumer
            && !self.same_field_package(origin, file)?
        {
            return None;
        }
        out.push_str("struct { ");
        for field in &self.fields[fields] {
            if !field.embedded {
                out.push_str(&field.name);
                out.push(' ');
            }
            let start = out.len();
            self.write_in(field.ty, consumer, out, depth + 1)?;
            if field.embedded {
                let mut name = None;
                for token in Lexer::new(&out[start..]) {
                    if token.kind == TokenKind::LBrack {
                        break;
                    }
                    if token.kind == TokenKind::Ident {
                        name = Some(&out[start + token.span.start..start + token.span.end]);
                    }
                }
                if name != Some(field.name.as_str()) {
                    return None;
                }
            }
            if !field.tag.is_empty() {
                out.push_str(" \"");
                const HEX: &[u8] = b"0123456789abcdef";
                for byte in &field.tag {
                    if out.len() + 4 > MAX_BYTES {
                        return None;
                    }
                    out.push_str("\\x");
                    out.push(char::from(HEX[usize::from(byte >> 4)]));
                    out.push(char::from(HEX[usize::from(byte & 15)]));
                }
                out.push('"');
            }
            out.push_str("; ");
        }
        out.push('}');
        (out.len() <= MAX_BYTES).then_some(())
    }
}
