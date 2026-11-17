//! Complete bounded struct fields, shared by identity, substitution and relations.
use super::{MAX_BYTES, MAX_DEPTH, Tags, Ty, TypeArena, TypeId};
use super::{PromotionIndices, Selection, SelectionKind};
use crate::analysis::obj::{MemberDeclId, ObjectRef};
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
    pub(super) selection: [u32; 2],
}

impl<'a> TypeArena<'a> {
    pub(super) fn read_struct_at(
        &mut self,
        origin: &Path,
        source: &str,
        file_offset: Option<usize>,
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
            if self.fields.len() + fields.len() >= self.node_limit {
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
            let ty = self.read_at(
                origin,
                annotation,
                file_offset.map(|base| base + field.annotation.start),
                &mut tokens,
                environment,
                depth + 1,
            )?;
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
            let next_bytes = bytes + name.len() + raw_tag.map_or(0, str::len);
            // Preflight before decoding/duplicating a shared grouped-field tag.
            if next_bytes > MAX_BYTES || self.field_bytes + next_bytes > self.byte_limit {
                return None;
            }
            let tag = match raw_tag {
                Some(tag) => crate::syntax::decode_string_literal(tag)?,
                None => Vec::new(),
            };
            bytes += name.len() + tag.len();
            let selection = file_offset
                .and_then(|base| {
                    Some([
                        u32::try_from(base + field.name.start).ok()?,
                        u32::try_from(base + field.name.end).ok()?,
                    ])
                })
                .unwrap_or([0, 0]);
            fields.push(Field {
                name: name.to_owned(),
                ty,
                embedded: field.embedded,
                tag,
                selection,
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
        if self.fields.len() + fields.len() > self.node_limit
            || bytes > MAX_BYTES
            || self.field_bytes + bytes > self.byte_limit
        {
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
        if a.parent() == b.parent() {
            return Some(true);
        }
        match (self.workspace.import_path(a), self.workspace.import_path(b)) {
            (Some(pa), Some(pb)) => Some(pa == pb),
            _ if self.symbolic => Some(false),
            _ => None,
        }
    }

    pub(crate) fn literal_field(
        &mut self,
        target: TypeId,
        consumer: &Path,
        name: &str,
    ) -> Option<Selection<'a>> {
        if name == "_" {
            return None;
        }
        let unaliased = self.unalias(target)?;
        let owner = match self.nodes.get(usize::from(unaliased.0))? {
            Ty::Named { declaration, .. } => Some(declaration.symbol),
            _ => None,
        };
        let root = self.underlying(unaliased)?;
        let Ty::Struct { origin, fields } = self.nodes.get(usize::from(root.0))?.clone() else {
            return None;
        };
        if self
            .workspace
            .file_go_version(consumer)
            .is_some_and(|version| version >= 27)
        {
            let mut selection = self.lookup_field_or_method(root, false, consumer, name)??;
            if !matches!(selection.obj, ObjectRef::Field(_)) || selection.indirect {
                return None;
            }
            selection.recv = target;
            return Some(selection);
        }
        let same_pkg =
            crate::syntax::is_exported_name(name) || self.same_field_package(origin, consumer)?;
        if !same_pkg {
            return None;
        }
        for (ordinal, field) in self.fields[fields].iter().enumerate() {
            if field.name == name {
                return Some(Selection {
                    kind: SelectionKind::FieldVal,
                    recv: target,
                    obj: ObjectRef::Field(MemberDeclId {
                        file: origin,
                        owner,
                        selection: field.selection,
                        embedded: field.embedded,
                        builtin: false,
                    }),
                    ty: field.ty,
                    indices: PromotionIndices::from_slice(&[u16::try_from(ordinal).ok()?])?,
                    indirect: false,
                });
            }
        }
        None
    }

    /// Direct field suggestions retain the checked field's identity and type.
    /// Promoted literal keys are admitted separately by the file language view.
    pub(crate) fn literal_fields(
        &mut self,
        target: TypeId,
        consumer: &Path,
    ) -> Option<Vec<(String, Selection<'a>)>> {
        let unaliased = self.unalias(target)?;
        let owner = self
            .named_declaration(unaliased)
            .map(|(decl, _, _)| decl.symbol);
        let root = self.underlying(unaliased)?;
        let Ty::Struct { origin, fields } = self.nodes.get(usize::from(root.0))?.clone() else {
            return None;
        };
        if self
            .workspace
            .file_go_version(consumer)
            .is_some_and(|version| version >= 27)
        {
            let mut members = self.accessible_members(root, false, consumer)?;
            members.retain(|(_, selection)| {
                matches!(selection.obj, ObjectRef::Field(_)) && !selection.indirect
            });
            for (_, selection) in &mut members {
                selection.recv = target;
            }
            members.sort_by(|(_, a), (_, b)| {
                a.indices
                    .as_slice()
                    .len()
                    .cmp(&b.indices.as_slice().len())
                    .then_with(|| a.indices.as_slice().cmp(b.indices.as_slice()))
            });
            return Some(members);
        }
        let mut names = 0usize;
        let mut result = Vec::new();
        for (ordinal, index) in fields.enumerate() {
            self.step()?;
            let field = &self.fields[index];
            if field.name == "_"
                || !crate::syntax::is_exported_name(&field.name)
                    && !self.same_field_package(origin, consumer)?
            {
                continue;
            }
            names = names.checked_add(field.name.len())?;
            if names > MAX_BYTES {
                return None;
            }
            result.push((
                field.name.clone(),
                Selection {
                    kind: SelectionKind::FieldVal,
                    recv: target,
                    obj: ObjectRef::Field(MemberDeclId {
                        file: origin,
                        owner,
                        selection: field.selection,
                        embedded: field.embedded,
                        builtin: false,
                    }),
                    ty: field.ty,
                    indices: PromotionIndices::from_slice(&[u16::try_from(ordinal).ok()?])?,
                    indirect: false,
                },
            ));
        }
        Some(result)
    }

    pub(crate) fn struct_positional_fields(
        &mut self,
        target: TypeId,
        consumer: &Path,
    ) -> Option<Vec<TypeId>> {
        let root = self.underlying(target)?;
        let Ty::Struct { origin, fields } = self.nodes.get(usize::from(root.0))?.clone() else {
            return None;
        };
        let mut out = Vec::with_capacity(fields.len());
        let mut same_pkg = None;
        for field in &self.fields[fields] {
            if !crate::syntax::is_exported_name(&field.name) {
                let allowed = match same_pkg {
                    Some(v) => v,
                    None => {
                        let v = self.same_field_package(origin, consumer)?;
                        same_pkg = Some(v);
                        v
                    }
                };
                if !allowed {
                    return None;
                }
            }
            out.push(field.ty);
        }
        Some(out)
    }

    pub(crate) fn is_struct(&mut self, target: TypeId) -> Option<bool> {
        let root = self.underlying(target)?;
        Some(matches!(
            self.nodes.get(usize::from(root.0))?,
            Ty::Struct { .. }
        ))
    }

    pub(crate) fn composite_container_types(
        &mut self,
        target: TypeId,
    ) -> Option<(Option<TypeId>, TypeId)> {
        let root = self.underlying(target)?;
        match self.nodes.get(usize::from(root.0))? {
            Ty::Slice(element) | Ty::Array { element, .. } => Some((None, *element)),
            Ty::Map { key, value } => Some((Some(*key), *value)),
            _ => None,
        }
    }

    pub(crate) fn elided_composite_element_type(&mut self, ty: TypeId) -> Option<TypeId> {
        let under = self.underlying(ty)?;
        if let Ty::Pointer(inner) = self.nodes.get(usize::from(under.0))?.clone() {
            let inner_under = self.underlying(inner)?;
            if matches!(
                self.nodes.get(usize::from(inner_under.0))?,
                Ty::Struct { .. } | Ty::Array { .. } | Ty::Slice(_) | Ty::Map { .. }
            ) {
                return Some(inner);
            }
            return None;
        }
        if matches!(
            self.nodes.get(usize::from(under.0))?,
            Ty::Struct { .. } | Ty::Array { .. } | Ty::Slice(_) | Ty::Map { .. }
        ) {
            Some(ty)
        } else {
            None
        }
    }
}
