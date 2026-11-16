//! Navigation and reference seeds share the request's immutable source view.
pub(in crate::analysis::query) mod anonymous;
mod decl_targets;
use super::Context;
use crate::analysis::obj::MemberDeclaration;
use crate::analysis::obj::{MethodOrigin, ObjectRef, SymbolOrigin};
use crate::analysis::resolve::NameIndex;
use crate::analysis::scope::Locals;
use crate::analysis::scope::universe;
use crate::analysis::sema::{Identity, IdentityResolver};
use crate::analysis::source::{Role, SourceAnalysis};
use crate::index::IndexedSymbol as Symbol;
use crate::syntax::{Lexer, TokenKind};
use crate::text::SourceMap;
use crate::workspace::Workspace;
pub(in crate::analysis::query) use decl_targets::{
    declaration_type_target, member_declaration_target,
};
use std::borrow::Cow;
use std::ops::Range;
use std::path::{Path, PathBuf};

pub(crate) enum Target<'a> {
    Unresolved,
    Local(Range<usize>),
    Global(&'a PathBuf, &'a Symbol),
    Member(MemberDeclaration<'a>),
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
    Member(MemberDeclaration<'a>),
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
fn import_qualifier_target(workspace: &Workspace, site: &ImportSite, source: &str) -> Range<usize> {
    match &site.alias {
        Some(alias) => {
            let declared = workspace.declared_package_name(&site.import_path);
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
fn import_sites(workspace: &Workspace, file: &Path, source: &str) -> Vec<ImportSite> {
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
        site.name = workspace.import_name(import).map(str::to_owned);
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

/// True when the identifier is a selector field (`x.field`).
fn preceded_by_dot(source: &str, start: usize) -> bool {
    let mut end = start;
    while let Some((offset, character)) = source[..end].char_indices().next_back() {
        if character.is_whitespace() {
            end = offset;
            continue;
        }
        return character == '.';
    }
    false
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
    workspace: &Workspace,
    file: &Path,
    source: &str,
) -> Option<(PathBuf, Range<usize>)> {
    let package = workspace.files.get(file)?.package.clone();
    let directory = file.parent()?;
    // Orphaned buffers see exactly their admitted orphan package view; real
    // packages exclude stubs that belong to an orphaned editor buffer.
    let detached = workspace.is_detached(file);
    // Orphaned test files form their own test package: the production siblings
    // admitted for a detached buffer belong to the non-test package, so gopls'
    // orphan view links the clause to itself.
    if detached
        && file
            .file_name()
            .is_some_and(|name| name.to_string_lossy().ends_with("_test.go"))
    {
        return None;
    }
    let mut candidates: Vec<&PathBuf> = workspace
        .files
        .iter()
        .filter(|(path, candidate)| {
            path.parent() == Some(directory)
                && candidate.package == package
                && (detached || !workspace.is_detached(path))
        })
        .map(|(path, _)| path)
        .collect();
    candidates.sort();
    // Bounded search: the package doc file is normally early in Go's compiled
    // file order, but large packages (runtime: ~590 files) need a wider window.
    candidates.truncate(256);
    for candidate in candidates {
        let text = if candidate == file {
            Cow::Borrowed(source)
        } else {
            match workspace.source(candidate) {
                Some(source) => Cow::Owned(source),
                None => continue,
            }
        };
        if !has_package_doc(&text) {
            continue;
        }
        if candidate == file {
            return None;
        }
        let name = package_clause_name(&text)?;
        return Some((candidate.clone(), name));
    }
    // Unindexed siblings (e.g. platform-only command directories) can still own
    // the package doc comment. Orphan packages only see their admitted view, and
    // a test file's package doc comes from test files, never unindexed
    // production files.
    if detached
        || file
            .file_name()
            .is_some_and(|name| name.to_string_lossy().ends_with("_test.go"))
    {
        return None;
    }
    let mut extra: Vec<PathBuf> = std::fs::read_dir(directory)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension().is_some_and(|extension| extension == "go")
                && !workspace.files.contains_key(path)
                && path != file
        })
        .collect();
    extra.sort();
    extra.truncate(32);
    for candidate in extra {
        let Some(text) = workspace.source(&candidate) else {
            continue;
        };
        if !has_package_doc(&text) {
            continue;
        }
        let name = package_clause_name(&text)?;
        if !text[name.clone()].eq(package.as_str()) {
            continue;
        }
        return Some((candidate, name));
    }
    None
}

/// `alias.Name` where `alias` is an imported package name: resolve the package
/// member through shared import metadata (constants, variables, types, funcs).
fn qualified_selector<'a>(
    workspace: &'a Workspace,
    file: &Path,
    source: &str,
    selection: &Range<usize>,
) -> Option<(&'a PathBuf, &'a Symbol)> {
    let mut tokens = Lexer::new(source).filter(|token| token.kind != TokenKind::Comment);
    let mut previous: Option<crate::syntax::Token> = None;
    let mut before: Option<crate::syntax::Token> = None;
    for token in tokens.by_ref() {
        if token.kind != TokenKind::Ident || token.span != *selection {
            before = previous;
            previous = Some(token);
            continue;
        }
        let (Some(period), Some(alias)) = (previous, before) else {
            return None;
        };
        if period.kind != TokenKind::Period || alias.kind != TokenKind::Ident {
            return None;
        }
        let alias_name = &source[alias.span];
        if !universe::imported_name(workspace, file, alias_name) {
            return None;
        }
        let field = &source[selection.clone()];
        let probe = crate::bloom::Fingerprint::new(field.as_bytes());
        let candidates: Vec<_> = crate::analysis::resolve::imported_symbol_candidates(
            workspace,
            file,
            alias_name,
            Some(probe),
        )
        .filter(|(_, symbol)| symbol.receiver.is_none() && symbol.name == field)
        .collect();
        // An orphaned file answers in a view selected by its own build
        // constraints, so variants of the imported package that the host build
        // excludes (for example windows syscall files) must not compete.
        let view = workspace.orphan_view_target(file);
        let mut matches = candidates.into_iter().filter(|(path, _)| {
            view.as_ref()
                .is_none_or(|target| workspace.file_matches_target(path, target))
        });
        let (path, symbol) = matches.next()?;
        return matches.next().is_none().then_some((path, symbol));
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
        let map = SourceMap::new(source);
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
                location: package_definition(workspace, file, source),
            });
        }
        let sites = import_sites(workspace, file, source);
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
        let mut target = if let Some(selection) = &selection {
            if let Some(symbol) = workspace.files[file]
                .symbols
                .iter()
                .find(|symbol| symbol.start == selection.start && symbol.end == selection.end)
            {
                Target::Global(
                    workspace
                        .files
                        .get_key_value(file)
                        .expect("indexed source")
                        .0,
                    symbol,
                )
            } else if let Some(target) =
                declaration_type_target(workspace, file, source, selection, &map)
            {
                target
            } else if let Some(target) =
                member_declaration_target(workspace, file, source, selection, &map)
            {
                Target::Member(target)
            } else if let Some(nav) = self.sema_navigation_target(offset, selection) {
                return Ok(nav);
            } else if &source[selection.clone()] == "_"
                && let Some(function) = super::cursor_function(workspace, file, source, offset)
                && let Some(binding) = Locals::scope_at_cursor(source, offset, Some(function))?
                    .locals
                    .range_blank_at(selection)
            {
                Target::Local(binding.selection.clone())
            } else {
                Target::Unresolved
            }
        } else {
            Target::Unresolved
        };
        let builtin_name = |selection: &Range<usize>| -> Option<String> {
            let name = &source[selection.clone()];
            (!preceded_by_dot(source, selection.start)
                && !universe::imported_name(workspace, file, name)
                && crate::analysis::resolve::unqualified_symbol(workspace, file, name).is_none()
                && universe::builtin(name))
            .then(|| name.to_string())
        };
        // An orphaned file resolves imported members in the view selected by its
        // own build constraints. Do this before host-context type resolution so
        // variant-specific declarations win (for example unix syscall types).
        if matches!(target, Target::Unresolved)
            && workspace.orphan_view_target(file).is_some()
            && let Some(selection) = &selection
            && !preceded_by_dot(source, selection.start)
            && let Some((path, symbol)) = qualified_selector(workspace, file, source, selection)
        {
            target = Target::Global(path, symbol);
        }
        // Inline `struct{...}` fields have no named owner symbol; the literal's
        // own type expression declares them. Selector chains ending at a named
        // struct field resolve through the chain's declaration annotations.
        if matches!(target, Target::Unresolved)
            && let Some(selection) = &selection
        {
            if let Some(field) = anonymous::field_target(source, selection) {
                target = Target::Local(field);
            } else if let Some((path, span)) =
                anonymous::chain_location(workspace, file, source, selection, offset)
            {
                if path == file {
                    target = Target::Local(span);
                } else {
                    return Ok(NavigationTarget {
                        target,
                        fallback: false,
                        builtin: None,
                        location: Some((path, span)),
                    });
                }
            }
        }
        // A package member selected through its import qualifier (`pkg.Name`).
        if matches!(target, Target::Unresolved)
            && let Some(selection) = &selection
            && let Some((path, symbol)) = qualified_selector(workspace, file, source, selection)
        {
            target = Target::Global(path, symbol);
        }
        // A qualifier (`pkg.Name`, `*pkg.Type`) links to the import alias name or,
        // for a default import, to the path literal, as gopls does.
        let import_path = selection
            .as_ref()
            .filter(|selection| !preceded_by_dot(source, selection.start))
            .and_then(|selection| {
                let name = &source[selection.clone()];
                universe::imported_name(workspace, file, name)
                    .then(|| sites.iter().find(|site| site.name.as_deref() == Some(name)))
                    .flatten()
                    .map(|site| import_qualifier_target(workspace, site, source))
            });
        if matches!(target, Target::Unresolved)
            && let Some(import_path) = import_path
        {
            target = Target::Local(import_path);
        }
        // Unresolved non-member occurrences inspect lexical/type-signature roles
        // before falling back to package-level or predeclared universe symbols.
        if matches!(target, Target::Unresolved)
            && let Some(selection) = &selection
            && !preceded_by_dot(source, selection.start)
        {
            let name = &source[selection.clone()];
            let analysis = SourceAnalysis::for_name(source, &workspace.files[file], name)?;
            let occ = analysis.names.iter().find(|n| n.span == *selection);
            match occ.map(|n| &n.role) {
                Some(Role::Parameter | Role::Result) => {
                    target = Target::Local(selection.clone());
                }
                Some(
                    Role::Field
                    | Role::Ignored
                    | Role::BlankDeclaration { .. }
                    | Role::Namespace
                    | Role::Member(_)
                    | Role::ExpressionMember(_)
                    | Role::Type(Some(_))
                    | Role::EmbeddedType(Some(_)),
                ) => {}
                Some(Role::LiteralKey(_) | Role::LiteralElementKey(_)) => {
                    if let Some(occ) = occ {
                        let mut resolver = IdentityResolver::new(workspace, file);
                        if let Some(resolved) = resolver.resolve(&analysis, source, occ) {
                            target = match resolved {
                                Identity::Local(sel) => Target::Local(sel),
                                Identity::Member(member) => Target::Member(member),
                                Identity::Global(path, symbol) => Target::Global(path, symbol),
                            };
                        }
                    }
                }
                Some(Role::FieldDeclaration(_)) => {
                    if let Some(occ) = occ {
                        let mut resolver = IdentityResolver::new(workspace, file);
                        if let Some(Identity::Member(member)) =
                            resolver.resolve(&analysis, source, occ)
                        {
                            target = Target::Member(member);
                        }
                    }
                }
                Some(Role::Type(None) | Role::EmbeddedType(None)) => {
                    if let Some(binding) = analysis.locals.definition_at(selection, name) {
                        if binding.object_kind.is_type() {
                            target = Target::Local(binding.selection.clone());
                        }
                    } else if let Some((path, symbol)) =
                        crate::analysis::resolve::unqualified_symbol(workspace, file, name)
                            .filter(|(_, symbol)| symbol.is_type())
                    {
                        target = Target::Global(path, symbol);
                    } else if let Some(builtin) = builtin_name(selection) {
                        return Ok(NavigationTarget {
                            target,
                            fallback: false,
                            builtin: Some(builtin),
                            location: None,
                        });
                    }
                }
                Some(Role::Name) | None => {
                    if let Some(binding) = analysis.locals.definition_at(selection, name) {
                        target = Target::Local(binding.selection.clone());
                    } else if let Some((path, symbol)) =
                        crate::analysis::resolve::unqualified_symbol(workspace, file, name)
                    {
                        target = Target::Global(path, symbol);
                    } else if let Some(builtin) = builtin_name(selection) {
                        return Ok(NavigationTarget {
                            target,
                            fallback: false,
                            builtin: Some(builtin),
                            location: None,
                        });
                    }
                }
            }
        }
        Ok(NavigationTarget {
            target,
            fallback: false,
            builtin: None,
            location: None,
        })
    }

    fn sema_navigation_target(
        &self,
        offset: usize,
        selection: &Range<usize>,
    ) -> Option<NavigationTarget<'a>> {
        let off = u32::try_from(offset).ok()?;
        let sel = [
            u32::try_from(selection.start).ok()?,
            u32::try_from(selection.end).ok()?,
        ];
        let file = self.view.file;
        let source = self.source();
        let (span, obj) = self.with_sema(|sema| sema.object_at(file, source, off))?;
        if span != sel {
            return None;
        }
        match obj {
            ObjectRef::PkgName { .. } => None,
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
                if let Some(decl) = member.to_member_declaration(self.workspace) {
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

    pub(crate) fn reference_index(&self, offset: usize) -> Option<NameIndex<'a>> {
        let source = self.source();
        let selection = Lexer::new(source)
            .take_while(|token| token.span.start <= offset)
            .find(|token| token.kind == TokenKind::Ident && offset <= token.span.end)?
            .span;
        Some(NameIndex::new(self.workspace, &source[selection]))
    }

    pub(crate) fn reference_target<'index>(
        &self,
        index: &'index NameIndex<'a>,
        offset: usize,
        include_declaration: bool,
    ) -> Result<ReferenceTarget<'index>, (i32, String)> {
        let source = self.source();
        let mut resolver = IdentityResolver::for_name(self.workspace, self.view.file, index);
        let analysis =
            SourceAnalysis::for_name(source, &self.workspace.files[self.view.file], index.name())?;
        let Some(occurrence) = analysis
            .names
            .iter()
            .find(|name| name.span.start <= offset && offset <= name.span.end)
        else {
            return Ok(ReferenceTarget::Unresolved);
        };
        let target = match resolver.resolve(&analysis, source, occurrence) {
            Some(Identity::Global(file, symbol)) => ReferenceTarget::Global(file, symbol),
            Some(Identity::Member(member)) => ReferenceTarget::Member(member),
            Some(Identity::Local(selection)) => {
                let mut locations = Vec::new();
                for name in &analysis.names {
                    if (include_declaration || name.span != selection)
                        && matches!(resolver.resolve(&analysis, source, name), Some(Identity::Local(ref other)) if *other == selection)
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
                let inline = anonymous::InlineIndex::new(source);
                anonymous::occurrence_field(
                    self.workspace,
                    self.view.file,
                    source,
                    &analysis,
                    occurrence,
                    &inline,
                )
                .map_or(ReferenceTarget::Unresolved, ReferenceTarget::AnonymousField)
            }
            None => ReferenceTarget::Unresolved,
        };
        drop(resolver);
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
        member: MemberDeclaration<'_>,
        current_map: &SourceMap<'_>,
    ) -> Option<super::SourceRange> {
        let data = if member.file == self.view.file {
            member.data_in_source(self.source(), current_map)
        } else {
            member.data(self.workspace)
        }?;
        Some(super::SourceRange::coordinates(
            (data.selection.start.line, data.selection.start.character),
            (data.selection.end.line, data.selection.end.character),
        ))
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
