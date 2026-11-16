//! Unified selector resolution results, mirroring `go/types.Selection`.

use super::TypeId;
use crate::analysis::obj::ObjectRef;

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
