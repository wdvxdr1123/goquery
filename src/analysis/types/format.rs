//! Unified `go/types.TypeString` formatting controlled by `Qualifier`.

use super::{BasicKind, Direction, MAX_BYTES, MAX_DEPTH, Ty, TypeArena, TypeId};
use crate::syntax::{Lexer, TokenKind};
use std::path::Path;

/// Controls package qualification when rendering Go type spellings.
#[derive(Clone, Copy)]
pub(crate) enum Qualifier<'p> {
    RelativeTo(&'p Path),
    Presentation(&'p Path),
    FullPath,
    Unqualified,
}

impl TypeArena<'_> {
    pub(crate) fn format_type(&self, id: TypeId, qualifier: Qualifier<'_>) -> Option<String> {
        let mut output = String::new();
        self.write_type_in(id, qualifier, &mut output, 0)?;
        Some(output)
    }

    pub(super) fn write_type_in(
        &self,
        id: TypeId,
        qualifier: Qualifier<'_>,
        out: &mut String,
        depth: usize,
    ) -> Option<()> {
        if depth >= MAX_DEPTH {
            return None;
        }
        match self.nodes.get(usize::from(id.0))? {
            Ty::Parameter(_) => {
                let (origin, _, name) = self.local_parameter(id)?;
                if !self.symbolic
                    && matches!(qualifier, Qualifier::RelativeTo(file) | Qualifier::Presentation(file) if file != origin)
                {
                    return None;
                }
                out.push_str(name);
            }
            Ty::LocalDeclaration {
                origin,
                selection,
                formal,
            } => match qualifier {
                Qualifier::RelativeTo(file) | Qualifier::Presentation(file)
                    if file == *origin && !*formal =>
                {
                    out.push_str(&self.local_type_name(origin, selection)?);
                }
                Qualifier::FullPath => {
                    if !formal {
                        out.push_str(&self.workspace.import_path(origin)?);
                        out.push('.');
                    }
                    out.push_str(&self.workspace.source_slice(origin, selection.clone())?);
                }
                Qualifier::Unqualified if !*formal => {
                    out.push_str(&self.local_type_name(origin, selection)?);
                }
                Qualifier::Unqualified => {
                    out.push_str(&self.workspace.source_slice(origin, selection.clone())?);
                }
                _ => return None,
            },
            Ty::Error(_) => out.push_str("error"),
            Ty::Interface {
                origin,
                methods,
                builtin_any,
            } => {
                self.write_interface(origin, methods.clone(), *builtin_any, qualifier, out, depth)?
            }
            Ty::Signature {
                parameters,
                results,
                variadic,
            } => self.write_signature(
                parameters.clone(),
                results.clone(),
                *variadic,
                qualifier,
                out,
                depth,
            )?,
            Ty::Struct { origin, fields } => {
                self.write_struct(origin, fields.clone(), qualifier, out, depth)?
            }
            Ty::Basic(BasicKind::UnsafePointer) => match qualifier {
                Qualifier::Unqualified => out.push_str("Pointer"),
                Qualifier::FullPath => out.push_str("unsafe.Pointer"),
                Qualifier::Presentation(file) => {
                    if self.workspace.import_path(file).as_deref() == Some("unsafe") {
                        out.push_str("Pointer");
                    } else {
                        let imported = self
                            .workspace
                            .files
                            .get(file)?
                            .imports
                            .iter()
                            .rev()
                            .find(|c| !c.is_blank() && c.path == "unsafe");
                        if !imported.is_some_and(|i| i.is_dot()) {
                            let name = imported
                                .and_then(|i| self.workspace.import_name(i))
                                .unwrap_or("unsafe");
                            out.push_str(name);
                            out.push('.');
                        }
                        out.push_str("Pointer");
                    }
                }
                Qualifier::RelativeTo(file) => {
                    if let Some(unsafe_file) = self
                        .workspace
                        .builtin_file()
                        .and_then(|b| Some(b.parent()?.parent()?.join("unsafe/unsafe.go")))
                        && self.workspace.files.contains_key(&unsafe_file)
                    {
                        out.push_str(&crate::analysis::resolve::result_type(
                            self.workspace,
                            file,
                            &unsafe_file,
                            "Pointer",
                        )?);
                    } else if self.workspace.import_path(file).as_deref() == Some("unsafe") {
                        out.push_str("Pointer");
                    } else {
                        let imported = self
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
                            out.push_str(self.workspace.import_name(imported)?);
                            out.push_str(".Pointer");
                        }
                    }
                }
            },
            Ty::Basic(kind) => out.push_str(kind.name()),
            Ty::Named {
                declaration,
                arguments,
            }
            | Ty::Alias {
                declaration,
                arguments,
            } => {
                match qualifier {
                    Qualifier::Presentation(file) => {
                        let same = file.parent() == declaration.file.parent()
                            && self.workspace.files.get(file)?.package
                                == self.workspace.files.get(declaration.file)?.package;
                        if !same {
                            let import = self.workspace.import_path(declaration.file)?;
                            let imported =
                                self.workspace.files.get(file)?.imports.iter().rev().find(
                                    |candidate| !candidate.is_blank() && candidate.path == import,
                                );
                            if !imported.is_some_and(|import| import.is_dot()) {
                                let name = imported
                                    .and_then(|import| self.workspace.import_name(import))
                                    .unwrap_or(
                                        &self.workspace.files.get(declaration.file)?.package,
                                    );
                                out.push_str(name);
                                out.push('.');
                            }
                        }
                        out.push_str(&declaration.symbol.name);
                    }
                    Qualifier::RelativeTo(file) => {
                        out.push_str(&crate::analysis::resolve::result_type(
                            self.workspace,
                            file,
                            declaration.file,
                            &declaration.symbol.name,
                        )?)
                    }
                    Qualifier::FullPath => {
                        out.push_str(&self.workspace.import_path(declaration.file)?);
                        out.push('.');
                        out.push_str(&declaration.symbol.name);
                    }
                    Qualifier::Unqualified => out.push_str(&declaration.symbol.name),
                }
                if !arguments.is_empty() {
                    out.push('[');
                    for (index, argument) in self.arguments[arguments.range()].iter().enumerate() {
                        if index != 0 {
                            out.push_str(", ");
                        }
                        self.write_type_in(*argument, qualifier, out, depth + 1)?;
                    }
                    out.push(']');
                }
            }
            Ty::Tuple(items) => {
                out.push('(');
                for (index, item) in self.arguments[items.range()].iter().enumerate() {
                    if index != 0 {
                        out.push_str(", ");
                    }
                    self.write_type_in(*item, qualifier, out, depth + 1)?;
                }
                out.push(')');
            }
            Ty::Pointer(element) | Ty::Slice(element) => {
                out.push_str(if matches!(self.nodes[usize::from(id.0)], Ty::Pointer(_)) {
                    "*"
                } else {
                    "[]"
                });
                self.write_type_in(*element, qualifier, out, depth + 1)?;
            }
            Ty::Array { length, element } => {
                out.push('[');
                out.push_str(&length.to_string());
                out.push(']');
                self.write_type_in(*element, qualifier, out, depth + 1)?;
            }
            Ty::Map { key, value } => {
                out.push_str("map[");
                self.write_type_in(*key, qualifier, out, depth + 1)?;
                out.push(']');
                self.write_type_in(*value, qualifier, out, depth + 1)?;
            }
            Ty::Chan { direction, element } => {
                out.push_str(match direction {
                    Direction::Both => "chan ",
                    Direction::Send => "chan<- ",
                    Direction::Receive => "<-chan ",
                });
                let parentheses = *direction == Direction::Both
                    && matches!(
                        self.nodes.get(usize::from(element.0)),
                        Some(Ty::Chan {
                            direction: Direction::Receive,
                            ..
                        })
                    );
                if parentheses {
                    out.push('(');
                }
                self.write_type_in(*element, qualifier, out, depth + 1)?;
                if parentheses {
                    out.push(')');
                }
            }
        }
        (out.len() <= MAX_BYTES).then_some(())
    }

    fn write_signature(
        &self,
        parameters: std::ops::Range<usize>,
        results: std::ops::Range<usize>,
        variadic: bool,
        qualifier: Qualifier<'_>,
        out: &mut String,
        depth: usize,
    ) -> Option<()> {
        out.push_str("func");
        self.write_signature_params_results(parameters, results, variadic, qualifier, out, depth)
    }

    fn write_signature_params_results(
        &self,
        parameters: std::ops::Range<usize>,
        results: std::ops::Range<usize>,
        variadic: bool,
        qualifier: Qualifier<'_>,
        out: &mut String,
        depth: usize,
    ) -> Option<()> {
        out.push('(');
        let count = parameters.len();
        for (index, ty) in self.arguments[parameters].iter().enumerate() {
            if index > 0 {
                out.push_str(", ");
            }
            if variadic && index + 1 == count {
                let Ty::Slice(element) = self.nodes[usize::from(ty.0)] else {
                    return None;
                };
                out.push_str("...");
                self.write_type_in(element, qualifier, out, depth + 1)?;
            } else {
                self.write_type_in(*ty, qualifier, out, depth + 1)?;
            }
        }
        out.push(')');
        if !results.is_empty() {
            out.push_str(" (");
            for (index, ty) in self.arguments[results].iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                self.write_type_in(*ty, qualifier, out, depth + 1)?;
            }
            out.push(')');
        }
        (out.len() <= MAX_BYTES).then_some(())
    }

    fn write_interface(
        &self,
        _origin: &Path,
        methods: std::ops::Range<usize>,
        builtin_any: bool,
        qualifier: Qualifier<'_>,
        out: &mut String,
        depth: usize,
    ) -> Option<()> {
        if builtin_any && methods.is_empty() {
            out.push_str("any");
            return Some(());
        }
        out.push_str("interface{");
        for method in &self.methods[methods] {
            if !crate::syntax::is_exported_name(&method.name)
                && let Qualifier::RelativeTo(file) = qualifier
                && !self.same_field_package(method.origin, file)?
            {
                return None;
            }
            out.push_str(&method.name);
            let mut signature = String::new();
            self.write_type_in(method.signature, qualifier, &mut signature, depth + 1)?;
            out.push_str(signature.strip_prefix("func")?);
            out.push(';');
            if out.len() > MAX_BYTES {
                return None;
            }
        }
        out.push('}');
        Some(())
    }

    fn write_struct(
        &self,
        origin: &Path,
        fields: std::ops::Range<usize>,
        qualifier: Qualifier<'_>,
        out: &mut String,
        depth: usize,
    ) -> Option<()> {
        if self.fields[fields.clone()]
            .iter()
            .any(|field| !crate::syntax::is_exported_name(&field.name))
            && let Qualifier::RelativeTo(file) = qualifier
            && !self.same_field_package(origin, file)?
        {
            return None;
        }
        out.push_str("struct { ");
        for field in &self.fields[fields] {
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
            out.push_str("; ");
        }
        out.push('}');
        (out.len() <= MAX_BYTES).then_some(())
    }
}
