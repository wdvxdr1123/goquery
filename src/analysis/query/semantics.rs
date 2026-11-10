//! Read-only semantic operations over the admitted request source and metadata.
use super::{Context, cursor_scope};
use crate::analysis::check::callable_signature::CallableSignature;
use crate::analysis::check::shapes::Shapes;
use crate::analysis::check::{self, Inference, ScopedType};
use crate::analysis::scope::{Binding, CursorScope, Locals, Resolver};
use crate::analysis::source::{Name, Role, SourceAnalysis};
use crate::analysis::types::{TypeArena, TypeDeclaration};
use crate::syntax::{Expr, ExprId, ExprKind, Lexer, TokenKind};

impl<'a> Context<'a> {
    pub(crate) fn cursor_scope(&self, offset: usize) -> Result<CursorScope, (i32, String)> {
        cursor_scope(self.workspace, self.view.file, self.source(), offset)
    }

    pub(crate) fn type_position(&self, offset: usize, scope: &CursorScope) -> bool {
        check::target::type_position(self.workspace, self.view.file, self.source(), offset, scope)
    }

    pub(crate) fn binding_type(&self, locals: &Locals, binding: &Binding) -> Option<String> {
        self.workspace
            .binding_type(self.view.file, self.source(), locals, binding)
    }

    pub(crate) fn completion_receiver(
        &self,
        scope: &CursorScope,
        offset: usize,
    ) -> Option<ScopedType<'a>> {
        let body = scope.body.as_ref()?;
        let (id, _) = body
            .tree
            .expressions
            .iter()
            .enumerate()
            .find(|(_, expression)| {
                matches!(&expression.kind, ExprKind::Selector { field, .. }
                if (body.base + field.start..=body.base + field.end).contains(&offset))
            })?;
        Inference::new(
            self.workspace,
            self.view.file,
            self.source(),
            &scope.locals,
            &body.tree.expressions,
            body.base,
        )
        .scoped_receiver_type(ExprId(id))
    }

    pub(crate) fn callable_information(
        &self,
        locals: &Locals,
        body: (&[Expr], usize),
        function: ExprId,
    ) -> Option<(CallableSignature, bool)> {
        self.workspace
            .callable_information(self.view.file, self.source(), locals, body, function)
    }

    pub(crate) fn type_declarations(
        &mut self,
        offset: usize,
    ) -> Result<Vec<TypeDeclaration<'a>>, (i32, String)> {
        let arena = self
            .arena
            .get_or_insert_with(|| TypeArena::new(self.workspace));
        check::type_definition::declarations(
            self.workspace,
            self.view.file,
            self.view.source.text(),
            offset,
            arena,
        )
    }
    pub(crate) fn semantic_analysis(&self) -> Result<SourceAnalysis, (i32, String)> {
        let mut analysis =
            SourceAnalysis::for_semantic(self.source(), &self.workspace.files[self.view.file])?;
        // Aggregate members written in anonymous type literals are declarations:
        // semantic tokens report them as property/definition like gopls. The
        // anonymous owner has no package-level symbol, so a sentinel owner is
        // resolved to no identity rather than to an unrelated declaration.
        for name in &mut analysis.names {
            if matches!(name.role, Role::Field) {
                name.role = Role::FieldDeclaration(usize::MAX);
            }
        }
        analysis.names.extend(self.import_names(self.source()));
        analysis.names.sort_unstable_by_key(|name| name.span.start);
        Ok(analysis)
    }

    pub(crate) fn resolver(&self) -> Resolver<'a> {
        Resolver::new(self.workspace, self.view.file)
    }

    pub(crate) fn shapes<'query>(&'query self, analysis: &'query SourceAnalysis) -> Shapes<'query> {
        Shapes::new(self.workspace, analysis, self.view.file, self.source())
    }

    // Match a default import's virtual declaration against the dependency's
    // actual package name, rather than assuming the import-path suffix.
    fn import_names(&self, source: &str) -> Vec<Name> {
        let mut names = Vec::new();
        let mut importing = false;
        let mut grouped = false;
        let mut explicit = false;
        for token in Lexer::new(source) {
            if token.kind == TokenKind::Import {
                importing = true;
                explicit = false;
                continue;
            }
            if !importing {
                if matches!(
                    token.kind,
                    TokenKind::Func | TokenKind::Type | TokenKind::Var | TokenKind::Const
                ) {
                    break;
                }
                continue;
            }
            match token.kind {
                TokenKind::LParen => grouped = true,
                TokenKind::RParen => {
                    importing = false;
                    grouped = false;
                }
                TokenKind::Ident | TokenKind::Period => explicit = true,
                TokenKind::Semicolon => {
                    explicit = false;
                    if !grouped {
                        importing = false;
                    }
                }
                TokenKind::String => {
                    if !explicit {
                        let literal = &source[token.span.clone()];
                        let package =
                            crate::syntax::decode_import_path(literal).and_then(|mut path| {
                                self.workspace
                                    .sdk_import_identity(self.view.file, &mut path);
                                self.workspace.declared_package_name(&path)
                            });
                        if let Some(package) = package
                            && let Some(offset) = literal.rfind(package)
                        {
                            names.push(Name {
                                span: token.span.start + offset
                                    ..token.span.start + offset + package.len(),
                                role: Role::Namespace,
                            });
                        }
                    }
                    explicit = false;
                    if !grouped {
                        importing = false;
                    }
                }
                _ => {}
            }
        }
        names
    }
}
