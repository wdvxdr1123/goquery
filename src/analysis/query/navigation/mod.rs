//! Navigation and reference seeds share the request's immutable source view.
pub(in crate::analysis::query) mod anonymous;
mod decl_targets;
use super::hierarchy::SymbolTarget;
use super::locations::NavigationLocation;
use super::{Context, Location, SourceRange};
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

pub(super) enum Target<'a> {
    Unresolved,
    Local(Range<usize>),
    Global(&'a PathBuf, &'a Symbol),
    Member(MemberDeclId<'a>),
    Builtin(&'static str),
    CrossFile(&'a Path, Range<usize>),
}

pub(crate) enum ReferenceTarget<'a> {
    Global(&'a PathBuf, &'a Symbol),
    Member(MemberDeclId<'a>),
    Local(Vec<Range<usize>>),
    AnonymousField(anonymous::InlineField),
    Unresolved,
}

/// One import spec, in source order, paired with its package metadata name.
struct ImportSite<'a> {
    alias: Option<Range<usize>>,
    path: Range<usize>,
    name: Option<&'a str>,
    import_path: &'a str,
}

/// gopls links a qualifier to the import path literal when the local name equals
/// the imported package's declared name; otherwise to the explicit alias name.
/// The equal-name form mirrors gopls' `PkgName` range heuristic: range start at
/// the spec's name position (the alias when written) and length `len(path)+2`.
fn import_qualifier_target(
    context: &Context<'_>,
    site: &ImportSite<'_>,
    source: &str,
) -> Range<usize> {
    match &site.alias {
        Some(alias) => {
            let declared = context
                .resolver()
                .declared_package_name(context.file(), site.import_path);
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
fn import_sites<'a>(context: &Context<'a>, source: &str) -> Vec<ImportSite<'a>> {
    let workspace = context.workspace();
    let file = context.file();
    let mut sites = Vec::new();
    crate::syntax::visit_import_spans(source, |spec| {
        sites.push(ImportSite {
            alias: spec.alias,
            path: spec.path,
            name: None,
            import_path: "",
        });
    });
    let imports = workspace
        .files
        .get(file)
        .map_or(&[][..], |stub| stub.imports.as_slice());
    for (site, import) in sites.iter_mut().zip(imports) {
        site.import_path = &import.path;
        if let Some(alias) = &site.alias
            && matches!(&source[alias.clone()], "_" | ".")
        {
            continue;
        }
        site.name = import
            .explicit_name()
            .or_else(|| context.resolver().declared_package_name(file, &import.path));
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
fn package_definition<'a>(
    context: &Context<'a>,
    file: &Path,
    source: &str,
) -> Option<(&'a Path, Range<usize>)> {
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
                .then(|| Some((candidate.as_path(), package_clause_name(&text)?)))
                .flatten();
        }
    }
    None
}

impl<'a> Context<'a> {
    pub(crate) fn definition_location(
        &self,
        offset: usize,
        method: &str,
    ) -> Result<Option<NavigationLocation<'a>>, (i32, String)> {
        let target = self.navigation_target_at(offset, true)?;
        self.trace_candidates(method, 1);
        let mut current_map = None;
        let location = match target {
            Target::Unresolved => None,
            Target::Local(span) => self
                .cross_file_range(self.file(), &span, &mut current_map)
                .map(|range| Location {
                    file: Cow::Borrowed(self.file()),
                    range,
                }),
            Target::CrossFile(file, span) => self
                .cross_file_range(file, &span, &mut current_map)
                .map(|range| Location {
                    file: Cow::Borrowed(file),
                    range,
                }),
            Target::Member(member) => {
                self.member_selection(member, &mut current_map)
                    .map(|range| Location {
                        file: Cow::Borrowed(member.file),
                        range,
                    })
            }
            Target::Builtin(name) => {
                self.builtin_symbol_location(name, false)
                    .map(|(file, range)| Location {
                        file: Cow::Owned(file),
                        range,
                    })
            }
            Target::Global(file, symbol) => {
                // Body-less Go functions may link to an assembler TEXT declaration.
                Some(self.asm_symbol_location(file, symbol).map_or_else(
                    || Location {
                        file: Cow::Borrowed(file.as_path()),
                        range: SourceRange::indexed_selection(&symbol.selection),
                    },
                    |(file, range)| Location {
                        file: Cow::Owned(file),
                        range,
                    },
                ))
            }
        };
        Ok(location)
    }

    pub(crate) fn hierarchy_symbol(
        &self,
        offset: usize,
        method: &str,
    ) -> Result<Option<SymbolTarget<'a>>, (i32, String)> {
        let target = self.navigation_target(offset)?;
        self.trace_candidates(method, 1);
        Ok(match target {
            Target::Global(file, symbol) => Some(SymbolTarget { file, symbol }),
            _ => None,
        })
    }

    pub(super) fn navigation_target(&self, offset: usize) -> Result<Target<'a>, (i32, String)> {
        self.navigation_target_at(offset, false)
    }

    pub(super) fn navigation_target_for_identifier(
        &self,
        offset: usize,
        selection: &Range<usize>,
    ) -> Result<Target<'a>, (i32, String)> {
        self.navigation_target_from_selection(offset, false, Some(selection.clone()))
    }

    fn navigation_target_at(
        &self,
        offset: usize,
        at_cursor: bool,
    ) -> Result<Target<'a>, (i32, String)> {
        let source = self.source();
        let selection = crate::syntax::Lexer::new(source)
            .take_while(|token| token.span.start <= offset)
            .find(|token| {
                token.kind == TokenKind::Ident
                    && token.span.start <= offset
                    && offset <= token.span.end
            })
            .map(|token| token.span);
        self.navigation_target_from_selection(offset, at_cursor, selection)
    }

    fn navigation_target_from_selection(
        &self,
        offset: usize,
        at_cursor: bool,
        selection: Option<Range<usize>>,
    ) -> Result<Target<'a>, (i32, String)> {
        let workspace = self.workspace;
        let file = self.view.file;
        let source = self.source();
        // `package name`: the package clause name links to itself in gopls.
        if let Some(name) = package_clause_name(source)
            && selection.as_ref() == Some(&name)
        {
            return Ok(match package_definition(self, file, source) {
                Some((path, span)) => Target::CrossFile(path, span),
                None => Target::Local(name),
            });
        }
        let sites = import_sites(self, source);
        // Cursor inside an import path literal: gopls links the literal itself.
        if let Some(site) = sites
            .iter()
            .find(|site| site.path.start <= offset && offset <= site.path.end)
        {
            return Ok(Target::Local(site.path.clone()));
        }
        // Cursor on an import alias declaration: links the alias name itself.
        if let Some(selection) = &selection
            && sites
                .iter()
                .any(|site| site.alias.as_ref() == Some(selection))
        {
            return Ok(Target::Local(selection.clone()));
        }
        if let Some(selection) = &selection
            && let Some(target) = self.sema_navigation_target(selection, at_cursor, &sites)
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
        Ok(target)
    }

    fn sema_navigation_target(
        &self,
        selection: &Range<usize>,
        at_cursor: bool,
        sites: &[ImportSite<'_>],
    ) -> Option<Target<'a>> {
        let sel = [
            u32::try_from(selection.start).ok()?,
            u32::try_from(selection.end).ok()?,
        ];
        let file = self.view.file;
        let source = self.source();
        let obj = self.with_sema(|sema| {
            if at_cursor {
                sema.resolve_navigation_offset(file, source, sel)
            } else {
                sema.resolve_offset(file, source, sel)
            }
        })?;
        match obj {
            ObjectRef::PkgName { .. } => {
                let name = source.get(selection.clone())?;
                let site = sites.iter().find(|site| site.name == Some(name))?;
                Some(Target::Local(import_qualifier_target(self, site, source)))
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
            | ObjectRef::Label { file: f, span: d } => {
                (f == file).then_some(Target::Local(d[0] as usize..d[1] as usize))
            }
            ObjectRef::TypeParam {
                owner: Some(decl),
                span: d,
                ..
            } => {
                let r = d[0] as usize..d[1] as usize;
                Some(if decl.file == file {
                    Target::Local(r)
                } else {
                    Target::CrossFile(decl.file, r)
                })
            }
            ObjectRef::TypeParam {
                owner: None,
                span: d,
                ..
            } => Some(Target::Local(d[0] as usize..d[1] as usize)),
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
                Some(Target::Global(path_buf, decl.symbol))
            }
            ObjectRef::Field(member) | ObjectRef::Method(MethodOrigin::Interface(member)) => {
                if let Some(decl) = member.indexed(self.workspace) {
                    Some(Target::Member(decl))
                } else if member.selection[0] < member.selection[1] {
                    let r = member.selection[0] as usize..member.selection[1] as usize;
                    Some(if member.file == file {
                        Target::Local(r)
                    } else {
                        Target::CrossFile(member.file, r)
                    })
                } else {
                    None
                }
            }
            ObjectRef::BuiltinFunction(bf) => Some(Target::Builtin(bf.name())),
            ObjectRef::BuiltinType(name) | ObjectRef::BuiltinConst(name) => {
                Some(Target::Builtin(name))
            }
            ObjectRef::Nil => Some(Target::Builtin("nil")),
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

    pub(super) fn cross_file_range<'source>(
        &'source self,
        path: &Path,
        span: &Range<usize>,
        current_map: &mut Option<SourceMap<'source>>,
    ) -> Option<super::SourceRange> {
        if path == self.view.file {
            let map = current_map.get_or_insert_with(|| SourceMap::new(self.source()));
            Some(SourceRange::new(map, span.clone()))
        } else {
            let source = self.workspace.source(path)?;
            Some(super::SourceRange::new(
                &SourceMap::new(&source),
                span.clone(),
            ))
        }
    }

    pub(super) fn member_selection<'source>(
        &'source self,
        member: MemberDeclId<'_>,
        current_map: &mut Option<SourceMap<'source>>,
    ) -> Option<super::SourceRange> {
        self.cross_file_range(
            member.file,
            &(member.selection[0] as usize..member.selection[1] as usize),
            current_map,
        )
    }

    /// gopls links a body-less Go function to its assembler `TEXT` declaration.
    fn asm_symbol_location(
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
            let name_start = source[token_start..token_end]
                .rfind('·')
                .map_or(token_start, |separator| {
                    token_start + separator + '·'.len_utf8()
                });
            if source[name_start..token_end] == symbol.name {
                return Some((
                    asm_file,
                    SourceRange::new(&SourceMap::new(&source), name_start..token_end),
                ));
            }
            offset = start + 4;
        }
        None
    }
}
