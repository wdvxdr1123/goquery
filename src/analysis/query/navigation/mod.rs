//! Navigation and reference seeds share the request's immutable source view.
pub(in crate::analysis::query) mod anonymous;
mod decl_targets;
use super::hierarchy::SymbolTarget;
use super::locations::NavigationLocation;
use super::{Context, Location, SourceRange};
use crate::analysis::resolve::NameProbe;
use crate::analysis::sema::{
    MemberDeclId, MethodOrigin, RegionMode, SemanticObject, SemanticObject as ObjectRef,
    SymbolOrigin, identifier_at, lexical_name_spans,
};
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

/// One import spec, in source order, paired with its indexed import metadata.
struct ImportSite<'a> {
    alias: Option<Range<usize>>,
    path: Range<usize>,
    explicit_name: Option<&'a str>,
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

/// Import specs in source order. `path` is the quoted literal span; package
/// names are resolved lazily only if a `PkgName` lacks `import_path_span`.
fn import_sites<'a>(context: &Context<'a>, source: &str) -> Vec<ImportSite<'a>> {
    let workspace = context.workspace();
    let file = context.file();
    let mut sites = Vec::new();
    crate::syntax::visit_import_spans(source, |spec| {
        sites.push(ImportSite {
            alias: spec.alias,
            path: spec.path,
            explicit_name: None,
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
        site.explicit_name = import.explicit_name();
    }
    sites
}

fn find_import_site_by_name<'s>(
    context: &Context<'_>,
    sites: &'s [ImportSite<'_>],
    source: &str,
    name: &str,
) -> Option<&'s ImportSite<'s>> {
    let file = context.file();
    sites.iter().find(|site| {
        if let Some(alias) = &site.alias
            && matches!(&source[alias.clone()], "_" | ".")
        {
            return false;
        }
        let local = site.explicit_name.or_else(|| {
            (!site.import_path.is_empty())
                .then(|| {
                    context
                        .resolver()
                        .declared_package_name(file, site.import_path)
                })
                .flatten()
        });
        local == Some(name)
    })
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
    let view = context.build_view?;
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
        &mut self,
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
        &mut self,
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

    pub(super) fn navigation_target(&mut self, offset: usize) -> Result<Target<'a>, (i32, String)> {
        self.navigation_target_at(offset, false)
    }

    pub(super) fn navigation_target_for_identifier(
        &mut self,
        offset: usize,
        selection: &Range<usize>,
    ) -> Result<Target<'a>, (i32, String)> {
        self.navigation_target_from_selection(offset, false, Some(selection.clone()))
    }

    fn navigation_target_at(
        &mut self,
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
        &mut self,
        offset: usize,
        at_cursor: bool,
        selection: Option<Range<usize>>,
    ) -> Result<Target<'a>, (i32, String)> {
        let (early_target, sites, is_blank) = {
            let file = self.view.file;
            let source = self.source();
            // `package name`: the package clause name links to itself in gopls.
            if let Some(name) = package_clause_name(source)
                && selection.as_ref() == Some(&name)
            {
                let target = match package_definition(self, file, source) {
                    Some((path, span)) => Target::CrossFile(path, span),
                    None => Target::Local(name),
                };
                return Ok(target);
            }
            let sites = import_sites(self, source);
            // Cursor inside an import path literal: gopls links the literal itself.
            if let Some(site) = sites
                .iter()
                .find(|site| site.path.start <= offset && offset <= site.path.end)
            {
                (Some(Target::Local(site.path.clone())), Vec::new(), false)
            } else if let Some(selection) = &selection
                && sites
                    .iter()
                    .any(|site| site.alias.as_ref() == Some(selection))
            {
                // Cursor on an import alias declaration: links the alias name itself.
                (Some(Target::Local(selection.clone())), Vec::new(), false)
            } else {
                let is_blank = selection
                    .as_ref()
                    .is_some_and(|selection| &source[selection.clone()] == "_");
                (None, sites, is_blank)
            }
        };
        if let Some(target) = early_target {
            return Ok(target);
        }
        if let Some(selection) = &selection
            && let Some(target) = self.sema_navigation_target(selection, at_cursor, &sites)
        {
            return Ok(target);
        }
        // Blank range declarations are protocol self-anchors, like import
        // literals; the Go universe does not give the blank identifier an object.
        let target = if let Some(selection) = &selection
            && is_blank
            && let (Ok(s), Ok(e)) = (u32::try_from(selection.start), u32::try_from(selection.end))
        {
            let (doc, sema) = self.split();
            if sema
                .region_at(
                    doc.file(),
                    doc.source(),
                    offset as u32,
                    RegionMode::Prefix(offset),
                )
                .is_some_and(|view| view.is_range_blank([s, e]))
            {
                Target::Local(selection.clone())
            } else {
                Target::Unresolved
            }
        } else {
            Target::Unresolved
        };
        Ok(target)
    }

    fn sema_navigation_target(
        &mut self,
        selection: &Range<usize>,
        at_cursor: bool,
        sites: &[ImportSite<'_>],
    ) -> Option<Target<'a>> {
        let sel = [
            u32::try_from(selection.start).ok()?,
            u32::try_from(selection.end).ok()?,
        ];
        let obj = {
            let (doc, sema) = self.split();
            if at_cursor {
                sema.resolve_navigation_offset(doc.file(), doc.source(), sel)
            } else {
                sema.resolve_offset(doc.file(), doc.source(), sel)
            }
        }?;
        let file = self.view.file;
        let source = self.source();
        match obj {
            ObjectRef::PkgName {
                import_path_span, ..
            } => {
                if import_path_span[0] < import_path_span[1] {
                    let path = import_path_span[0] as usize..import_path_span[1] as usize;
                    if let Some(site) = sites.iter().find(|site| site.path == path) {
                        return Some(Target::Local(import_qualifier_target(self, site, source)));
                    }
                    return Some(Target::Local(path));
                }
                let name = source.get(selection.clone())?;
                let site = find_import_site_by_name(self, sites, source, name)?;
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
        &mut self,
        index: &'index NameProbe,
        offset: usize,
        include_declaration: bool,
    ) -> Result<ReferenceTarget<'index>, (i32, String)>
    where
        'a: 'index,
    {
        let (doc, sema) = self.split();
        let source = doc.source();
        let file = doc.file();
        let workspace = doc.workspace();
        let Some(stub) = workspace.files.get(file) else {
            return Ok(ReferenceTarget::Unresolved);
        };
        let Some(cursor_span) = identifier_at(source, offset) else {
            return Ok(ReferenceTarget::Unresolved);
        };
        if &source[cursor_span.clone()] != index.name() || index.name() == "_" {
            return Ok(ReferenceTarget::Unresolved);
        }
        let cursor_key = [
            u32::try_from(cursor_span.start).unwrap_or(u32::MAX),
            u32::try_from(cursor_span.end).unwrap_or(u32::MAX),
        ];
        let Some(cursor_obj) = sema.resolve_offset(file, source, cursor_key) else {
            if sema.implicit_declaration_at(file, source, cursor_key) {
                let mut locations = Vec::new();
                for span in lexical_name_spans(source, stub, index.name()) {
                    let key = [span.start as u32, span.end as u32];
                    let matches_guard = (include_declaration && key == cursor_key)
                        || matches!(
                            sema.resolve_offset(file, source, key),
                            Some(SemanticObject::Var {
                                origin: SymbolOrigin::Local { span: d, .. },
                                ..
                            }) if d == cursor_key
                        );
                    if matches_guard {
                        if locations.len() == crate::analysis::budget::MAX_REFERENCES {
                            return Err((
                                crate::error::REQUEST_FAILED,
                                "reference result exceeds 4096 locations".into(),
                            ));
                        }
                        locations.push(span);
                    }
                }
                return Ok(ReferenceTarget::Local(locations));
            }
            return Ok(ReferenceTarget::Unresolved);
        };
        let target = match cursor_obj {
            SemanticObject::Const(SymbolOrigin::Package(decl))
            | SemanticObject::TypeName {
                origin: SymbolOrigin::Package(decl),
                ..
            }
            | SemanticObject::Var {
                origin: SymbolOrigin::Package(decl),
                ..
            }
            | SemanticObject::Func(decl)
            | SemanticObject::Method(MethodOrigin::Concrete(decl)) => {
                match workspace.files.get_key_value(decl.file) {
                    Some((path, _)) => ReferenceTarget::Global(path, decl.symbol),
                    None => ReferenceTarget::Unresolved,
                }
            }
            SemanticObject::Field(member)
            | SemanticObject::Method(MethodOrigin::Interface(member)) => {
                if let Some(indexed) = member.indexed(workspace) {
                    ReferenceTarget::Member(indexed)
                } else if matches!(cursor_obj, SemanticObject::Field(_))
                    && !member.builtin
                    && member.selection[0] < member.selection[1]
                {
                    ReferenceTarget::AnonymousField(anonymous::InlineField {
                        file: member.file.to_path_buf(),
                        selection: member.selection[0] as usize..member.selection[1] as usize,
                        name: index.name().to_owned(),
                    })
                } else {
                    ReferenceTarget::Unresolved
                }
            }
            SemanticObject::Const(SymbolOrigin::Local {
                span: decl_span, ..
            })
            | SemanticObject::TypeName {
                origin:
                    SymbolOrigin::Local {
                        span: decl_span, ..
                    },
                ..
            }
            | SemanticObject::Var {
                origin:
                    SymbolOrigin::Local {
                        span: decl_span, ..
                    },
                ..
            }
            | SemanticObject::TypeParam {
                span: decl_span, ..
            }
            | SemanticObject::Label {
                span: decl_span, ..
            } => {
                let mut locations = Vec::new();
                let is_case_var = cursor_obj.case_scope().is_some();
                for span in lexical_name_spans(source, stub, index.name()) {
                    let key = [span.start as u32, span.end as u32];
                    let is_decl = key == decl_span
                        && (sema.resolve_offset(file, source, key) == Some(cursor_obj)
                            || (is_case_var && sema.implicit_declaration_at(file, source, key)));
                    if !include_declaration && is_decl {
                        continue;
                    }
                    let matched = is_decl
                        || sema
                            .resolve_offset(file, source, key)
                            .is_some_and(|other| cursor_obj.same_reference_target(other, false));
                    if matched {
                        if locations.len() == crate::analysis::budget::MAX_REFERENCES {
                            return Err((
                                crate::error::REQUEST_FAILED,
                                "reference result exceeds 4096 locations".into(),
                            ));
                        }
                        locations.push(span);
                    }
                }
                ReferenceTarget::Local(locations)
            }
            SemanticObject::PkgName { .. }
            | SemanticObject::BuiltinFunction(_)
            | SemanticObject::BuiltinType(_)
            | SemanticObject::BuiltinConst(_)
            | SemanticObject::Nil => ReferenceTarget::Unresolved,
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
