//! Complete bounded struct fields, shared by identity, substitution and relations.
use super::lower::ConstExprEval;
use super::{
    CheckOutcome, MAX_BYTES, MAX_DEPTH, PromotionIndices, Selection, SelectionKind, Tags, Ty,
    TypeArena, TypeId, UnknownReason,
};
use crate::analysis::engine::obj::{MemberDeclId, ObjectRef};
use crate::analysis::engine::types::declaration_body;
use crate::syntax::{Lexer, TokenKind, TypeNodeId, TypeSyntax, TypeTreeKind};
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
    #[allow(clippy::too_many_arguments)]
    pub(super) fn lower_struct_fields_checked(
        &mut self,
        origin: &Path,
        source: &str,
        parsed: &TypeSyntax,
        fields_range: Range<u32>,
        file_offset: Option<usize>,
        environment: &[(&str, TypeId)],
        const_eval: ConstExprEval<'_, 'a>,
        depth: usize,
    ) -> CheckOutcome<TypeId> {
        if depth >= MAX_DEPTH {
            return CheckOutcome::Unknown(UnknownReason::BudgetExceeded);
        }
        let Some(origin) = self
            .workspace
            .files
            .get_key_value(origin)
            .map(|(p, _)| p.as_path())
        else {
            return CheckOutcome::Unknown(UnknownReason::MissingMetadata);
        };
        let mut fields = Vec::new();
        let mut bytes = 0;
        for field in parsed.struct_fields(fields_range) {
            if fields.len() >= self.active_node_limit() {
                return CheckOutcome::Unknown(UnknownReason::BudgetExceeded);
            }
            if field.embedded
                && self.is_embedded_env_type_param(source, parsed, field.ty, environment)
            {
                return CheckOutcome::Unknown(UnknownReason::Unsupported);
            }
            let ty = match self.lower_type_tree_node(
                origin,
                source,
                parsed,
                field.ty,
                file_offset,
                environment,
                const_eval,
                depth + 1,
            ) {
                CheckOutcome::Ok(ty) => ty,
                CheckOutcome::Invalid(()) => {
                    return CheckOutcome::Unknown(UnknownReason::Unsupported);
                }
                CheckOutcome::Unknown(reason) => return CheckOutcome::Unknown(reason),
            };
            let Some(name_span) = &field.name else {
                return CheckOutcome::Unknown(UnknownReason::Unsupported);
            };
            let name_range = name_span.start as usize..name_span.end as usize;
            let Some(name) = source.get(name_range.clone()) else {
                return CheckOutcome::Unknown(UnknownReason::SyntaxError);
            };
            if name != "_" && fields.iter().any(|existing: &Field| existing.name == name) {
                return CheckOutcome::Unknown(UnknownReason::Unsupported);
            }
            if field.embedded {
                let Some(unaliased) = self.unalias(ty) else {
                    return CheckOutcome::Unknown(UnknownReason::Unsupported);
                };
                let Some((base, pointer)) = (match self.node(unaliased) {
                    Some(Ty::Pointer(element)) => self.unalias(*element).map(|b| (b, true)),
                    Some(_) => Some((unaliased, false)),
                    None => None,
                }) else {
                    return CheckOutcome::Unknown(UnknownReason::Unsupported);
                };
                if self.embedded_base(base, pointer, 0).is_none() {
                    return CheckOutcome::Unknown(UnknownReason::Unsupported);
                }
            }
            let raw_tag = match &field.tag {
                Some(span) => match source.get(span.start as usize..span.end as usize) {
                    Some(text) => Some(text),
                    None => return CheckOutcome::Unknown(UnknownReason::SyntaxError),
                },
                None => None,
            };
            let next_bytes = bytes + name.len() + raw_tag.map_or(0, str::len);
            // Preflight before decoding/duplicating a shared grouped-field tag.
            if next_bytes > MAX_BYTES {
                return CheckOutcome::Unknown(UnknownReason::BudgetExceeded);
            }
            let tag = match raw_tag {
                Some(tag) => match crate::syntax::decode_string_literal(tag) {
                    Some(decoded) => decoded,
                    None => return CheckOutcome::Unknown(UnknownReason::SyntaxError),
                },
                None => Vec::new(),
            };
            bytes += name.len() + tag.len();
            let selection = file_offset
                .and_then(|base| {
                    Some([
                        u32::try_from(base + name_range.start).ok()?,
                        u32::try_from(base + name_range.end).ok()?,
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
        match self.store_struct(origin, fields) {
            Some(id) => CheckOutcome::Ok(id),
            None => CheckOutcome::Unknown(UnknownReason::BudgetExceeded),
        }
    }

    fn is_embedded_env_type_param(
        &self,
        source: &str,
        parsed: &TypeSyntax,
        mut node_id: TypeNodeId,
        environment: &[(&str, TypeId)],
    ) -> bool {
        for _ in 0..MAX_DEPTH {
            let Some(node) = parsed.tree_node(node_id) else {
                return false;
            };
            match &node.kind {
                TypeTreeKind::Pointer(inner) | TypeTreeKind::Parenthesized(inner) => {
                    node_id = *inner;
                }
                TypeTreeKind::Name { span } => {
                    let Some(name) = source.get(span.start as usize..span.end as usize) else {
                        return false;
                    };
                    return environment.iter().any(|(parameter, ty)| {
                        *parameter == name && matches!(self.node(*ty), Some(Ty::TypeParam(_)))
                    });
                }
                _ => return false,
            }
        }
        false
    }

    // Match go/types' single dereference followed by underlying classification.
    // Reading only the head avoids expanding the fields of a recursive *Node.
    fn embedded_base(&mut self, ty: TypeId, pointer: bool, depth: usize) -> Option<()> {
        self.step()?;
        if depth >= MAX_DEPTH {
            return None;
        }
        let ty = self.unalias(ty)?;
        if let Some((declaration, _, _)) = self.named_info(ty) {
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
            return match head {
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
            };
        }
        match *self.node(ty)? {
            Ty::TypeParam(_) | Ty::Pointer(_) => None,
            _ => Some(()),
        }
    }

    pub(super) fn store_struct(&mut self, origin: &'a Path, fields: Vec<Field>) -> Option<TypeId> {
        self.store_struct_nodes(origin, fields)
    }

    pub(super) fn same_struct_fields(
        &self,
        a: (&Path, TypeId, Range<usize>),
        b: (&Path, TypeId, Range<usize>),
    ) -> Option<bool> {
        self.struct_fields_with_tags(a, b, Tags::Include)
    }

    pub(super) fn struct_fields_with_tags(
        &self,
        (a, left, aa): (&Path, TypeId, Range<usize>),
        (b, right, ba): (&Path, TypeId, Range<usize>),
        tags: Tags,
    ) -> Option<bool> {
        if aa.len() != ba.len() {
            return Some(false);
        }
        let a_fields = self.fields_slice(left, aa)?;
        let b_fields = self.fields_slice(right, ba)?;
        for (a_field, b_field) in a_fields.iter().zip(b_fields) {
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
            _ if self.mode.is_symbolic() => Some(false),
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
        let owner = self.named_info(unaliased).map(|(decl, _, _)| decl.symbol);
        let root = self.underlying(unaliased)?;
        let (origin, fields) = {
            let data = self.struct_data(root)?;
            (data.origin, data.fields.clone())
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
        for (ordinal, field) in self.fields_slice(root, fields)?.iter().enumerate() {
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
        let (origin, fields) = {
            let data = self.struct_data(root)?;
            (data.origin, data.fields.clone())
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
        let field_count = fields.len();
        for ordinal in 0..field_count {
            self.step()?;
            let field = &self.fields_slice(root, fields.clone())?[ordinal];
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
        let (origin, fields) = {
            let data = self.struct_data(root)?;
            (data.origin, data.fields.clone())
        };
        let mut out = Vec::with_capacity(fields.len());
        let mut same_pkg = None;
        for field in self.fields_slice(root, fields)? {
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
        Some(matches!(self.node(root)?, Ty::Struct { .. }))
    }

    pub(crate) fn composite_container_types(
        &mut self,
        target: TypeId,
    ) -> Option<(Option<TypeId>, TypeId)> {
        let root = self.underlying(target)?;
        match self.node(root)? {
            Ty::Slice(element) | Ty::Array { element, .. } => Some((None, *element)),
            Ty::Map { key, value } => Some((Some(*key), *value)),
            _ => None,
        }
    }

    pub(crate) fn elided_composite_element_type(&mut self, ty: TypeId) -> Option<TypeId> {
        let under = self.underlying(ty)?;
        if let Ty::Pointer(inner) = *self.node(under)? {
            let inner_under = self.underlying(inner)?;
            if matches!(
                self.node(inner_under)?,
                Ty::Struct { .. } | Ty::Array { .. } | Ty::Slice(_) | Ty::Map { .. }
            ) {
                return Some(inner);
            }
            return None;
        }
        if matches!(
            self.node(under)?,
            Ty::Struct { .. } | Ty::Array { .. } | Ty::Slice(_) | Ty::Map { .. }
        ) {
            Some(ty)
        } else {
            None
        }
    }
}
