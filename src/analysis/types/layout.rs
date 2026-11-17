//! gcSizes over structural type identities. Unknown facts never acquire a size.
use super::{BasicKind, MAX_DEPTH, Ty, TypeArena, TypeId};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Layout {
    pub(crate) size: i64,
    pub(crate) align: i64,
}

pub(crate) struct FieldLayout {
    pub(crate) name: String,
    pub(crate) size: i64,
    pub(crate) offset: i64,
    pub(crate) align: i64,
}

pub(crate) struct StructLayout {
    pub(crate) layout: Layout,
    pub(crate) fields: Vec<FieldLayout>,
}

pub(crate) fn align_to(value: i64, align: i64) -> Option<i64> {
    value
        .checked_add(align.checked_sub(1)?)?
        .checked_div(align)?
        .checked_mul(align)
}

impl TypeArena<'_> {
    pub(crate) fn layout(&mut self, input: TypeId) -> Option<Layout> {
        let (word, max_align) = self.build_view.gc_sizes()?;
        self.layout_inner(input, word, max_align, 0)
    }

    fn layout_inner(
        &mut self,
        input: TypeId,
        word: i64,
        max_align: i64,
        depth: usize,
    ) -> Option<Layout> {
        self.step()?;
        if depth >= MAX_DEPTH {
            return None;
        }
        let input = self.unalias(input)?;
        let root = self.underlying(input)?;
        let layout = match self.nodes.get(usize::from(root.0))?.clone() {
            Ty::Basic(kind) => {
                let size = match kind.canonical() {
                    BasicKind::Bool | BasicKind::Int8 | BasicKind::Uint8 => 1,
                    BasicKind::Int16 | BasicKind::Uint16 => 2,
                    BasicKind::Int32 | BasicKind::Uint32 | BasicKind::Float32 => 4,
                    BasicKind::Int64
                    | BasicKind::Uint64
                    | BasicKind::Float64
                    | BasicKind::Complex64 => 8,
                    BasicKind::Complex128 => 16,
                    BasicKind::Int
                    | BasicKind::Uint
                    | BasicKind::Uintptr
                    | BasicKind::UnsafePointer => word,
                    BasicKind::String => {
                        return Some(Layout {
                            size: 2 * word,
                            align: word,
                        });
                    }
                    _ => return None,
                };
                let align = if matches!(kind, BasicKind::Complex64 | BasicKind::Complex128) {
                    size / 2
                } else {
                    size
                };
                Layout {
                    size,
                    align: align.min(max_align),
                }
            }
            Ty::Pointer(_) | Ty::Map { .. } | Ty::Chan { .. } | Ty::Signature { .. } => Layout {
                size: word,
                align: word,
            },
            Ty::Slice(_) => Layout {
                size: 3 * word,
                align: word,
            },
            Ty::Interface { .. } => Layout {
                size: 2 * word,
                align: word,
            },
            Ty::Array { length, element } => {
                let element = self.layout_inner(element, word, max_align, depth + 1)?;
                Layout {
                    size: element.size.checked_mul(i64::try_from(length).ok()?)?,
                    align: element.align,
                }
            }
            Ty::Struct { fields, .. } => {
                let fields = self.fields.get(fields)?.to_vec();
                let mut offset = 0;
                let count = fields.len();
                let mut align = if count == 0 {
                    self.atomic_alignment(input).unwrap_or(1)
                } else {
                    1
                };
                for (index, field) in fields.iter().enumerate() {
                    let layout = self.layout_inner(field.ty, word, max_align, depth + 1)?;
                    offset = align_to(offset, layout.align)?;
                    align = align.max(layout.align);
                    let size = if index + 1 == count && offset > 0 && layout.size == 0 {
                        1
                    } else {
                        layout.size
                    };
                    offset = offset.checked_add(size)?;
                }
                Layout {
                    size: align_to(offset, align)?,
                    align,
                }
            }
            _ => return None,
        };
        Some(layout)
    }

    /// Compiler-recognized marker declarations; spelling alone is insufficient.
    fn atomic_alignment(&self, input: TypeId) -> Option<i64> {
        let (decl, _, _) = self.named_declaration(input)?;
        matches!(
            self.workspace.import_path(decl.file)?.as_str(),
            "sync/atomic" | "internal/runtime/atomic"
        )
        .then_some(())?;
        match decl.symbol.name.as_str() {
            "align64" => Some(8),
            "align128" => Some(16),
            _ => None,
        }
    }

    pub(crate) fn struct_layout(&mut self, input: TypeId) -> Option<StructLayout> {
        let layout = self.layout(input)?;
        let root = self.underlying(input)?;
        let Ty::Struct { fields, .. } = self.nodes.get(usize::from(root.0))?.clone() else {
            return None;
        };
        let fields = self.fields.get(fields)?.to_vec();
        let mut result = Vec::with_capacity(fields.len());
        let mut offset = 0;
        for field in fields {
            let layout = self.layout(field.ty)?;
            offset = align_to(offset, layout.align)?;
            result.push(FieldLayout {
                name: field.name,
                size: layout.size,
                offset,
                align: layout.align,
            });
            offset = offset.checked_add(layout.size)?;
        }
        Some(StructLayout {
            layout,
            fields: result,
        })
    }
}
