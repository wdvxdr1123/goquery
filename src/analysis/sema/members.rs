//! Presentation projections of proven native member selections.
use super::Semantics;
use crate::analysis::engine::obj::{DeclId, MemberDeclId, MethodOrigin, ObjectRef};
use crate::analysis::engine::types::{Qualifier, Selection, SelectionKind, TypeArena, TypeId};
use std::path::Path;

pub(crate) struct PromotedField {
    pub(crate) name: String,
    pub(crate) type_text: String,
    pub(crate) path: String,
}

pub(crate) struct SelectedPromotion<'a> {
    pub(crate) object: ObjectRef<'a>,
    pub(crate) outer: Option<DeclId<'a>>,
    pub(crate) through: Vec<String>,
}

pub(crate) struct MethodView<'a> {
    pub(crate) name: String,
    pub(crate) object: ObjectRef<'a>,
    pub(crate) receiver: String,
    pub(crate) signature: String,
    pub(crate) parameters: Vec<String>,
    pub(crate) results: Vec<String>,
    pub(crate) variadic: bool,
}

/// Owned type spellings and source-name decoration from one native signature.
pub(crate) struct CallableView {
    pub(crate) names: Option<crate::analysis::engine::types::CallableNames>,
    pub(crate) parameters: Vec<String>,
    pub(crate) results: Vec<String>,
    pub(crate) variadic: bool,
}

impl CallableView {
    pub(in crate::analysis) fn from_type(
        arena: &mut TypeArena<'_>,
        ty: TypeId,
        consumer: &Path,
    ) -> Option<Self> {
        let qualifier = Qualifier::Presentation(consumer);
        let (parameters, results, variadic) = arena.signature_parts(ty)?;
        let view = Self {
            names: arena.callable_names(ty),
            parameters: parameters
                .into_iter()
                .map(|ty| arena.format_type(ty, qualifier))
                .collect::<Option<_>>()?,
            results: results
                .into_iter()
                .map(|ty| arena.format_type(ty, qualifier))
                .collect::<Option<_>>()?,
            variadic,
        };
        (view.bytes() <= crate::analysis::engine::budget::MAX_BYTES).then_some(view)
    }

    fn bytes(&self) -> usize {
        self.parameters
            .iter()
            .chain(&self.results)
            .map(String::len)
            .sum::<usize>()
            + self.names.as_ref().map_or(0, |names| {
                names.receiver.len()
                    + names
                        .parameters
                        .iter()
                        .chain(&names.results)
                        .map(String::len)
                        .sum::<usize>()
            })
    }
}

pub(crate) struct MemberTypeView {
    pub(crate) type_text: String,
    pub(crate) callable: Option<CallableView>,
    pub(crate) receiver: Option<String>,
    pub(crate) method_expression: bool,
}

impl MemberTypeView {
    pub(super) fn bytes(&self) -> usize {
        self.type_text.len()
            + self.receiver.as_ref().map_or(0, String::len)
            + self.callable.as_ref().map_or(0, CallableView::bytes)
    }
    pub(super) fn read<'a>(
        arena: &mut TypeArena<'a>,
        ty: TypeId,
        selection: Option<&Selection<'a>>,
        consumer: &Path,
        include_receiver: bool,
    ) -> Option<Self> {
        let qualifier = Qualifier::Presentation(consumer);
        let type_text = arena.format_type(ty, qualifier)?;
        let callable = if arena.is_callable(ty) == Some(true) {
            Some(CallableView::from_type(arena, ty, consumer)?)
        } else {
            None
        };
        let receiver = if let Some(selection) = selection
            .filter(|selection| include_receiver && matches!(selection.obj, ObjectRef::Method(_)))
        {
            let receiver = arena.selected_method_receiver(selection)?;
            Some(arena.format_type(receiver, qualifier)?)
        } else {
            None
        };
        let bytes = type_text.len()
            + receiver.as_ref().map_or(0, String::len)
            + callable.as_ref().map_or(0, CallableView::bytes);
        (bytes <= crate::analysis::engine::budget::MAX_BYTES).then_some(Self {
            type_text,
            callable,
            receiver,
            method_expression: selection
                .is_some_and(|selection| selection.kind == SelectionKind::MethodExpr),
        })
    }
}

impl<'a> Semantics<'a> {
    pub(crate) fn member_type_view(
        &mut self,
        file: &Path,
        source: &str,
        span: [u32; 2],
        expected: MemberDeclId<'_>,
    ) -> Option<MemberTypeView> {
        let (ty, selection) = {
            let (region, _) = self.region_and_arena_at(file, source, span[0], None)?;
            let object = region
                .def_by_span(span)
                .or_else(|| region.use_by_span(span))
                .or_else(|| {
                    region
                        .selection_by_span(span)
                        .map(|selection| selection.obj)
                })?;
            match object {
                ObjectRef::Field(member) | ObjectRef::Method(MethodOrigin::Interface(member))
                    if member == expected => {}
                _ => return None,
            }
            let selection = region.selection_by_span(span).cloned();
            let ty = selection
                .as_ref()
                .map(|selection| selection.ty)
                .or_else(|| region.expr_type(span).map(|operand| operand.ty));
            (ty, selection)
        };
        if let Some(ty) = ty {
            return MemberTypeView::read(&mut self.arena, ty, selection.as_ref(), file, true);
        }
        // An interface method declaration has no selector expression. Resolve
        // its signature in its own checked formal environment, never in a
        // caller's guessed receiver spelling.
        if expected.file != file || expected.selection != span {
            return None;
        }
        let member = expected.indexed(self.workspace)?;
        let (owner_file, stub) = self.workspace.files.get_key_value(member.file)?;
        let start = member.owner?.start;
        let symbol = stub.symbols.iter().find(|symbol| symbol.start == start)?;
        self.arena.with_projection(true, |arena| {
            let receiver = arena.declaration_type(DeclId { file:owner_file, symbol })?;
            let (_, selection) = arena.accessible_members(receiver, true, file)?.into_iter().find(|(_, selection)| {
                matches!(selection.obj, ObjectRef::Field(identity) | ObjectRef::Method(MethodOrigin::Interface(identity)) if identity == expected)
            })?;
            MemberTypeView::read(arena, selection.ty, Some(&selection), file, true)
        })
    }

    pub(crate) fn declaration_methods(
        &mut self,
        declaration: DeclId<'_>,
        consumer: &Path,
    ) -> Option<Vec<MethodView<'a>>> {
        let declaration = self.canonical_decl(declaration)?;
        let file = declaration.file;
        let symbol = declaration.symbol;
        self.arena.with_projection(true, |arena| {
            let receiver = arena.declaration_type(declaration)?;
            let interface =
                arena.form(receiver)? == crate::analysis::engine::types::TypeForm::Interface;
            let members = arena.accessible_members(receiver, true, consumer)?;
            let mut methods = Vec::new();
            let mut bytes = 0usize;
            for (name, selection) in members {
                if !matches!(selection.obj, ObjectRef::Method(_)) {
                    continue;
                }
                if interface
                    && let ObjectRef::Method(crate::analysis::engine::obj::MethodOrigin::Interface(
                        member,
                    )) = selection.obj
                    && member.file == file
                    && symbol.declaration().is_some_and(|span| {
                        span.start <= member.selection[0] as usize
                            && member.selection[1] as usize <= span.end
                    })
                {
                    continue;
                }
                if methods.len() >= 128 {
                    return None;
                }
                let qualifier = Qualifier::Presentation(consumer);
                let owner = arena.selected_method_receiver(&selection)?;
                let receiver = arena.format_type(owner, qualifier)?;
                let signature = arena.format_type(selection.ty, qualifier)?;
                let (parameters, results, variadic) = arena.signature_parts(selection.ty)?;
                let parameters = parameters
                    .into_iter()
                    .map(|ty| arena.format_type(ty, qualifier))
                    .collect::<Option<Vec<_>>>()?;
                let results = results
                    .into_iter()
                    .map(|ty| arena.format_type(ty, qualifier))
                    .collect::<Option<Vec<_>>>()?;
                bytes = bytes.checked_add(
                    name.len()
                        + receiver.len()
                        + signature.len()
                        + parameters
                            .iter()
                            .chain(&results)
                            .map(String::len)
                            .sum::<usize>(),
                )?;
                if bytes > crate::analysis::engine::budget::MAX_BYTES {
                    return None;
                }
                methods.push(MethodView {
                    name,
                    object: selection.obj,
                    receiver,
                    signature,
                    parameters,
                    results,
                    variadic,
                });
            }
            Some(methods)
        })
    }

    pub(crate) fn selected_promotion(
        &mut self,
        file: &Path,
        source: &str,
        span: [u32; 2],
    ) -> Option<SelectedPromotion<'a>> {
        let (region, arena) = self.region_and_arena_at(file, source, span[0], None)?;
        let selection = region.selection_by_span(span)?.clone();
        let indices = selection.indices.as_slice();
        let indices = if selection.kind == SelectionKind::FieldVal {
            indices.get(..indices.len().checked_sub(1)?)?
        } else {
            indices
        };
        let path = arena.embedded_field_path(selection.recv, indices)?;
        let through = path
            .into_iter()
            .map(|(_, ty)| arena.format_type(ty, Qualifier::Presentation(file)))
            .collect::<Option<Vec<_>>>()?;
        Some(SelectedPromotion {
            object: selection.obj,
            outer: arena.receiver_declaration(selection.recv),
            through,
        })
    }

    pub(crate) fn promoted_fields(
        &mut self,
        declaration: DeclId<'_>,
        consumer: &Path,
    ) -> Option<Vec<PromotedField>> {
        let (file, stub) = self.workspace.files.get_key_value(declaration.file)?;
        let symbol = stub.symbols.iter().find(|symbol| {
            symbol.start == declaration.symbol.start && symbol.name == declaration.symbol.name
        })?;
        self.arena.with_projection(false, |arena| {
            let receiver = arena.declaration_type(DeclId { file, symbol })?;
            let mut members = arena.accessible_members(receiver, true, consumer)?;
            members.retain(|(_, selection)| {
                selection.kind == SelectionKind::FieldVal && selection.indices.as_slice().len() > 1
            });
            if members.len() > 128 {
                return None;
            }
            // gopls presents source-order depth-first inventory, after native
            // breadth-first lookup has resolved collisions and shadowing.
            members.sort_by(|a, b| a.1.indices.as_slice().cmp(b.1.indices.as_slice()));
            let mut fields = Vec::new();
            let mut bytes = 0usize;
            for (name, selection) in members {
                let indices = selection.indices.as_slice();
                let path = arena.embedded_field_path(receiver, &indices[..indices.len() - 1])?;
                let path = path
                    .into_iter()
                    .map(|(name, _)| name)
                    .collect::<Vec<_>>()
                    .join(".");
                let type_text =
                    arena.format_type(selection.ty, Qualifier::Presentation(consumer))?;
                bytes = bytes.checked_add(name.len() + type_text.len() + path.len())?;
                if bytes > crate::analysis::engine::budget::MAX_BYTES {
                    return None;
                }
                fields.push(PromotedField {
                    name,
                    type_text,
                    path,
                });
            }
            Some(fields)
        })
    }
}
