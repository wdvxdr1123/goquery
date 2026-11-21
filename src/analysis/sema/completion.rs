//! Bounded completion projections of checked operands and exact selections.
use super::{CallableView, MemberTypeView, Semantics};
use crate::analysis::engine::check::region::RegionInfo;
use crate::analysis::engine::obj::{DeclId, ObjectRef, SymbolOrigin};
use crate::analysis::engine::types::{Mode, Operand, Selection, TypeArena, TypeForm, TypeId};
use std::collections::HashSet;
use std::path::Path;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum CompletionSite {
    Selector([u32; 2]),
    Literal([u32; 2]),
}

#[derive(Clone, Copy)]
pub(crate) struct MemberRequest<'a> {
    pub(crate) site: CompletionSite,
    pub(crate) offset: u32,
    pub(crate) used: &'a [[u32; 2]],
}

pub(super) struct CachedCompletionSelections<'a> {
    file: &'a Path,
    region_span: [u32; 2],
    site: CompletionSite,
    used: Vec<[u32; 2]>,
    pub(super) receiver: Operand,
    pub(super) members: Vec<(String, Selection<'a>)>,
}

pub(super) fn completion_receiver(
    region: &RegionInfo<'_>,
    site: CompletionSite,
) -> Option<Operand> {
    match site {
        CompletionSite::Selector(span) => region.expr_type(span),
        CompletionSite::Literal(span) => Some(Operand {
            mode: Mode::Value,
            ty: region.literal_type(span)?,
            value: None,
        }),
    }
}

pub(crate) struct CompletionTypeView {
    pub(crate) form: TypeForm,
    pub(crate) text: String,
    pub(crate) callable: Option<CallableView>,
    pub(crate) header: Option<String>,
}

impl CompletionTypeView {
    fn read(arena: &mut TypeArena<'_>, ty: TypeId, consumer: &Path) -> Option<Self> {
        let (form, root, source) = arena.presentation_type(ty)?;
        if matches!(form, TypeForm::Struct | TypeForm::Interface) {
            return Some(Self {
                form,
                text: if form == TypeForm::Struct {
                    "struct{...}"
                } else {
                    "interface{...}"
                }
                .into(),
                callable: None,
                header: None,
            });
        }
        let view = MemberTypeView::read(arena, root, None, consumer, false)?;
        let header = if form == TypeForm::Signature {
            if let Some((decl, _, _)) = arena.named_declaration(source) {
                let span = decl.symbol.end..decl.symbol.declaration()?.end;
                let source = arena.workspace().source_slice(decl.file, span)?;
                crate::analysis::engine::types::declaration_body(&source).map(str::to_owned)
            } else {
                arena
                    .local_type_annotation(source)
                    .map(|(_, annotation)| annotation.to_owned())
            }
        } else {
            None
        };
        Some(Self {
            form,
            text: view.type_text,
            callable: view.callable,
            header,
        })
    }
}

impl<'a> Semantics<'a> {
    pub(crate) fn declaration_completion_type(
        &mut self,
        declaration: DeclId<'_>,
        consumer: &Path,
    ) -> Option<CompletionTypeView> {
        let declaration = self.canonical_decl(declaration)?;
        self.arena.with_projection(true, |arena| {
            let ty = arena.declaration_type(declaration)?;
            CompletionTypeView::read(arena, ty, consumer)
        })
    }

    pub(crate) fn local_completion_type(
        &mut self,
        file: &Path,
        source: &str,
        span: [u32; 2],
    ) -> Option<CompletionTypeView> {
        let (region, arena) = self.region_and_arena_at(file, source, span[0], None)?;
        match region.def_by_span(span)? {
            ObjectRef::TypeParam { .. } => Some(CompletionTypeView {
                form: TypeForm::Interface,
                text: "interface{...}".into(),
                callable: None,
                header: None,
            }),
            ObjectRef::TypeName {
                origin: SymbolOrigin::Local { id, .. },
                ..
            } => {
                let ty = region.local_type(id)?.ty;
                arena.with_projection(true, |arena| CompletionTypeView::read(arena, ty, file))
            }
            _ => None,
        }
    }

    pub(super) fn completion_selections(
        &mut self,
        file: &Path,
        source: &str,
        request: MemberRequest<'_>,
    ) -> Option<CachedCompletionSelections<'a>> {
        let file = self.intern_file(file)?;
        if !self.ensure_region(file, source, request.offset, Some(request.offset as usize)) {
            return None;
        }
        let region_span = self.cached.as_ref()?.region.span;
        if let Some(cached) = self.cached_completion.take()
            && cached.file == file
            && cached.region_span == region_span
            && cached.site == request.site
            && cached.used.as_slice() == request.used
        {
            return Some(cached);
        }
        let region = &self.cached.as_ref()?.region;
        let receiver = completion_receiver(region, request.site)?;
        if !receiver.mode.is_value() && receiver.mode != Mode::TypeExpr {
            return None;
        }
        let mut members = match request.site {
            CompletionSite::Selector(_) => {
                self.arena
                    .accessible_members(receiver.ty, receiver.mode == Mode::Variable, file)?
            }
            CompletionSite::Literal(_) => self.arena.literal_fields(receiver.ty, file)?,
        };
        if receiver.mode == Mode::TypeExpr {
            members.retain(|(_, selection)| matches!(selection.obj, ObjectRef::Method(_)));
        }
        if request.used.len() > crate::analysis::engine::budget::MAX_NODES {
            return None;
        }
        let used: HashSet<_> = request
            .used
            .iter()
            .filter_map(|span| region.use_by_span(*span))
            .collect();
        members.retain(|(_, selection)| !used.contains(&selection.obj));
        if matches!(request.site, CompletionSite::Literal(_)) {
            members.retain(|(_, selection)| {
                !request
                    .used
                    .iter()
                    .filter_map(|span| region.selection_by_span(*span))
                    .any(|used| selection.indices.conflicts(&used.indices))
            });
        }
        Some(CachedCompletionSelections {
            file,
            region_span,
            site: request.site,
            used: request.used.to_vec(),
            receiver,
            members,
        })
    }

    pub(crate) fn completion_members(
        &mut self,
        file: &Path,
        source: &str,
        request: MemberRequest<'_>,
    ) -> Option<Vec<(String, ObjectRef<'a>)>> {
        let cached = self.completion_selections(file, source, request)?;
        let out = cached
            .members
            .iter()
            .map(|(name, selection)| (name.clone(), selection.obj))
            .collect();
        self.cached_completion = Some(cached);
        Some(out)
    }

    /// Only the ranked candidates are formatted. No TypeId survives a facade
    /// call; all selected type strings together fit the shared 16 KiB budget.
    pub(crate) fn completion_member_views(
        &mut self,
        file: &Path,
        source: &str,
        request: MemberRequest<'_>,
        objects: &[ObjectRef<'_>],
    ) -> Option<Vec<Option<MemberTypeView>>> {
        if objects.len() > 200 {
            return None;
        }
        let cached = self.completion_selections(file, source, request)?;
        let receiver = cached.receiver;
        let mut bytes = 0usize;
        let mut views = Vec::with_capacity(objects.len());
        for object in objects {
            let selection = cached
                .members
                .iter()
                .find(|(_, selection)| selection.obj == *object)
                .and_then(|(name, selection)| {
                    if receiver.mode == Mode::TypeExpr {
                        self.arena
                            .lookup_method_expr(receiver.ty, file, name)
                            .flatten()
                            .filter(|selection| selection.obj == *object)
                    } else {
                        Some(selection.clone())
                    }
                });
            let view = selection.as_ref().and_then(|selection| {
                MemberTypeView::read(&mut self.arena, selection.ty, Some(selection), file, false)
            });
            if let Some(view) = &view {
                bytes = bytes.checked_add(view.bytes())?;
                if bytes > crate::analysis::engine::budget::MAX_BYTES {
                    self.cached_completion = Some(cached);
                    return None;
                }
            }
            views.push(view);
        }
        self.cached_completion = Some(cached);
        Some(views)
    }
}
