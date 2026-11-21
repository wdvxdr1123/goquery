//! Unified selector resolution results, mirroring `go/types.Selection`.

use super::{Ty, TypeArena, TypeId};
use crate::analysis::engine::obj::{DeclId, ObjectRef};

/// Kind of selector expression resolved by field/method lookup.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum SelectionKind {
    FieldVal,
    MethodVal,
    MethodExpr,
}

/// Compact embedded-field traversal path (inline for depth <= 4).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct PromotionIndices {
    inline: [u16; 4],
    len: u8,
    overflow: Option<Box<[u16]>>,
}

impl PromotionIndices {
    pub(crate) fn from_slice(indices: &[u16]) -> Option<Self> {
        if indices.len() <= 4 {
            let mut inline = [0_u16; 4];
            inline[..indices.len()].copy_from_slice(indices);
            Some(Self {
                inline,
                len: indices.len() as u8,
                overflow: None,
            })
        } else {
            Some(Self {
                inline: [0_u16; 4],
                len: 0,
                overflow: Some(indices.into()),
            })
        }
    }

    pub(crate) fn as_slice(&self) -> &[u16] {
        if let Some(overflow) = &self.overflow {
            overflow
        } else {
            &self.inline[..usize::from(self.len)]
        }
    }

    pub(crate) fn conflicts(&self, other: &Self) -> bool {
        self.as_slice().starts_with(other.as_slice())
            || other.as_slice().starts_with(self.as_slice())
    }
}

/// Complete resolved selection information from `lookup_field_or_method` or `lookup_method_expr`.
#[derive(Clone, Debug)]
pub(crate) struct Selection<'a> {
    pub(crate) kind: SelectionKind,
    pub(crate) recv: TypeId,
    pub(crate) obj: ObjectRef<'a>,
    pub(crate) ty: TypeId,
    pub(crate) indices: PromotionIndices,
    pub(crate) indirect: bool,
}

impl<'a> TypeArena<'a> {
    pub(crate) fn receiver_declaration(&mut self, input: TypeId) -> Option<DeclId<'a>> {
        let root = self.receiver_base(input)?;
        self.named_declaration(root)
            .map(|(declaration, _, _)| declaration)
    }

    pub(crate) fn receiver_base(&mut self, input: TypeId) -> Option<TypeId> {
        let mut root = self.unalias(input)?;
        if let Ty::Pointer(element) = self.node(root)? {
            root = self.unalias(*element)?;
        }
        Some(root)
    }

    /// Project a proven selection path through the same instantiated fields.
    /// This does not search for a member or resolve any source spelling.
    pub(crate) fn embedded_field_path(
        &mut self,
        mut input: TypeId,
        indices: &[u16],
    ) -> Option<Vec<(String, TypeId)>> {
        let mut path = Vec::new();
        for index in indices {
            self.step()?;
            let mut root = self.unalias(input)?;
            if let Ty::Pointer(element) = self.node(root)? {
                root = self.unalias(*element)?;
            }
            let root = self.underlying(root)?;
            let fields = self.struct_data(root)?.fields.clone();
            let field = self.fields_slice(root, fields)?.get(usize::from(*index))?;
            if !field.embedded {
                return None;
            }
            input = field.ty;
            path.push((field.name.clone(), input));
        }
        Some(path)
    }
}
