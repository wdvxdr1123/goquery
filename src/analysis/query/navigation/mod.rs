//! Navigation and reference seeds share the request's immutable source view.
pub(in crate::analysis::query) mod anonymous;
mod decl_targets;
use super::Context;
use crate::analysis::obj::MemberDeclId;
use crate::analysis::obj::{MethodOrigin, ObjectRef, SymbolOrigin};
use crate::analysis::resolve::NameProbe;
use crate::analysis::scope::Locals;
use crate::analysis::scope::occurrences::{Occurrences, Role};
use crate::analysis::sema::{Identity, IdentityResolver};
use crate::index::IndexedSymbol as Symbol;
use crate::syntax::{Lexer, TokenKind};
use crate::text::SourceMap;
pub(in crate::analysis::query) use decl_targets::member_declaration_target;
use std::borrow::Cow;
use std::ops::Range;
use std::path::{Path, PathBuf};

pub(crate) enum Target<'a> {
    Unresolved,
    Local(Range<usize>),
    Global(&'a PathBuf, &'a Symbol),
    Member(MemberDeclId<'a>),
}

pub(crate) struct NavigationTarget<'a> {
    pub(crate) target: Target<'a>,
    pub(crate) fallback: bool,
    /// Predeclared identifier that resolves to the configured SDK's builtin.go
    /// declaration; the feature projects the name to its SDK location.
    pub(crate) builtin: Option<String>,
    /// Cross-file location for targets that are not indexed symbols (the
    /// package clause of another compiled file).
    pub(crate) location: Option<(PathBuf, Range<usize>)>,
}

pub(crate) enum ReferenceTarget<'a> {
    Global(&'a PathBuf, &'a Symbol),
    Member(MemberDeclId<'a>),
    Local(Vec<Range<usize>>),
    AnonymousField(anonymous::InlineField),
    Unresolved,
}

/// One import spec, in source order, paired with its package metadata name.
struct ImportSite {
    alias: Option<Range<usize>>,
    path: Range<usize>,
    name: Option<String>,
    import_path: String,
}

/// gopls links a qualifier to the import path literal when the local name equals
/// the imported package's declared name; otherwise to the explicit alias name.
/// The equal-name form mirrors gopls' `PkgName` range heuristic: range start at
/// the spec's name position (the alias when written) and length `len(path)+2`.
fn import_qualifier_target(context: &Context<'_>, site: &ImportSite, source: &str) -> Range<usize> {
    match &site.alias {
        Some(alias) => {
            let declared = context
                .resolver()
                .declared_package_name(context.file(), &site.import_path);
            if declared.is_some_and(|name| name == &source[alias.clone()]) {
                let end = (alias.start + site.import_path.len() + 2).min(source.len());
                alias.start..end
            } else {
                alias.clone()
            }
        }
        None => site.path.clone(),
    }
}

/// Import specs in source order. `path` is the quoted literal span; `name` is
/// the package identifier the file uses for it (explicit alias or the imported
/// package's declared name from indexed metadata, never a path suffix guess).
fn import_sites(context: &Context<'_>, source: &str) -> Vec<ImportSite> {
    let workspace = context.workspace();
    let file = context.file();
    let mut sites = Vec::new();
    let mut tokens = Lexer::new(source);
    while let Some(token) = tokens.next() {
        if token.kind != TokenKind::Import {
            continue;
        }
        match tokens.next().map(|token| (token.kind, token.span)) {
            Some((TokenKind::String, path)) => sites.push(ImportSite {
                alias: None,
                path,
                name: None,
                import_path: String::new(),
            }),
            Some((TokenKind::LParen, _)) => {
                while let Some(token) = tokens.next() {
                    match token.kind {
                        TokenKind::RParen => break,
                        TokenKind::String => sites.push(ImportSite {
                            alias: None,
                            path: token.span,
                            name: None,
                            import_path: String::new(),
                        }),
                        TokenKind::Ident | TokenKind::Period => {
                            let Some(path) = tokens.next() else { break };
                            if path.kind == TokenKind::String {
                                sites.push(ImportSite {
                                    alias: Some(token.span),
                                    path: path.span,
                                    name: None,
                                    import_path: String::new(),
                                });
                            }
                        }
                        _ => {}
                    }
                }
            }
            Some((TokenKind::Ident | TokenKind::Period, alias)) => {
                let Some(path) = tokens.next() else { continue };
                if path.kind == TokenKind::String {
                    sites.push(ImportSite {
                        alias: Some(alias),
                        path: path.span,
                        name: None,
                        import_path: String::new(),
                    });
                }
            }
            _ => {}
        }
    }
    let imports = workspace
        .files
        .get(file)
        .map_or(&[][..], |stub| stub.imports.as_slice());
    for (site, import) in sites.iter_mut().zip(imports) {
        site.import_path = import.path.clone();
        if let Some(alias) = &site.alias
            && matches!(&source[alias.clone()], "_" | ".")
        {
            continue;
        }
        site.name = import
            .explicit_name()
            .or_else(|| context.resolver().declared_package_name(file, &import.path))
            .map(str::to_owned);
    }
    sites
}

/// Identifier span of the package clause name, if this file has one.
fn package_clause_name(source: &str) -> Option<Range<usize>> {
    let mut tokens = Lexer::new(source);
    loop {
        match tokens.next()? {
            token if token.kind == TokenKind::Package => {
                let name = tokens.next()?;
                return (name.kind == TokenKind::Ident).then_some(name.span);
            }
            token if token.kind == TokenKind::Semicolon => continue,
            _ => return None,
        }
    }
}

/// True when a comment group ends on the line directly before the package
/// keyword, which is how go/parser attaches a package doc comment.
fn has_package_doc(source: &str) -> bool {
    let mut last_comment: Option<Range<usize>> = None;
    let mut package_start = None;
    for token in Lexer::with_comments(source) {
        match token.kind {
            TokenKind::Comment => last_comment = Some(token.span),
            TokenKind::Package => {
                package_start = Some(token.span.start);
                break;
            }
            TokenKind::Semicolon => continue,
            _ => break,
        }
    }
    let (Some(comment), Some(package_start)) = (last_comment, package_start) else {
        return false;
    };
    let line = |offset: usize| {
        source[..offset]
            .bytes()
            .filter(|byte| *byte == b'\n')
            .count()
    };
    line(package_start) == line(comment.end) + 1
}

/// gopls links the package clause to the first compiled file that documents the
/// package; file order follows the indexed path order of the same package.
fn package_definition(
    context: &Context<'_>,
    file: &Path,
    source: &str,
) -> Option<(PathBuf, Range<usize>)> {
    let workspace = context.workspace();
    if workspace.is_detached(file) && !crate::workspace::production_file(file) {
        return None;
    }
    let view = context.build_view.get()?;
    let mut read_bytes = 0usize;
    for (candidate, _) in workspace.package_files_in_view(file, view).take(256) {
        let remaining = (1024 * 1024usize).checked_sub(read_bytes)?;
        if remaining == 0 {
            return None;
        }
        let limit = remaining.min(64 * 1024);
        let text = if candidate == file {
            let mut end = source.len().min(limit);
            while !source.is_char_boundary(end) {
                end -= 1;
            }
            Cow::Borrowed(&source[..end])
        } else {
            let Some(source) = workspace.source_prefix(candidate, limit) else {
                continue;
            };
            Cow::Owned(source)
        };
        read_bytes += text.len();
        if has_package_doc(&text) {
            return (candidate != file)
                .then(|| Some((candidate.clone(), package_clause_name(&text)?)))
                .flatten();
        }
    }
    None
}

impl<'a> Context<'a> {
    pub(crate) fn navigation_target(
        &self,
        offset: usize,
    ) -> Result<NavigationTarget<'a>, (i32, String)> {
        let workspace = self.workspace;
        let file = self.view.file;
        let source = self.source();
        let selection = crate::syntax::Lexer::new(source)
            .take_while(|token| token.span.start <= offset)
            .find(|token| {
                token.kind == TokenKind::Ident
                    && token.span.start <= offset
                    && offset <= token.span.end
            })
            .map(|token| token.span);
        // `package name`: the package clause name links to itself in gopls.
        if let Some(name) = package_clause_name(source)
            && selection.as_ref() == Some(&name)
        {
            return Ok(NavigationTarget {
                target: Target::Local(name.clone()),
                fallback: false,
                builtin: None,
                location: package_definition(self, file, source),
            });
        }
        let sites = import_sites(self, source);
        // Cursor inside an import path literal: gopls links the literal itself.
        if let Some(site) = sites
            .iter()
            .find(|site| site.path.start <= offset && offset <= site.path.end)
        {
            return Ok(NavigationTarget {
                target: Target::Local(site.path.clone()),
                fallback: false,
                builtin: None,
                location: None,
            });
        }
        // Cursor on an import alias declaration: links the alias name itself.
        if let Some(selection) = &selection
            && sites
                .iter()
                .any(|site| site.alias.as_ref() == Some(selection))
        {
            return Ok(NavigationTarget {
                target: Target::Local(selection.clone()),
                fallback: false,
                builtin: None,
                location: None,
            });
        }
        if let Some(selection) = &selection
            && let Some(target) = self.sema_navigation_target(selection)
        {
            return Ok(target);
        }
        // Blank range declarations are protocol self-anchors, like import
        // literals; the Go universe does not give the blank identifier an object.
        let target = if let Some(selection) = &selection
            && &source[selection.clone()] == "_"
            && let Some(function) = super::cursor_function(workspace, file, source, offset)
            && let Some(binding) = Locals::scope_at_cursor(source, offset, Some(function))?
                .locals
                .range_blank_at(selection)
        {
            Target::Local(binding.selection.clone())
        } else {
            Target::Unresolved
        };
        Ok(NavigationTarget {
            target,
            fallback: false,
            builtin: None,
            location: None,
        })
    }

    fn sema_navigation_target(&self, selection: &Range<usize>) -> Option<NavigationTarget<'a>> {
        let sel = [
            u32::try_from(selection.start).ok()?,
            u32::try_from(selection.end).ok()?,
        ];
        let file = self.view.file;
        let source = self.source();
        let obj = self.with_sema(|sema| sema.resolve_offset(file, source, sel))?;
        match obj {
            ObjectRef::PkgName { .. } => {
                let name = source.get(selection.clone())?;
                let sites = import_sites(self, source);
                let site = sites
                    .iter()
                    .find(|site| site.name.as_deref() == Some(name))?;
                Some(NavigationTarget {
                    target: Target::Local(import_qualifier_target(self, site, source)),
                    fallback: false,
                    builtin: None,
                    location: None,
                })
            }
            ObjectRef::Var {
                origin:
                    SymbolOrigin::Local {
                        file: f, span: d, ..
                    },
                ..
            }
            | ObjectRef::Const(SymbolOrigin::Local {
                file: f, span: d, ..
            })
            | ObjectRef::TypeName {
                origin:
                    SymbolOrigin::Local {
                        file: f, span: d, ..
                    },
                ..
            }
            | ObjectRef::Label { file: f, span: d } => (f == file).then_some(NavigationTarget {
                target: Target::Local(d[0] as usize..d[1] as usize),
                fallback: false,
                builtin: None,
                location: None,
            }),
            ObjectRef::TypeParam {
                owner: Some(decl),
                span: d,
                ..
            } => {
                let r = d[0] as usize..d[1] as usize;
                if decl.file == file {
                    Some(NavigationTarget {
                        target: Target::Local(r),
                        fallback: false,
                        builtin: None,
                        location: None,
                    })
                } else {
                    Some(NavigationTarget {
                        target: Target::Unresolved,
                        fallback: false,
                        builtin: None,
                        location: Some((decl.file.to_path_buf(), r)),
                    })
                }
            }
            ObjectRef::TypeParam {
                owner: None,
                span: d,
                ..
            } => Some(NavigationTarget {
                target: Target::Local(d[0] as usize..d[1] as usize),
                fallback: false,
                builtin: None,
                location: None,
            }),
            ObjectRef::Const(SymbolOrigin::Package(decl))
            | ObjectRef::TypeName {
                origin: SymbolOrigin::Package(decl),
                ..
            }
            | ObjectRef::Var {
                origin: SymbolOrigin::Package(decl),
                ..
            }
            | ObjectRef::Func(decl)
            | ObjectRef::Method(MethodOrigin::Concrete(decl)) => {
                let (path_buf, _) = self.workspace.files.get_key_value(decl.file)?;
                Some(NavigationTarget {
                    target: Target::Global(path_buf, decl.symbol),
                    fallback: false,
                    builtin: None,
                    location: None,
                })
            }
            ObjectRef::Field(member) | ObjectRef::Method(MethodOrigin::Interface(member)) => {
                if let Some(decl) = member.indexed(self.workspace) {
                    Some(NavigationTarget {
                        target: Target::Member(decl),
                        fallback: false,
                        builtin: None,
                        location: None,
                    })
                } else if member.selection[0] < member.selection[1] {
                    let r = member.selection[0] as usize..member.selection[1] as usize;
                    if member.file == file {
                        Some(NavigationTarget {
                            target: Target::Local(r),
                            fallback: false,
                            builtin: None,
                            location: None,
                        })
                    } else {
                        Some(NavigationTarget {
                            target: Target::Unresolved,
                            fallback: false,
                            builtin: None,
                            location: Some((member.file.to_path_buf(), r)),
                        })
                    }
                } else {
                    None
                }
            }
            ObjectRef::BuiltinFunction(bf) => Some(NavigationTarget {
                target: Target::Unresolved,
                fallback: false,
                builtin: Some(bf.name().to_owned()),
                location: None,
            }),
            ObjectRef::BuiltinType(name) | ObjectRef::BuiltinConst(name) => {
                Some(NavigationTarget {
                    target: Target::Unresolved,
                    fallback: false,
                    builtin: Some(name.to_owned()),
                    location: None,
                })
            }
            ObjectRef::Nil => Some(NavigationTarget {
                target: Target::Unresolved,
                fallback: false,
                builtin: Some("nil".to_owned()),
                location: None,
            }),
        }
    }

    pub(crate) fn reference_probe(&self, offset: usize) -> Option<NameProbe> {
        let source = self.source();
        let selection = Lexer::new(source)
            .take_while(|token| token.span.start <= offset)
            .find(|token| token.kind == TokenKind::Ident && offset <= token.span.end)?
            .span;
        Some(NameProbe::new(&source[selection]))
    }

    pub(crate) fn reference_target<'index>(
        &self,
        index: &'index NameProbe,
        offset: usize,
        include_declaration: bool,
    ) -> Result<ReferenceTarget<'index>, (i32, String)>
    where
        'a: 'index,
    {
        let source = self.source();
        let mut resolver = IdentityResolver::for_name(self.workspace, self.view.file, index);
        let analysis =
            Occurrences::for_name(source, &self.workspace.files[self.view.file], index.name())?;
        let Some(occurrence) = analysis
            .names
            .iter()
            .find(|name| name.span.start <= offset && offset <= name.span.end)
        else {
            return Ok(ReferenceTarget::Unresolved);
        };
        let target = match self.resolve_identity(&mut resolver, &analysis, source, occurrence) {
            Some(Identity::Global(file, symbol)) => ReferenceTarget::Global(file, symbol),
            Some(Identity::Member(member)) => ReferenceTarget::Member(member),
            Some(Identity::Local(selection)) => {
                let mut locations = Vec::new();
                for name in &analysis.names {
                    if (include_declaration || name.span != selection)
                        && matches!(self.resolve_identity(&mut resolver, &analysis, source, name), Some(Identity::Local(ref other)) if *other == selection)
                    {
                        if locations.len() == crate::analysis::budget::MAX_REFERENCES {
                            return Err((
                                crate::error::REQUEST_FAILED,
                                "reference result exceeds 4096 locations".into(),
                            ));
                        }
                        locations.push(name.span.clone());
                    }
                }
                ReferenceTarget::Local(locations)
            }
            None if matches!(
                occurrence.role,
                Role::Field
                    | Role::LiteralKey(_)
                    | Role::LiteralElementKey(_)
                    | Role::Member(Some(_))
                    | Role::ExpressionMember(_)
            ) =>
            {
                anonymous::occurrence_field(self, self.view.file, source, &analysis, occurrence)
                    .map_or(ReferenceTarget::Unresolved, ReferenceTarget::AnonymousField)
            }
            None => ReferenceTarget::Unresolved,
        };
        Ok(target)
    }

    pub(crate) fn cross_file_range(
        &self,
        path: &Path,
        span: &Range<usize>,
        current_map: &SourceMap<'_>,
    ) -> Option<super::SourceRange> {
        if path == self.view.file {
            Some(super::SourceRange::new(current_map, span.clone()))
        } else {
            let source = self.workspace.source(path)?;
            Some(super::SourceRange::new(
                &SourceMap::new(&source),
                span.clone(),
            ))
        }
    }

    pub(crate) fn member_selection(
        &self,
        member: MemberDeclId<'_>,
        current_map: &SourceMap<'_>,
    ) -> Option<super::SourceRange> {
        self.cross_file_range(
            member.file,
            &(member.selection[0] as usize..member.selection[1] as usize),
            current_map,
        )
    }

    /// gopls links a body-less Go function to its assembler `TEXT` declaration.
    pub(crate) fn asm_symbol_location(
        &self,
        go_file: &Path,
        symbol: &Symbol,
    ) -> Option<(PathBuf, super::SourceRange)> {
        if symbol.body().is_some() || !symbol.is_function() {
            return None;
        }
        let asm_file = go_file.with_extension("s");
        if asm_file == go_file {
            return None;
        }
        let source = self.workspace.source(&asm_file)?;
        let map = SourceMap::new(&source);
        let mut offset = 0;
        while let Some(found) = source[offset..].find("TEXT") {
            let start = offset + found;
            let after = source[start + 4..].trim_start();
            let token_start = source.len() - after.len();
            let token_end = token_start
                + after
                    .find(|character: char| {
                        character.is_whitespace() || character == '(' || character == ','
                    })
                    .unwrap_or(after.len());
            let token = &source[token_start..token_end];
            if let Some(separator) = token.rfind('·') {
                let name_start = token_start + separator + '·'.len_utf8();
                let name = &source[name_start..token_end];
                if name == symbol.name {
                    return Some((
                        asm_file,
                        super::SourceRange::new(&map, name_start..name_start + name.len()),
                    ));
                }
            } else if token == symbol.name {
                return Some((
                    asm_file,
                    super::SourceRange::new(&map, token_start..token_end),
                ));
            }
            offset = start + 4;
        }
        None
    }
}
