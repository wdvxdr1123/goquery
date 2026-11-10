//! Bounded, query-local named type expansion; declaration identity stays separate.
mod context;
mod implementation;
pub(crate) mod signature;
use crate::analysis::member::MemberDeclaration;
use crate::analysis::scope as locals;
use crate::analysis::types::{TypeArena, TypeForm, TypePresentation, declaration_body};
use crate::index::IndexedSymbol as Symbol;
use crate::text::SourceMap;
use crate::workspace::Workspace;
use context::{OwnerKind, TypeContext};
use std::path::{Path, PathBuf};

impl Workspace {
    /// Requalify every named reference without expanding aliases or definitions.
    pub(crate) fn requalified_type(
        &self,
        file: &Path,
        origin: &Path,
        annotation: &str,
    ) -> Option<String> {
        let origin = self.files.get_key_value(origin)?.0;
        // Rendering calls result_type only for individual, uninstantiated names;
        // arguments are separate references, so this does not recurse on itself.
        TypeContext::new(self).render_text(origin, annotation, None, file)
    }

    pub(crate) fn declared_type_presentation(
        &self,
        file: &Path,
        origin: &Path,
        declaration: &str,
    ) -> Option<TypePresentation> {
        if declaration.len() > 16 * 1024 {
            return None;
        }
        let origin = self.files.get_key_value(origin)?.0;
        TypeContext::new(self).declaration_presentation(origin, declaration, file)
    }

    pub(crate) fn local_type_presentation(
        &self,
        file: &Path,
        source: &str,
        locals: &locals::Locals,
        binding: &locals::Binding,
    ) -> Option<TypePresentation> {
        if binding.declaration == locals::DeclarationKind::TypeParameter {
            return Some(TypePresentation {
                form: TypeForm::Interface,
                text: "interface{...}".to_owned(),
            });
        }
        if binding.declaration != locals::DeclarationKind::Type {
            return None;
        }
        let mut span = binding.annotation()?.clone();
        for _ in 0..32 {
            let annotation = source.get(span.clone())?;
            if annotation.len() > 16 * 1024 {
                return None;
            }
            let parsed = crate::syntax::parse_type(annotation);
            if !parsed.diagnostics.is_empty() {
                return None;
            }
            let mut root = annotation;
            for _ in 0..32 {
                let first = crate::syntax::Lexer::new(root).next()?;
                if first.kind != crate::syntax::TokenKind::LParen {
                    break;
                }
                let last = crate::syntax::Lexer::new(root)
                    .filter(|token| token.kind != crate::syntax::TokenKind::Semicolon)
                    .last()?;
                if last.kind != crate::syntax::TokenKind::RParen {
                    return None;
                }
                root = root.get(first.span.end..last.span.start)?;
            }
            if crate::syntax::Lexer::new(root).next()?.kind == crate::syntax::TokenKind::Ident
                && let Some(name) = locals::named_type(root)
                && let Some(local) =
                    locals.resolve_at(span.start, locals::named_root(&name).split('.').next()?)
            {
                if local.declaration != locals::DeclarationKind::Type {
                    return None;
                }
                span = local.annotation()?.clone();
                continue;
            }
            let names: Vec<_> = locals
                .visible_bindings(span.start)
                .filter(|binding| {
                    binding.object_kind.is_type()
                        && locals
                            .resolve_at(span.start, &binding.name)
                            .is_some_and(|actual| actual.selection == binding.selection)
                })
                .map(|binding| binding.name.clone())
                .collect();
            let file = self.files.get_key_value(file)?.0;
            return TypeContext::new(self).local_presentation(file, annotation, &names);
        }
        None
    }
    pub(crate) fn completion_inferable(
        &self,
        file: &Path,
        formals: &[(&str, &str)],
        inputs: &[&str],
    ) -> Option<Vec<bool>> {
        let file = self.files.get_key_value(file)?.0;
        TypeContext::new(self).inferable(file, formals, inputs)
    }
    pub(crate) fn range_type(
        &self,
        file: &Path,
        text: &str,
        locals: &locals::Locals,
        offset: usize,
        result: usize,
        count: usize,
    ) -> Option<String> {
        if text.len() > 16 * 1024 || count > 2 || result >= count {
            return None;
        }
        if let Some(name) = locals::named_type(text)
            && locals
                .resolve_at(offset, locals::named_root(&name).split('.').next()?)
                .is_some()
        {
            return None;
        }
        let file = self.files.get_key_value(file)?.0;
        let mut context = TypeContext::new(self);
        let input = context.push(file, text, None)?;
        let ty = context
            .range_type(input, result, count, file)?
            .trim()
            .to_owned();
        if context
            .argument_roots
            .iter()
            .any(|(origin, root)| *origin == file && locals.resolve_at(offset, root).is_some())
        {
            return None;
        }
        if let Some(name) = locals::named_type(&ty)
            && locals
                .resolve_at(offset, locals::named_root(&name).split('.').next()?)
                .is_some()
        {
            return None;
        }
        Some(ty)
    }
    /// Render a named type's underlying declaration in the consumer's scope.
    pub(crate) fn instantiated_underlying_type(&self, file: &Path, name: &str) -> Option<String> {
        let mut context = TypeContext::new(self);
        let file = self.files.get_key_value(file)?.0;
        let input = context.push(file, name, None)?;
        let resolved = context.resolve(input, Resolution::Underlying)?;
        let environment = context.environment(&resolved)?;
        let source = self.source(resolved.file)?;
        let map = SourceMap::new(&source);
        let declaration =
            source.get(resolved.symbol.end..map.offset(resolved.symbol.range.end)?)?;
        if declaration.len() > 16 * 1024 {
            return None;
        }
        context.render_text(
            resolved.file,
            declaration_body(declaration)?,
            environment,
            file,
        )
    }

    pub(crate) fn type_form(
        &self,
        file: &Path,
        text: &str,
        locals: &locals::Locals,
        offset: usize,
    ) -> Option<TypeForm> {
        if text.len() > 16 * 1024 {
            return None;
        }
        let mut root = text;
        for _ in 0..32 {
            let first = crate::syntax::Lexer::new(root).next()?;
            if first.kind != crate::syntax::TokenKind::LParen {
                break;
            }
            let last = crate::syntax::Lexer::new(root)
                .filter(|token| token.kind != crate::syntax::TokenKind::Semicolon)
                .last()?;
            if last.kind != crate::syntax::TokenKind::RParen {
                return None;
            }
            root = root.get(first.span.end..last.span.start)?;
        }
        if crate::syntax::Lexer::new(root).next()?.kind == crate::syntax::TokenKind::LParen {
            return None;
        }
        if crate::syntax::Lexer::new(root).next()?.kind == crate::syntax::TokenKind::Ident
            && locals::named_type(root).is_some_and(|name| {
                locals
                    .resolve_at(
                        offset,
                        locals::named_root(&name).split('.').next().unwrap_or(""),
                    )
                    .is_some()
            })
        {
            return None;
        }
        let parsed = crate::syntax::parse_type(text);
        if !parsed.diagnostics.is_empty() {
            return None;
        }
        let shadowed = parsed.references.iter().any(|reference| {
            let span = reference.qualifier.as_ref().unwrap_or(&reference.name);
            locals.resolve_at(offset, &text[span.clone()]).is_some()
        });
        if !shadowed {
            let mut arena = TypeArena::new(self);
            if let Some(input) = arena.parse(file, text)
                && let Some(form) = arena.form(input)
            {
                return Some(form);
            }
            if arena.has_type_error() {
                return None;
            }
        }
        let mut context = TypeContext::new(self);
        let file = self.files.get_key_value(file)?.0;
        let input = context.push(file, text, None)?;
        let form = context.form(input, 0)?;
        if context
            .argument_roots
            .iter()
            .any(|(origin, root)| *origin == file && locals.resolve_at(offset, root).is_some())
        {
            return None;
        }
        Some(form)
    }

    /// Resolve the written type name without erasing alias declarations.
    pub(crate) fn type_declaration(&self, file: &Path, name: &str) -> Option<(&PathBuf, &Symbol)> {
        let name = locals::named_root(name);
        if let Some((alias, name)) = name.split_once('.') {
            self.imported_symbol_candidates(
                file,
                alias,
                Some(crate::bloom::Fingerprint::new(name.as_bytes())),
            )
            .find(|(_, symbol)| symbol.is_type() && symbol.name == name)
        } else {
            self.unqualified_symbol(file, name)
                .filter(|(_, symbol)| symbol.is_type())
        }
    }

    /// Member lookup follows aliases but not distinct defined types.
    pub(crate) fn embedded_target(&self, file: &Path, name: &str) -> Option<(&PathBuf, &Symbol)> {
        let direct = self.type_declaration(file, name)?;
        if direct
            .1
            .type_info
            .as_ref()
            .and_then(|info| info.target.as_ref())
            .filter(|target| target.alias)
            .is_none()
        {
            return Some(direct);
        }
        let mut context = TypeContext::new(self);
        let file = self.files.get_key_value(file)?.0;
        let input = context.push(file, name, None)?;
        let resolved = context.resolve(input, Resolution::Aliases)?;
        Some((resolved.file, resolved.symbol))
    }

    /// Structural fields/interface methods follow definitions without inheriting concrete methods.
    pub(crate) fn underlying_target(&self, file: &Path, name: &str) -> Option<(&PathBuf, &Symbol)> {
        self.structural_target(file, name, Resolution::Underlying)
    }

    /// Composite keys require a non-pointer aggregate underlying type.
    pub(crate) fn composite_target(&self, file: &Path, name: &str) -> Option<(&PathBuf, &Symbol)> {
        self.structural_target(file, name, Resolution::Composite)
    }

    fn structural_target(
        &self,
        file: &Path,
        name: &str,
        resolution: Resolution,
    ) -> Option<(&PathBuf, &Symbol)> {
        let direct = self.type_declaration(file, name)?;
        if direct
            .1
            .type_info
            .as_ref()
            .and_then(|info| info.target.as_ref())
            .is_none()
        {
            return Some(direct);
        }
        let mut context = TypeContext::new(self);
        let file = self.files.get_key_value(file)?.0;
        let input = context.push(file, name, None)?;
        let resolved = context.resolve(input, resolution)?;
        Some((resolved.file, resolved.symbol))
    }

    /// An argument used as the receiver root must not resolve through a local
    /// type parameter to an unrelated same-spelled package declaration.
    pub(crate) fn receiver_in_scope(
        &self,
        file: &Path,
        name: &str,
        locals: &locals::Locals,
        offset: usize,
    ) -> bool {
        if let Some(name) = locals::named_type(name)
            && locals
                .resolve_at(
                    offset,
                    locals::named_root(&name).split('.').next().unwrap_or(""),
                )
                .is_some_and(|binding| binding.object_kind.is_type())
        {
            return false;
        }
        if !name.contains('[')
            || !locals
                .visible_bindings(offset)
                .any(|binding| binding.object_kind.is_type())
            || !self
                .type_declaration(file, name)
                .is_some_and(|(_, symbol)| {
                    symbol
                        .type_info
                        .as_ref()
                        .is_some_and(|info| info.target.is_some())
                })
        {
            return true;
        }
        let Some((file, _)) = self.files.get_key_value(file) else {
            return false;
        };
        let mut context = TypeContext::new(self);
        let Some(input) = context.push(file, name, None) else {
            return false;
        };
        let _ = context.resolve(input, Resolution::Underlying);
        !context.argument_roots.iter().any(|(origin, name)| {
            *origin == file
                && locals
                    .resolve_at(offset, name.split('.').next().expect("named root"))
                    .is_some_and(|binding| binding.object_kind.is_type())
        })
    }

    pub(crate) fn instantiated_member_type(
        &self,
        file: &Path,
        receiver: &str,
        member: MemberDeclaration<'_>,
        annotation: &str,
    ) -> Option<String> {
        self.member_types(file, receiver, member)?
            .render(annotation)
    }

    pub(crate) fn member_types(
        &self,
        file: &Path,
        receiver: &str,
        member: MemberDeclaration<'_>,
    ) -> Option<MemberTypes<'_>> {
        self.scoped_member_types(file, file, receiver, member)
    }

    pub(crate) fn scoped_member_types(
        &self,
        file: &Path,
        scope: &Path,
        receiver: &str,
        member: MemberDeclaration<'_>,
    ) -> Option<MemberTypes<'_>> {
        let mut context = TypeContext::new(self);
        let file = self.files.get_key_value(file)?.0;
        let scope = self.files.get_key_value(scope)?.0;
        let input = context.push(scope, receiver, None)?;
        let (origin, environment) =
            context.owner_environment(input, member.file, member.owner, OwnerKind::Aggregate)?;
        Some(MemberTypes {
            context,
            origin,
            environment,
            file,
        })
    }

    pub(crate) fn method_types(
        &self,
        file: &Path,
        receiver: &str,
        origin: &Path,
        method: &Symbol,
        names: Vec<String>,
    ) -> Option<MemberTypes<'_>> {
        self.scoped_method_types(file, file, receiver, origin, method, names)
    }

    pub(crate) fn scoped_method_types(
        &self,
        file: &Path,
        scope: &Path,
        receiver: &str,
        origin: &Path,
        method: &Symbol,
        names: Vec<String>,
    ) -> Option<MemberTypes<'_>> {
        let (owner_file, owner) = self.type_declaration(origin, method.receiver.as_deref()?)?;
        let mut context = TypeContext::new(self);
        let file = self.files.get_key_value(file)?.0;
        let scope = self.files.get_key_value(scope)?.0;
        let input = context.push(scope, receiver, None)?;
        let (_, environment) =
            context.owner_environment(input, owner_file, owner, OwnerKind::Receiver)?;
        let environment = context.renamed_environment(environment, names)?;
        let origin = self.files.get_key_value(origin)?.0;
        Some(MemberTypes {
            context,
            origin,
            environment,
            file,
        })
    }
}

#[derive(Clone, Copy)]
enum Resolution {
    Aliases,
    Underlying,
    Composite,
}

/// Prepared only for one selected owner during a completion/query batch.
pub(crate) struct MemberTypes<'a> {
    context: TypeContext<'a>,
    origin: &'a PathBuf,
    environment: Option<usize>,
    file: &'a PathBuf,
}

impl MemberTypes<'_> {
    pub(crate) fn signature_identity(&mut self, header: &str) -> Option<String> {
        self.context
            .signature_identity(self.origin, header, self.environment, 0)
    }

    pub(crate) fn form(
        &mut self,
        annotation: &str,
        locals: &locals::Locals,
        offset: usize,
    ) -> Option<TypeForm> {
        let input = self
            .context
            .push(self.origin, annotation, self.environment)?;
        let form = self.context.form(input, 0)?;
        if self
            .context
            .argument_roots
            .iter()
            .any(|(origin, root)| *origin == self.file && locals.resolve_at(offset, root).is_some())
        {
            return None;
        }
        Some(form)
    }

    pub(crate) fn render(&self, annotation: &str) -> Option<String> {
        self.context
            .render_text(self.origin, annotation, self.environment, self.file)
    }

    pub(crate) fn rename_parameters(&mut self, names: Vec<String>) -> Option<()> {
        self.environment = self.context.renamed_environment(self.environment, names)?;
        Some(())
    }
}
