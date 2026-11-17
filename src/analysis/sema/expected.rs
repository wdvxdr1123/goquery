//! Completion compatibility uses checked expectations and native assignments.
use super::completion::{CompletionSite, completion_receiver};
use super::{MemberRequest, Semantics};
use crate::analysis::obj::{DeclId, ObjectRef, SymbolOrigin};
use crate::analysis::types::{Mode, Operand};
use std::path::Path;

#[derive(Clone, Copy)]
pub(crate) enum CompletionProbe<'a> {
    Declaration(DeclId<'a>),
    Binding {
        selection: [u32; 2],
        scope_start: u32,
    },
    Member(ObjectRef<'a>),
}

pub(crate) struct CompletionRanking {
    pub(crate) function_value: bool,
    pub(crate) compatible: Vec<Option<bool>>,
}

impl<'a> Semantics<'a> {
    /// Rank only the bounded fuzzy preselection. Each speculative declaration
    /// is rewound together with declaration-cache facts that point above it.
    pub(crate) fn completion_ranking(
        &mut self,
        file: &Path,
        source: &str,
        offset: u32,
        member: Option<MemberRequest<'_>>,
        probes: &[(Option<CompletionProbe<'_>>, bool)],
        calls: bool,
    ) -> Option<CompletionRanking> {
        if probes.len() > 200 {
            return None;
        }
        self.region_and_arena_at(file, source, offset, Some(offset as usize))?;
        let selections =
            member.and_then(|request| self.completion_selections(file, source, request));
        let region = &self.cached.as_ref()?.region;
        let target = region.expected_type_at(offset)?;
        let function_value = self.arena.is_callable(target)?;
        let receiver = member.and_then(|request| completion_receiver(region, request.site));
        let int_bits = self.arena.integer_bits()?;
        let workspace = self.workspace;
        let mut compatible = Vec::with_capacity(probes.len());
        for (probe, call) in probes {
            let fit = self.arena.with_projection(false, |arena| {
                let operand = match probe.as_ref()? {
                    CompletionProbe::Declaration(declaration) => {
                        let (file, stub) = workspace.files.get_key_value(declaration.file)?;
                        let symbol = stub.symbols.iter().find(|symbol| {
                            symbol.start == declaration.symbol.start
                                && symbol.name == declaration.symbol.name
                        })?;
                        self.decls.operand_of(arena, DeclId { file, symbol })
                    }
                    CompletionProbe::Binding {
                        selection,
                        scope_start,
                    } => {
                        let object = region.def_by_span(*selection).or_else(|| {
                            region.implicits.iter().find_map(|(span, object)| {
                                if span[0] <= *scope_start
                                    && *scope_start < span[1]
                                    && let ObjectRef::Var {
                                        origin: SymbolOrigin::Local { span, .. },
                                        ..
                                    } = object
                                    && span == selection
                                {
                                    Some(*object)
                                } else {
                                    None
                                }
                            })
                        });
                        match object {
                            Some(
                                ObjectRef::Var {
                                    origin: SymbolOrigin::Local { id, .. },
                                    ..
                                }
                                | ObjectRef::Const(SymbolOrigin::Local { id, .. }),
                            ) => region.local_type(id),
                            _ => None,
                        }
                    }
                    CompletionProbe::Member(object) => {
                        let (name, selection) = selections
                            .as_ref()?
                            .iter()
                            .find(|(_, selection)| selection.obj == *object)?;
                        let ty = if receiver?.mode == Mode::TypeExpr
                            && matches!(member?.site, CompletionSite::Selector(_))
                        {
                            arena.lookup_method_expr(receiver?.ty, file, name)??.ty
                        } else {
                            selection.ty
                        };
                        Some(Operand {
                            mode: Mode::Value,
                            ty,
                            value: None,
                        })
                    }
                }?;
                if !operand.mode.is_value() {
                    return Some(false);
                }
                if arena.assign_value(operand, target, int_bits) == Some(true) {
                    return Some(true);
                }
                if calls
                    && *call
                    && !function_value
                    && let Some((_, results, _)) = arena.signature_parts(operand.ty)
                    && results.len() == 1
                {
                    return arena.assign_value(
                        Operand {
                            mode: Mode::Value,
                            ty: results[0],
                            value: None,
                        },
                        target,
                        int_bits,
                    );
                }
                arena.assign_value(operand, target, int_bits)
            });
            self.decls.rewind_above(self.arena.node_count());
            compatible.push(fit);
        }
        Some(CompletionRanking {
            function_value,
            compatible,
        })
    }
}
