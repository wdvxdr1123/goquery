//! Unified `go/types.TypeString` formatting controlled by `Qualifier`.

use super::ty::{CheckMode, TypeOrigin};
use super::{BasicKind, Direction, MAX_BYTES, MAX_DEPTH, Ty, TypeId, TypeStore};
use crate::syntax::{Lexer, TokenKind};
use std::path::Path;

/// Controls package qualification when rendering Go type spellings.
#[derive(Clone, Copy)]
pub(crate) enum Qualifier<'p> {
    RelativeTo(&'p Path),
    Presentation(&'p Path),
    FullPath,
}

/// Read-only formatter over `&TypeStore` with explicit checking mode.
pub(crate) struct TypeWriter<'s, 'a> {
    store: &'s TypeStore<'a>,
    mode: CheckMode,
}

impl<'s, 'a> TypeWriter<'s, 'a> {
    pub(crate) const fn new(store: &'s TypeStore<'a>, mode: CheckMode) -> Self {
        Self { store, mode }
    }

    pub(crate) fn format(&self, id: TypeId, qualifier: Qualifier<'_>) -> Option<String> {
        let mut output = String::new();
        self.write_type_in(id, qualifier, &mut output, 0)?;
        Some(output)
    }

    fn write_type_in(
        &self,
        id: TypeId,
        qualifier: Qualifier<'_>,
        out: &mut String,
        depth: usize,
    ) -> Option<()> {
        if depth >= MAX_DEPTH {
            return None;
        }
        match *self.store.node(id)? {
            Ty::Invalid => return None,
            Ty::TypeParam(_) => {
                let (origin, _, name) = self.store.local_parameter(id)?;
                if !self.mode.is_symbolic()
                    && matches!(qualifier, Qualifier::RelativeTo(file) | Qualifier::Presentation(file) if file != origin)
                {
                    return None;
                }
                out.push_str(name);
            }
            Ty::Interface(_) => {
                let data = self.store.interface_data(id)?;
                self.write_interface(id, data, qualifier, out, depth)?;
            }
            Ty::Signature(_) => {
                let data = self.store.signature_data(id)?;
                self.write_signature(
                    id,
                    data.parameters.clone(),
                    data.results.clone(),
                    data.variadic,
                    qualifier,
                    out,
                    depth,
                )?;
            }
            Ty::Struct(_) => {
                let data = self.store.struct_data(id)?;
                self.write_struct(id, data.origin, data.fields.clone(), qualifier, out, depth)?;
            }
            Ty::Union(slice) => {
                for (index, term) in self.store.terms_slice(id, slice).iter().enumerate() {
                    if index > 0 {
                        out.push_str(" | ");
                    }
                    if term.approximate {
                        out.push('~');
                    }
                    self.write_type_in(term.ty, qualifier, out, depth + 1)?;
                }
            }
            Ty::Basic(BasicKind::UnsafePointer) => match qualifier {
                Qualifier::FullPath => out.push_str("unsafe.Pointer"),
                Qualifier::Presentation(file) => {
                    if self.store.workspace.import_path(file).as_deref() == Some("unsafe") {
                        out.push_str("Pointer");
                    } else {
                        let imported = self
                            .store
                            .workspace
                            .files
                            .get(file)?
                            .imports
                            .iter()
                            .rev()
                            .find(|c| !c.is_blank() && c.path == "unsafe");
                        if !imported.is_some_and(|i| i.is_dot()) {
                            let name = imported
                                .and_then(|i| self.store.resolver().import_name(file, i))
                                .unwrap_or("unsafe");
                            out.push_str(name);
                            out.push('.');
                        }
                        out.push_str("Pointer");
                    }
                }
                Qualifier::RelativeTo(file) => {
                    if let Some(unsafe_file) = self
                        .store
                        .workspace
                        .builtin_file()
                        .and_then(|b| Some(b.parent()?.parent()?.join("unsafe/unsafe.go")))
                        && self.store.workspace.files.contains_key(&unsafe_file)
                    {
                        out.push_str(&crate::analysis::engine::resolve::result_type(
                            self.store.workspace,
                            file,
                            &unsafe_file,
                            "Pointer",
                        )?);
                    } else if self.store.workspace.import_path(file).as_deref() == Some("unsafe") {
                        out.push_str("Pointer");
                    } else {
                        let imported = self
                            .store
                            .workspace
                            .files
                            .get(file)?
                            .imports
                            .iter()
                            .rev()
                            .find(|c| !c.is_blank() && c.path == "unsafe")?;
                        if imported.is_dot() {
                            out.push_str("Pointer");
                        } else {
                            out.push_str(self.store.resolver().import_name(file, imported)?);
                            out.push_str(".Pointer");
                        }
                    }
                }
            },
            Ty::Basic(kind) => out.push_str(kind.name()),
            Ty::Named { origin, arguments } | Ty::Alias { origin, arguments } => {
                match self.store.origin_data(id, origin)? {
                    TypeOrigin::BuiltinError { .. } => out.push_str("error"),
                    TypeOrigin::Local {
                        origin,
                        start,
                        end,
                        formal,
                    } => {
                        let selection = *start as usize..*end as usize;
                        match qualifier {
                            Qualifier::RelativeTo(file) | Qualifier::Presentation(file)
                                if file == *origin && !*formal =>
                            {
                                out.push_str(&self.store.local_type_name(origin, &selection)?);
                            }
                            Qualifier::FullPath => {
                                if !*formal {
                                    out.push_str(&self.store.workspace.import_path(origin)?);
                                    out.push('.');
                                }
                                out.push_str(
                                    &self.store.workspace.source_slice(origin, selection)?,
                                );
                            }
                            _ => return None,
                        }
                    }
                    TypeOrigin::Package(declaration) => {
                        match qualifier {
                            Qualifier::Presentation(file) => {
                                let same = file.parent() == declaration.file.parent()
                                    && self.store.workspace.files.get(file)?.package
                                        == self
                                            .store
                                            .workspace
                                            .files
                                            .get(declaration.file)?
                                            .package;
                                if !same {
                                    let import =
                                        self.store.workspace.import_path(declaration.file)?;
                                    let imported = self
                                        .store
                                        .workspace
                                        .files
                                        .get(file)?
                                        .imports
                                        .iter()
                                        .rev()
                                        .find(|candidate| {
                                            !candidate.is_blank() && candidate.path == import
                                        });
                                    if !imported.is_some_and(|import| import.is_dot()) {
                                        let name = imported
                                            .and_then(|import| {
                                                self.store.resolver().import_name(file, import)
                                            })
                                            .unwrap_or(
                                                &self
                                                    .store
                                                    .workspace
                                                    .files
                                                    .get(declaration.file)?
                                                    .package,
                                            );
                                        out.push_str(name);
                                        out.push('.');
                                    }
                                }
                                out.push_str(&declaration.symbol.name);
                            }
                            Qualifier::RelativeTo(file) => {
                                out.push_str(&crate::analysis::engine::resolve::result_type(
                                    self.store.workspace,
                                    file,
                                    declaration.file,
                                    &declaration.symbol.name,
                                )?)
                            }
                            Qualifier::FullPath => {
                                out.push_str(&self.store.workspace.import_path(declaration.file)?);
                                out.push('.');
                                out.push_str(&declaration.symbol.name);
                            }
                        }
                        if !arguments.is_empty() {
                            out.push('[');
                            for (index, argument) in
                                self.store.arguments_slice(id, arguments).iter().enumerate()
                            {
                                if index != 0 {
                                    out.push_str(", ");
                                }
                                self.write_type_in(*argument, qualifier, out, depth + 1)?;
                            }
                            out.push(']');
                        }
                    }
                }
            }
            Ty::Tuple(items) => {
                out.push('(');
                for (index, item) in self.store.arguments_slice(id, items).iter().enumerate() {
                    if index != 0 {
                        out.push_str(", ");
                    }
                    self.write_type_in(*item, qualifier, out, depth + 1)?;
                }
                out.push(')');
            }
            Ty::Pointer(element) | Ty::Slice(element) => {
                out.push_str(if matches!(self.store.node(id)?, Ty::Pointer(_)) {
                    "*"
                } else {
                    "[]"
                });
                self.write_type_in(element, qualifier, out, depth + 1)?;
            }
            Ty::Array { length, element } => {
                out.push('[');
                out.push_str(&length.to_string());
                out.push(']');
                self.write_type_in(element, qualifier, out, depth + 1)?;
            }
            Ty::Map { key, value } => {
                out.push_str("map[");
                self.write_type_in(key, qualifier, out, depth + 1)?;
                out.push(']');
                self.write_type_in(value, qualifier, out, depth + 1)?;
            }
            Ty::Chan { direction, element } => {
                out.push_str(match direction {
                    Direction::Both => "chan ",
                    Direction::Send => "chan<- ",
                    Direction::Receive => "<-chan ",
                });
                let parentheses = direction == Direction::Both
                    && matches!(
                        self.store.node(element),
                        Some(Ty::Chan {
                            direction: Direction::Receive,
                            ..
                        })
                    );
                if parentheses {
                    out.push('(');
                }
                self.write_type_in(element, qualifier, out, depth + 1)?;
                if parentheses {
                    out.push(')');
                }
            }
        }
        (out.len() <= MAX_BYTES).then_some(())
    }

    #[allow(clippy::too_many_arguments)]
    fn write_signature(
        &self,
        ty: TypeId,
        parameters: std::ops::Range<usize>,
        results: std::ops::Range<usize>,
        variadic: bool,
        qualifier: Qualifier<'_>,
        out: &mut String,
        depth: usize,
    ) -> Option<()> {
        out.push_str("func");
        let names = self.store.signature_name_text(ty).filter(|(_, p, r)| {
            usize::from(*p) == parameters.len() && usize::from(*r) == results.len()
        });
        let mut parameter_names = names.map(|(text, _, _)| text.split_terminator('\0').skip(1));
        let mut result_names =
            names.map(|(text, p, _)| text.split_terminator('\0').skip(1 + usize::from(p)));
        out.push('(');
        let count = parameters.len();
        for (index, param_ty) in self.store.arg_range(ty, parameters).iter().enumerate() {
            if index > 0 {
                out.push_str(", ");
            }
            if let Some(name) = parameter_names
                .as_mut()
                .and_then(Iterator::next)
                .filter(|name| !name.is_empty())
            {
                out.push_str(name);
                out.push(' ');
            }
            if variadic && index + 1 == count {
                let Some(Ty::Slice(element)) = self.store.node(*param_ty) else {
                    return None;
                };
                out.push_str("...");
                self.write_type_in(*element, qualifier, out, depth + 1)?;
            } else {
                self.write_type_in(*param_ty, qualifier, out, depth + 1)?;
            }
        }
        out.push(')');
        let first_result_name = result_names.as_mut().and_then(Iterator::next).unwrap_or("");
        let result_slice = self.store.arg_range(ty, results);
        if result_slice.len() == 1 && first_result_name.is_empty() {
            out.push(' ');
            self.write_type_in(result_slice[0], qualifier, out, depth + 1)?;
        } else if !result_slice.is_empty() {
            out.push_str(" (");
            for (index, res_ty) in result_slice.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                let name = if index == 0 {
                    first_result_name
                } else {
                    result_names.as_mut().and_then(Iterator::next).unwrap_or("")
                };
                if !name.is_empty() {
                    out.push_str(name);
                    out.push(' ');
                }
                self.write_type_in(*res_ty, qualifier, out, depth + 1)?;
            }
            out.push(')');
        }
        (out.len() <= MAX_BYTES).then_some(())
    }

    fn write_interface(
        &self,
        id: TypeId,
        data: &super::ty::InterfaceData<'_>,
        qualifier: Qualifier<'_>,
        out: &mut String,
        depth: usize,
    ) -> Option<()> {
        if data.builtin_any
            && data.methods.is_empty()
            && !data.comparable
            && !data.has_type_elements
        {
            out.push_str("any");
            return Some(());
        }
        out.push_str("interface{");
        let mut first = true;
        for method in self.store.methods_slice(id, data.methods.clone()) {
            if !first {
                out.push_str("; ");
            }
            first = false;
            if !crate::syntax::is_exported_name(&method.name)
                && let Qualifier::RelativeTo(file) = qualifier
                && !self.store.same_field_package(method.origin, file)?
            {
                return None;
            }
            out.push_str(&method.name);
            let mut signature = String::new();
            self.write_type_in(method.signature, qualifier, &mut signature, depth + 1)?;
            out.push_str(signature.strip_prefix("func")?);
            if out.len() > MAX_BYTES {
                return None;
            }
        }
        let embeddeds = self.store.arguments_slice(id, data.embeddeds);
        if data.comparable && embeddeds.is_empty() {
            if !first {
                out.push_str("; ");
            }
            first = false;
            out.push_str("comparable");
        }
        for &embedded in embeddeds {
            if !first {
                out.push_str("; ");
            }
            first = false;
            self.write_type_in(embedded, qualifier, out, depth + 1)?;
            if out.len() > MAX_BYTES {
                return None;
            }
        }
        out.push('}');
        Some(())
    }

    fn write_struct(
        &self,
        id: TypeId,
        origin: &Path,
        fields: std::ops::Range<usize>,
        qualifier: Qualifier<'_>,
        out: &mut String,
        depth: usize,
    ) -> Option<()> {
        let fields = self.store.fields_slice(id, fields)?;
        if fields
            .iter()
            .any(|field| !crate::syntax::is_exported_name(&field.name))
            && let Qualifier::RelativeTo(file) = qualifier
            && !self.store.same_field_package(origin, file)?
        {
            return None;
        }
        out.push_str("struct{");
        for (index, field) in fields.iter().enumerate() {
            if index > 0 {
                out.push_str("; ");
            }
            if !field.embedded {
                out.push_str(&field.name);
                out.push(' ');
            }
            let start = out.len();
            self.write_type_in(field.ty, qualifier, out, depth + 1)?;
            if field.embedded {
                let mut name = None;
                for token in Lexer::new(&out[start..]) {
                    if token.kind == TokenKind::LBrack {
                        break;
                    }
                    if token.kind == TokenKind::Ident {
                        name = Some(&out[start + token.span.start..start + token.span.end]);
                    }
                }
                if name != Some(field.name.as_str()) {
                    return None;
                }
            }
            if !field.tag.is_empty() {
                out.push_str(" \"");
                const HEX: &[u8] = b"0123456789abcdef";
                for byte in &field.tag {
                    if out.len() + 4 > MAX_BYTES {
                        return None;
                    }
                    out.push_str("\\x");
                    out.push(char::from(HEX[usize::from(byte >> 4)]));
                    out.push(char::from(HEX[usize::from(byte & 15)]));
                }
                out.push('"');
            }
        }
        out.push('}');
        (out.len() <= MAX_BYTES).then_some(())
    }
}

impl TypeStore<'_> {
    pub(crate) fn format_type(&self, id: TypeId, qualifier: Qualifier<'_>) -> Option<String> {
        TypeWriter::new(self, self.mode).format(id, qualifier)
    }

    pub(crate) fn render(&self, id: TypeId, consumer: &Path) -> Option<String> {
        self.format_type(id, Qualifier::RelativeTo(consumer))
    }

    /// Labels may mention a known package without adding it to the file imports.
    /// This text must not be parsed as a semantic fact in the consumer's scope.
    pub(crate) fn render_presentation(&self, id: TypeId, consumer: &Path) -> Option<String> {
        self.format_type(id, Qualifier::Presentation(consumer))
    }

    pub(crate) fn diagnostic_type(&self, id: TypeId) -> Option<String> {
        self.format_type(id, Qualifier::FullPath)
    }
}
