//! Navigation and reference seeds share the request's immutable source view.
use super::Context;
use crate::analysis::check::navigation::Target;
use crate::analysis::member::MemberDeclaration;
use crate::analysis::resolve::NameIndex;
use crate::analysis::scope::universe;
use crate::analysis::scope::{Identity, Locals, Resolver};
use crate::analysis::source::SourceAnalysis;
use crate::index::IndexedSymbol as Symbol;
use crate::syntax::{Lexer, TokenKind};
use crate::text::SourceMap;
use crate::workspace::Workspace;
use std::borrow::Cow;
use std::ops::Range;
use std::path::{Path, PathBuf};

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
        let candidates: Vec<_> = workspace
            .imported_symbol_candidates(file, alias_name, Some(probe))
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
                workspace.declaration_type_target(file, source, selection, &map)
            {
                target
            } else if let Some(target) =
                workspace.member_declaration_target(file, source, selection, &map)
            {
                Target::Member(target)
            } else {
                let function = super::cursor_function(workspace, file, source, offset);
                let scope = Locals::scope_at_cursor(source, offset, function)?;
                workspace.expression_target(file, source, selection, &scope)
            }
        } else {
            Target::Unresolved
        };
        let builtin_name = |selection: &Range<usize>| -> Option<String> {
            let name = &source[selection.clone()];
            (!preceded_by_dot(source, selection.start)
                && !universe::imported_name(workspace, file, name)
                && workspace.unqualified_symbol(file, name).is_none()
                && universe::builtin(name))
            .then(|| name.to_string())
        };
        // An orphaned file resolves imported members in the view selected by its
        // own build constraints. Do this before host-context type resolution so
        // variant-specific declarations win (for example unix syscall types).
        if matches!(target, Target::Opaque | Target::Unresolved)
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
        if matches!(target, Target::Opaque | Target::Unresolved)
            && let Some(selection) = &selection
        {
            if let Some(field) =
                crate::analysis::check::navigation::anonymous::field_target(source, selection)
            {
                target = Target::Local(field);
            } else if let Some((path, span)) =
                crate::analysis::check::navigation::anonymous::chain_location(
                    workspace, file, source, selection, offset,
                )
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
        if matches!(target, Target::Opaque | Target::Unresolved)
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
        if matches!(target, Target::Opaque | Target::Unresolved)
            && let Some(import_path) = import_path
        {
            target = Target::Local(import_path);
        }
        // A package-level declaration is the next identity source for unresolved
        // names; detached files still resolve their package's declarations.
        if matches!(target, Target::Unresolved)
            && let Some(selection) = &selection
            && !preceded_by_dot(source, selection.start)
            && let Some((path, symbol)) =
                workspace.unqualified_symbol(file, &source[selection.clone()])
        {
            target = Target::Global(path, symbol);
        }
        if matches!(target, Target::Unresolved)
            && let Some(builtin) = selection.as_ref().and_then(&builtin_name)
        {
            return Ok(NavigationTarget {
                target,
                fallback: false,
                builtin: Some(builtin),
                location: None,
            });
        }
        let mut builtin = None;
        if matches!(target, Target::Opaque) {
            let mut resolver = Resolver::new(workspace, file);
            let analysis = SourceAnalysis::new(source, &workspace.files[file])?;
            let resolved = analysis
                .names
                .iter()
                .find(|name| name.span.start <= offset && offset <= name.span.end)
                .and_then(|name| resolver.resolve(&analysis, source, name));
            target = match resolved {
                Some(Identity::Local(selection)) => Target::Local(selection),
                Some(Identity::Member(member)) => Target::Member(member),
                Some(Identity::Global(path, symbol)) => Target::Global(path, symbol),
                None => {
                    // Locals in scope were resolved above; a package-level
                    // declaration is the next identity source (detached files
                    // still resolve their package's production declarations).
                    let package_symbol = selection
                        .as_ref()
                        .filter(|selection| !preceded_by_dot(source, selection.start))
                        .and_then(|selection| {
                            workspace.unqualified_symbol(file, &source[selection.clone()])
                        });
                    if let Some((path, symbol)) = package_symbol {
                        Target::Global(path, symbol)
                    } else {
                        // Universe names with no local/package/import shadowing
                        // project to builtin.go.
                        builtin = selection.as_ref().and_then(builtin_name);
                        Target::Unresolved
                    }
                }
            };
            return Ok(NavigationTarget {
                target,
                fallback: true,
                builtin,
                location: None,
            });
        }
        Ok(NavigationTarget {
            target,
            fallback: false,
            builtin: None,
            location: None,
        })
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
        let mut resolver = Resolver::for_name(self.workspace, self.view.file, index);
        let analysis =
            SourceAnalysis::for_name(source, &self.workspace.files[self.view.file], index.name())?;
        let target = match analysis
            .names
            .iter()
            .find(|name| name.span.start <= offset && offset <= name.span.end)
            .and_then(|name| resolver.resolve(&analysis, source, name))
        {
            Some(Identity::Global(file, symbol)) => ReferenceTarget::Global(file, symbol),
            Some(Identity::Member(member)) => ReferenceTarget::Member(member),
            Some(Identity::Local(selection)) => {
                let mut locations = Vec::new();
                for name in &analysis.names {
                    if (include_declaration || name.span != selection)
                        && matches!(resolver.resolve(&analysis, source, name), Some(Identity::Local(ref other)) if *other == selection)
                    {
                        if locations.len() == super::budget::MAX_REFERENCES {
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
            None => ReferenceTarget::Unresolved,
        };
        drop(resolver);
        Ok(target)
    }
}
