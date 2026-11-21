//! `gcSizes` over structural type identities. Unknown facts never acquire a size.
use super::{BasicKind, MAX_DEPTH, Ty, TypeId, TypeStore};

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

/// Target-specific layout calculation over `TypeStore`.
pub(crate) trait Sizes<'a> {
    fn layout(&mut self, store: &mut TypeStore<'a>, input: TypeId) -> Option<Layout>;
    fn struct_layout(&mut self, store: &mut TypeStore<'a>, input: TypeId) -> Option<StructLayout>;
}

/// Standard `gc` compiler sizing (`wordSize`, `maxAlign`).
pub(crate) struct StdSizes {
    word: i64,
    max_align: i64,
}

impl StdSizes {
    pub(crate) const fn new(word: i64, max_align: i64) -> Self {
        Self { word, max_align }
    }

    pub(crate) fn from_store(store: &TypeStore<'_>) -> Option<Self> {
        let (word, max_align) = store.build_view.gc_sizes()?;
        Some(Self::new(word, max_align))
    }

    fn layout_inner(
        &self,
        store: &mut TypeStore<'_>,
        input: TypeId,
        depth: usize,
    ) -> Option<Layout> {
        store.step()?;
        if depth >= MAX_DEPTH {
            return None;
        }
        let input = store.unalias(input)?;
        let root = store.underlying(input)?;
        let layout = match *store.node(root)? {
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
                    | BasicKind::UnsafePointer => self.word,
                    BasicKind::String => {
                        return Some(Layout {
                            size: 2 * self.word,
                            align: self.word,
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
                    align: align.min(self.max_align),
                }
            }
            Ty::Pointer(_) | Ty::Map { .. } | Ty::Chan { .. } | Ty::Signature(_) => Layout {
                size: self.word,
                align: self.word,
            },
            Ty::Slice(_) => Layout {
                size: 3 * self.word,
                align: self.word,
            },
            Ty::Interface(_) => Layout {
                size: 2 * self.word,
                align: self.word,
            },
            Ty::Array { length, element } => {
                let element = self.layout_inner(store, element, depth + 1)?;
                Layout {
                    size: element.size.checked_mul(i64::try_from(length).ok()?)?,
                    align: element.align,
                }
            }
            Ty::Struct(_) => {
                let fields = store.struct_data(root)?.fields.clone();
                let fields = store.fields_slice(root, fields)?.to_vec();
                let mut offset = 0;
                let count = fields.len();
                let mut align = if count == 0 {
                    atomic_alignment(store, input).unwrap_or(1)
                } else {
                    1
                };
                for (index, field) in fields.iter().enumerate() {
                    let layout = self.layout_inner(store, field.ty, depth + 1)?;
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
}

impl<'a> Sizes<'a> for StdSizes {
    fn layout(&mut self, store: &mut TypeStore<'a>, input: TypeId) -> Option<Layout> {
        self.layout_inner(store, input, 0)
    }

    fn struct_layout(&mut self, store: &mut TypeStore<'a>, input: TypeId) -> Option<StructLayout> {
        let layout = self.layout(store, input)?;
        let root = store.underlying(input)?;
        let fields = store.struct_data(root)?.fields.clone();
        let fields = store.fields_slice(root, fields)?.to_vec();
        let mut result = Vec::with_capacity(fields.len());
        let mut offset = 0;
        for field in fields {
            let layout = self.layout(store, field.ty)?;
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

/// Compiler-recognized marker declarations; spelling alone is insufficient.
fn atomic_alignment(store: &TypeStore<'_>, input: TypeId) -> Option<i64> {
    let (decl, _, _) = store.named_declaration(input)?;
    matches!(
        store.workspace.import_path(decl.file)?.as_str(),
        "sync/atomic" | "internal/runtime/atomic"
    )
    .then_some(())?;
    match decl.symbol.name.as_str() {
        "align64" => Some(8),
        "align128" => Some(16),
        _ => None,
    }
}

impl<'a> TypeStore<'a> {
    pub(crate) fn layout(&mut self, input: TypeId) -> Option<Layout> {
        StdSizes::from_store(self)?.layout(self, input)
    }

    pub(crate) fn struct_layout(&mut self, input: TypeId) -> Option<StructLayout> {
        StdSizes::from_store(self)?.struct_layout(self, input)
    }
}
