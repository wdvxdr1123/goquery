//! Universe recognition shares local, package and import shadow guards.
use super::Locals;
use crate::workspace::Workspace;
use std::path::Path;

pub(crate) fn imported_name(workspace: &Workspace, file: &Path, name: &str) -> bool {
    workspace.files.get(file).is_some_and(|stub| {
        stub.imports.iter().any(|import| {
            !import.is_dot() && !import.is_blank() && workspace.import_name(import) == Some(name)
        })
    })
}
pub(crate) fn visible(
    workspace: &Workspace,
    file: &Path,
    locals: Option<&Locals>,
    offset: usize,
    name: &str,
) -> bool {
    locals.is_none_or(|locals| locals.resolve_at(offset, name).is_none())
        && workspace.unqualified_symbol(file, name).is_none()
        && !imported_name(workspace, file, name)
}

pub(crate) fn builtin(name: &str) -> bool {
    builtin_type(name)
        || builtin_function(name)
        || matches!(name, "true" | "false" | "nil" | "iota")
}

pub(crate) fn builtin_type(name: &str) -> bool {
    matches!(
        name,
        "any"
            | "bool"
            | "byte"
            | "comparable"
            | "complex64"
            | "complex128"
            | "error"
            | "float32"
            | "float64"
            | "int"
            | "int8"
            | "int16"
            | "int32"
            | "int64"
            | "rune"
            | "string"
            | "uint"
            | "uint8"
            | "uint16"
            | "uint32"
            | "uint64"
            | "uintptr"
    )
}

pub(crate) fn builtin_function(name: &str) -> bool {
    matches!(
        name,
        "append"
            | "cap"
            | "clear"
            | "close"
            | "complex"
            | "copy"
            | "delete"
            | "imag"
            | "len"
            | "make"
            | "max"
            | "min"
            | "new"
            | "panic"
            | "print"
            | "println"
            | "real"
            | "recover"
    )
}
