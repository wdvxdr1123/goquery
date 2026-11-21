//! gopls-compatible size/offset annotations for type and field declarations.
//!
//! Layout follows `go/types` gcSizes for the request view's word size: word-aligned
//! fields, gc's zero-size final field rule and padded struct sizes. Only
//! structural type layouts are computed; unresolved or parameterized types stay
//! unknown instead of guessing.
use crate::analysis::query::Context;
use crate::analysis::sema::DeclId;
use crate::analysis::sema::layout::{FieldLayout, Layout, StructLayout, align_to};
use crate::index::IndexedSymbol as Symbol;
use std::path::Path;

fn declaration<'a>(context: &Context<'a>, file: &Path, symbol: &Symbol) -> Option<DeclId<'a>> {
    let (file, stub) = context.workspace().files.get_key_value(file)?;
    let symbol = stub
        .symbols
        .iter()
        .find(|entry| entry.start == symbol.start)?;
    Some(DeclId { file, symbol })
}

pub(super) fn struct_text_layout(
    context: &mut Context<'_>,
    origin: &Path,
    text: &str,
) -> Option<StructLayout> {
    context.with_sema(|sema| sema.struct_layout_of_text(origin, text))
}

pub(super) fn type_layout(
    context: &mut Context<'_>,
    origin: &Path,
    symbol: &Symbol,
) -> Option<Layout> {
    let decl = declaration(context, origin, symbol)?;
    context.with_sema(|sema| sema.layout_of_decl(decl))
}

pub(super) fn struct_layout(
    context: &mut Context<'_>,
    origin: &Path,
    symbol: &Symbol,
) -> Option<StructLayout> {
    let decl = declaration(context, origin, symbol)?;
    context.with_sema(|sema| sema.struct_layout_of_decl(decl))
}

pub(super) fn field_layout(
    context: &mut Context<'_>,
    origin: &Path,
    owner: &Symbol,
    name: &str,
) -> Option<FieldLayout> {
    struct_layout(context, origin, owner)?
        .fields
        .into_iter()
        .find(|field| field.name == name)
}

/// gopls' wasted-space percentage of a struct type.
pub(super) fn wasted(layout: StructLayout) -> Option<i64> {
    if layout.layout.size <= 0 || layout.fields.is_empty() {
        return None;
    }
    let mut fields: Vec<&FieldLayout> = layout.fields.iter().collect();
    fields.sort_by_key(|field| std::cmp::Reverse(field.size));
    let mut offset = 0i64;
    let mut last = (0i64, 0i64);
    for field in &fields {
        offset = align_to(offset, field.align.max(1))?;
        last = (offset, field.size);
        offset = offset.checked_add(field.size)?;
    }
    let compact = last.0 + last.1;
    let wasted = 100 * (layout.layout.size - compact) / layout.layout.size;
    (wasted >= 0).then_some(wasted)
}

/// `size=..[ (N% wasted)][, class=..]` annotation.
pub(super) fn size_summary(layout: Layout, wasted: Option<i64>) -> String {
    let mut text = format!("size={}", number(layout.size));
    if let Some(wasted) = wasted.filter(|wasted| *wasted >= 20) {
        text.push_str(&format!(" ({wasted}% wasted)"));
    }
    if let Some(class) = size_class(layout.size).filter(|class| *class > layout.size) {
        text.push_str(&format!(", class={}", number(class)));
    }
    text
}

/// gopls `sizeClass`: the runtime allocator size class of an allocation.
pub(super) fn size_class(size: i64) -> Option<i64> {
    const CLASSES: &[i64] = &[
        0, 8, 16, 24, 32, 48, 64, 80, 96, 112, 128, 144, 160, 176, 192, 208, 224, 240, 256, 288,
        320, 352, 384, 416, 448, 480, 512, 576, 640, 704, 768, 896, 1024, 1152, 1280, 1408, 1536,
        1792, 2048, 2304, 2688, 3072, 3200, 3456, 4096, 4864, 5376, 6144, 6528, 6784, 6912, 8192,
        9472, 9728, 10240, 10880, 12288, 13568, 14336, 16384, 18432, 19072, 20480, 21760, 24576,
        27264, 28672, 32768,
    ];
    const PAGE: i64 = 8192;
    if size > 1 << 16 {
        return None;
    }
    if size <= 0 {
        return Some(0);
    }
    if size > 32768 {
        return Some((size + PAGE - 1) / PAGE * PAGE);
    }
    CLASSES.iter().copied().find(|class| *class >= size)
}

/// Decimal rendering used by `size=`/`offset=` annotations.
pub(super) fn number(value: i64) -> String {
    if value < 10 {
        value.to_string()
    } else {
        format!("{value} (0x{value:x})")
    }
}
