//! Rename identity, bounded reference rows and edit operands; no retained state.
//!
//! The semantic rules follow gopls: the selected object decides the edit set
//! (local binding, package declaration, field/method, receiver group or import
//! name), and declaration doc comments/doc links are renamed with it.
use super::locations::SourcePosition;
use super::{Context, ReferenceTarget, SourceRange};
use crate::analysis::member::MemberDeclaration;
use crate::analysis::resolve::{self, NameIndex};
use crate::analysis::scope::{self, Identity, Resolver};
use crate::analysis::source::{Role, SourceAnalysis};
use crate::bloom::Fingerprint;
use crate::index::IndexedSymbol as Symbol;
use crate::syntax::{Lexer, MemberKind, TokenKind};
use crate::text::SourceMap;
use crate::workspace::Workspace;
use std::collections::HashSet;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// A successful prepare-rename result: the selected identifier range and name.
pub(crate) struct PrepareItem {
    pub(crate) range: SourceRange,
    pub(crate) placeholder: String,
}

/// Replacement text semantics for one edit row.
pub(crate) enum RenameText {
    /// Replace with the requested new name.
    Name,
    /// Import spec prefix; an explicit alias is only written when needed.
    ImportPrefix { imported: String },
}

/// One workspace edit operand, already in protocol coordinates.
pub(crate) struct RenameEdit {
    pub(crate) file: PathBuf,
    pub(crate) range: SourceRange,
    pub(crate) text: RenameText,
}

pub(crate) struct RenameEdits {
    pub(crate) edits: Vec<RenameEdit>,
}

/// The semantic object a rename request applies to. Targets own their key so no
/// query-local index borrow escapes target discovery.
enum Target {
    Global {
        file: PathBuf,
        symbol: Symbol,
    },
    Member(MemberKey),
    Local {
        selection: Range<usize>,
        spans: Vec<Range<usize>>,
    },
    Import {
        name: String,
    },
    Label {
        spans: Vec<Range<usize>>,
    },
    /// A field owned by an inline (`struct{...}`) type: no member symbol exists,
    /// so the declaration offset itself is the identity.
    AnonymousField {
        file: PathBuf,
        selection: Range<usize>,
        name: String,
    },
}

/// Stable member key: declaring file, owner declaration start and member name.
struct MemberKey {
    file: PathBuf,
    owner_start: usize,
    name: String,
    kind: MemberKind,
    embedded: bool,
}

impl MemberKey {
    fn of(member: &MemberDeclaration<'_>) -> Self {
        Self {
            file: member.file.clone(),
            owner_start: member.owner.start,
            name: member.name.to_owned(),
            kind: member.kind,
            embedded: member.embedded,
        }
    }

    /// Rebuilds a workspace-borrowed member declaration for reference queries.
    fn resolve<'q>(&'q self, workspace: &'q Workspace) -> Option<MemberDeclaration<'q>> {
        let (file, stub) = workspace.files.get_key_value(&self.file)?;
        let owner = stub
            .symbols
            .iter()
            .find(|symbol| symbol.start == self.owner_start)?;
        Some(MemberDeclaration {
            file,
            owner,
            name: &self.name,
            kind: self.kind,
            embedded: self.embedded,
        })
    }
}

/// Package-name declarations require a file/directory rename, which this server
/// does not implement; gopls reports the same message when the client lacks
/// file-renaming support.
const PACKAGE_RENAME_ERROR: &str =
    "can't rename package: LSP client does not support file renaming";

impl<'a> Context<'a> {
    pub(crate) fn prepare_rename(
        &self,
        offset: usize,
    ) -> Result<Option<PrepareItem>, (i32, String)> {
        let source = self.source();
        if let Some(span) = package_name_span(source)
            && contains_span(&span, offset)
        {
            return Err((0, PACKAGE_RENAME_ERROR.into()));
        }
        if func_keyword_at(source, offset) {
            return Ok(None);
        }
        let Some(ident) = cursor_identifier(source, offset) else {
            return Ok(self
                .import_spec_at(source, offset)
                .and_then(|import| self.prepare_import(&import)));
        };
        if let Some(import) = self.import_object_at(source, &ident) {
            return Ok(self.prepare_import(&import));
        }
        let Some(target) = self.prepare_target(source, &ident) else {
            return Ok(None);
        };
        if !prepare_renamable(self.workspace(), source, self.file(), &ident, &target) {
            return Ok(None);
        }
        let map = SourceMap::new(source);
        Ok(Some(PrepareItem {
            range: SourceRange::new(&map, ident.clone()),
            placeholder: target_name(source, &target),
        }))
    }

    pub(crate) fn rename_edits(
        &self,
        offset: usize,
        new_name: &str,
    ) -> Result<RenameEdits, (i32, String)> {
        let source = self.source();
        let file = self.file();
        if let Some(span) = package_name_span(source)
            && contains_span(&span, offset)
        {
            return Err((0, PACKAGE_RENAME_ERROR.into()));
        }
        if func_keyword_at(source, offset) {
            return Err((
                0,
                if valid_identifier(new_name) {
                    "parsed signature is *ast.Ident, not a function type".into()
                } else {
                    format!("invalid identifier to rename: {new_name:?}")
                },
            ));
        }
        if !valid_identifier(new_name) {
            return Err((0, format!("invalid identifier to rename: {new_name:?}")));
        }
        let Some(ident) = cursor_identifier(source, offset) else {
            let import = self
                .import_spec_at(source, offset)
                .ok_or_else(|| (0, "no identifier found".to_string()))?;
            return self.finish_import(file, &import, new_name);
        };
        if let Some(import) = self.import_object_at(source, &ident) {
            return self.finish_import(file, &import, new_name);
        }
        let target = self
            .rename_target(source, &ident)?
            .ok_or_else(|| (0, "no identifier found".to_string()))?;
        let old_name = target_name(source, &target);
        if old_name == new_name {
            return Err((0, format!("old and new names are the same: {new_name}")));
        }
        if let Some(error) = check_renamable(self.workspace(), source, file, &ident, &target) {
            return Err((0, error));
        }
        if let Some(error) = self.method_package_error(&target, &old_name, new_name) {
            return Err((0, error));
        }
        let mut edits = self.target_edits(file, &target, &old_name, new_name)?;
        self.export_check(&target, &edits, &old_name, new_name)?;
        edits.extend(self.declaration_doc_edits(source, file, &target, &old_name));
        edits.extend(self.link_edits(&target, &old_name)?);
        Ok(RenameEdits {
            edits: finalize(edits),
        })
    }

    fn finish_import(
        &self,
        file: &Path,
        import: &ImportAt,
        new_name: &str,
    ) -> Result<RenameEdits, (i32, String)> {
        if import.name == new_name {
            return Err((0, format!("old and new names are the same: {new_name}")));
        }
        if import.name == "_" {
            return Err((0, "can't rename \"_\"".into()));
        }
        if new_name == "init" {
            return Err((0, "\"init\" is not a valid imported package name".into()));
        }
        let mut edits = Vec::new();
        self.import_edits(file, import, &mut edits)?;
        Ok(RenameEdits {
            edits: finalize(edits),
        })
    }
}

// ---------------------------------------------------------------------------
// Target discovery
// ---------------------------------------------------------------------------

impl<'a> Context<'a> {
    /// Prepare reports `null` for every unresolvable position; only the
    /// package-name declaration surfaces a user-facing error.
    fn prepare_target(&self, source: &str, ident: &Range<usize>) -> Option<Target> {
        self.rename_target(source, ident).ok().flatten()
    }

    fn rename_target(
        &self,
        source: &str,
        ident: &Range<usize>,
    ) -> Result<Option<Target>, (i32, String)> {
        let file = self.file();
        if let Some(import) = self.import_object_at(source, ident) {
            return Ok(Some(Target::Import { name: import.name }));
        }
        let Some(index) = self.reference_index(ident.start) else {
            return self.unresolved_target(source, file, ident);
        };
        let target = self.reference_target(&index, ident.start, true)?;
        Ok(match target {
            ReferenceTarget::Global(path, symbol) => Some(Target::Global {
                file: path.clone(),
                symbol: symbol.clone(),
            }),
            ReferenceTarget::Member(member) => Some(Target::Member(MemberKey::of(&member))),
            ReferenceTarget::Local(spans) => {
                let selection = self
                    .local_declaration(source, ident, &spans)
                    .unwrap_or_else(|| ident.clone());
                Some(Target::Local { selection, spans })
            }
            ReferenceTarget::Unresolved => {
                if let Some(field) =
                    crate::analysis::check::navigation::anonymous::field_declaration(
                        self.workspace(),
                        file,
                        source,
                        ident,
                        ident.start,
                    )
                {
                    return Ok(Some(Target::AnonymousField {
                        file: field.file,
                        selection: field.selection,
                        name: field.name,
                    }));
                }
                if let Some(label) = self.label_target(source, ident)? {
                    return Ok(Some(label));
                }
                self.unresolved_target(source, file, ident)?
            }
        })
    }

    /// Locates the declaring occurrence of a local binding among its references.
    fn local_declaration(
        &self,
        source: &str,
        ident: &Range<usize>,
        spans: &[Range<usize>],
    ) -> Option<Range<usize>> {
        let file = self.file();
        let stub = self.workspace().files.get(file)?;
        let analysis = SourceAnalysis::new(source, stub).ok()?;
        let mut resolver = Resolver::new(self.workspace(), file);
        for name in &analysis.names {
            if !spans.contains(&name.span) {
                continue;
            }
            if matches!(resolver.resolve(&analysis, source, name), Some(Identity::Local(selection)) if selection == name.span)
            {
                return Some(name.span.clone());
            }
        }
        spans.iter().find(|span| **span == *ident).cloned()
    }

    /// Blank declarations and predeclared names never resolve to a symbol.
    fn unresolved_target(
        &self,
        source: &str,
        file: &Path,
        ident: &Range<usize>,
    ) -> Result<Option<Target>, (i32, String)> {
        let spelling = &source[ident.clone()];
        let stub = &self.workspace().files[file];
        let analysis = SourceAnalysis::new(source, stub)?;
        let role = analysis
            .names
            .iter()
            .find(|name| name.span == *ident)
            .map(|name| &name.role);
        if matches!(role, Some(Role::BlankDeclaration { .. })) {
            return Err((0, "can't rename \"_\"".into()));
        }
        if spelling == "_" {
            if blank_declaration(source, ident) {
                return Err((0, "can't rename \"_\"".into()));
            }
            return Err((0, "no object found for \"_\"".into()));
        }
        if scope::universe::builtin(spelling)
            && scope::universe::visible(
                self.workspace(),
                file,
                Some(&analysis.locals),
                ident.start,
                spelling,
            )
        {
            return Err((0, format!("{spelling} is built in and cannot be renamed")));
        }
        Err((0, format!("no object found for {spelling:?}")))
    }

    fn label_target(
        &self,
        source: &str,
        ident: &Range<usize>,
    ) -> Result<Option<Target>, (i32, String)> {
        let file = self.file();
        let Some((_, function)) =
            super::cursor_function(self.workspace(), file, source, ident.start)
        else {
            return Ok(None);
        };
        let tokens: Vec<(TokenKind, Range<usize>)> = Lexer::new(&source[function.clone()])
            .filter(|token| token.kind != TokenKind::Comment)
            .map(|token| {
                (
                    token.kind,
                    function.start + token.span.start..function.start + token.span.end,
                )
            })
            .collect();
        let Some(position) = tokens.iter().position(|(_, span)| *span == *ident) else {
            return Ok(None);
        };
        if tokens.get(position + 1).map(|(kind, _)| *kind) != Some(TokenKind::Colon) {
            return Ok(None);
        }
        let old = &source[ident.clone()];
        let mut spans = Vec::new();
        for (index, (kind, span)) in tokens.iter().enumerate() {
            if *kind != TokenKind::Ident || &source[span.clone()] != old {
                continue;
            }
            let before = index.checked_sub(1).map(|index| tokens[index].0);
            let after = tokens.get(index + 1).map(|(kind, _)| *kind);
            let declaration = after == Some(TokenKind::Colon)
                && matches!(
                    before,
                    None | Some(TokenKind::LBrace)
                        | Some(TokenKind::RBrace)
                        | Some(TokenKind::Semicolon)
                        | Some(TokenKind::Colon)
                );
            let reference = matches!(
                before,
                Some(TokenKind::Break) | Some(TokenKind::Continue) | Some(TokenKind::Goto)
            );
            if declaration || reference {
                spans.push(span.clone());
            }
        }
        if !spans.contains(ident) {
            return Ok(None);
        }
        Ok(Some(Target::Label { spans }))
    }

    // -- imports ----------------------------------------------------------

    fn import_specs(&self, source: &str) -> Vec<ImportSpec> {
        let mut specs = Vec::new();
        let mut tokens = Lexer::new(source)
            .filter(|token| token.kind != TokenKind::Comment)
            .peekable();
        while let Some(token) = tokens.next() {
            if token.kind != TokenKind::Import {
                continue;
            }
            let grouped = tokens
                .next()
                .is_some_and(|next| next.kind == TokenKind::LParen);
            if !grouped {
                if let Some(spec) = parse_import_spec(&mut tokens) {
                    specs.push(spec);
                }
                continue;
            }
            loop {
                match tokens.peek().map(|token| token.kind) {
                    Some(TokenKind::RParen) => {
                        tokens.next();
                        break;
                    }
                    Some(TokenKind::Semicolon) => {
                        tokens.next();
                    }
                    Some(_) => {
                        if let Some(spec) = parse_import_spec(&mut tokens) {
                            specs.push(spec);
                        } else {
                            break;
                        }
                    }
                    None => break,
                }
            }
        }
        specs
    }

    fn import_spec_at(&self, source: &str, offset: usize) -> Option<ImportAt> {
        let specs = self.import_specs(source);
        let index = specs
            .iter()
            .position(|spec| contains_span(&spec.path, offset))?;
        let stub = self.workspace().files.get(self.file())?;
        let import = stub.imports.get(index)?;
        Some(ImportAt {
            index,
            name: self.import_effective_name(import),
            spec: Some(specs[index].clone()),
            cursor: None,
        })
    }

    /// Detects import objects selected at an identifier: explicit alias,
    /// qualifier use, or the package name of an import path.
    fn import_object_at(&self, source: &str, ident: &Range<usize>) -> Option<ImportAt> {
        let specs = self.import_specs(source);
        let stub = self.workspace().files.get(self.file())?;
        if let Some((index, spec)) = specs
            .iter()
            .enumerate()
            .find(|(_, spec)| spec.alias.as_ref() == Some(ident))
        {
            let import = stub.imports.get(index)?;
            return Some(ImportAt {
                index,
                name: self.import_effective_name(import),
                spec: Some(spec.clone()),
                cursor: Some(ident.clone()),
            });
        }
        let spelling = &source[ident.clone()];
        let index = stub
            .imports
            .iter()
            .position(|import| self.import_effective_name(import) == spelling)?;
        if !qualifier_use(source, ident) {
            return None;
        }
        if self
            .workspace()
            .unqualified_symbol(self.file(), spelling)
            .is_some()
        {
            return None;
        }
        if let Ok(scope) = self.cursor_scope(ident.start)
            && scope.locals.resolve_at(ident.start, spelling).is_some()
        {
            return None;
        }
        Some(ImportAt {
            index,
            name: spelling.to_owned(),
            spec: specs.get(index).cloned(),
            cursor: Some(ident.clone()),
        })
    }

    fn import_effective_name(&self, import: &crate::syntax::Import) -> String {
        self.workspace()
            .import_name(import)
            .map(str::to_owned)
            .unwrap_or_else(|| import_suffix(&import.path).to_owned())
    }

    fn prepare_import(&self, import: &ImportAt) -> Option<PrepareItem> {
        let source = self.source();
        let spec = import.spec.as_ref()?;
        if import.name == "_" {
            return None;
        }
        let map = SourceMap::new(source);
        let range = match (&spec.alias, &import.cursor) {
            // A selected identifier (alias declaration or qualifier use) always
            // reports its own range, as gopls does for the ImportSpec object.
            (_, Some(cursor)) => SourceRange::new(&map, cursor.clone()),
            (Some(alias), None) => SourceRange::new(&map, alias.clone()),
            (None, None) => {
                let start = map.position(spec.path.start);
                SourceRange::coordinates(
                    (start.line, start.character),
                    (start.line, start.character),
                )
            }
        };
        Some(PrepareItem {
            range,
            placeholder: import.name.clone(),
        })
    }

    fn import_edits(
        &self,
        file: &Path,
        import: &ImportAt,
        rows: &mut Vec<RenameEdit>,
    ) -> Result<(), (i32, String)> {
        let source = self.source();
        let map = SourceMap::new(source);
        let stub = self.workspace().files.get(file).ok_or((
            crate::error::INVALID_PARAMS,
            "missing package metadata".to_string(),
        ))?;
        let declared = stub
            .imports
            .get(import.index)
            .map(|import| {
                self.workspace()
                    .declared_package_name(&import.path)
                    .map_or_else(|| import_suffix(&import.path).to_owned(), str::to_owned)
            })
            .ok_or((crate::error::INVALID_PARAMS, "missing import".to_string()))?;
        if let Some(spec) = &import.spec {
            rows.push(RenameEdit {
                file: file.to_path_buf(),
                range: SourceRange::new(&map, spec.start..spec.path.start),
                text: RenameText::ImportPrefix { imported: declared },
            });
        }
        // Qualifier uses of this import in the current file, respecting lexical
        // and package-level shadowing.
        let old_name = &import.name;
        let mut tokens = Lexer::new(source).filter(|token| token.kind != TokenKind::Comment);
        let mut previous = None;
        let mut next = tokens.next();
        while let Some(token) = next {
            let following = tokens.next();
            if token.kind == TokenKind::Ident
                && &source[token.span.clone()] == old_name
                && following.as_ref().map(|next| next.kind) == Some(TokenKind::Period)
                && !matches!(
                    previous.map(|previous: crate::syntax::Token| previous.kind),
                    Some(TokenKind::Period) | Some(TokenKind::Ident)
                )
                && self
                    .workspace()
                    .unqualified_symbol(file, old_name)
                    .is_none()
            {
                let shadowed = self
                    .cursor_scope(token.span.start)
                    .ok()
                    .is_some_and(|scope| {
                        scope
                            .locals
                            .resolve_at(token.span.start, old_name)
                            .is_some()
                    });
                if !shadowed {
                    rows.push(RenameEdit {
                        file: file.to_path_buf(),
                        range: SourceRange::new(&map, token.span.clone()),
                        text: RenameText::Name,
                    });
                }
            }
            previous = Some(token);
            next = following;
        }
        Ok(())
    }
}

#[derive(Clone)]
struct ImportSpec {
    /// Start of the spec: explicit alias if present, otherwise the path quote.
    start: usize,
    alias: Option<Range<usize>>,
    path: Range<usize>,
}

#[derive(Clone)]
struct ImportAt {
    index: usize,
    name: String,
    spec: Option<ImportSpec>,
    /// Import identifier selected by the cursor (alias or qualifier use).
    cursor: Option<Range<usize>>,
}

fn parse_import_spec(
    tokens: &mut impl Iterator<Item = crate::syntax::Token>,
) -> Option<ImportSpec> {
    let token = tokens.next()?;
    match token.kind {
        TokenKind::Ident => {
            let alias = token.span;
            let path = tokens
                .next()
                .filter(|next| next.kind == TokenKind::String)?;
            Some(ImportSpec {
                start: alias.start,
                alias: Some(alias),
                path: path.span,
            })
        }
        TokenKind::Period => {
            let path = tokens
                .next()
                .filter(|next| next.kind == TokenKind::String)?;
            Some(ImportSpec {
                start: token.span.start,
                alias: Some(token.span),
                path: path.span,
            })
        }
        TokenKind::String => Some(ImportSpec {
            start: token.span.start,
            alias: None,
            path: token.span,
        }),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Rename checks
// ---------------------------------------------------------------------------

fn prepare_renamable(
    workspace: &Workspace,
    source: &str,
    file: &Path,
    cursor: &Range<usize>,
    target: &Target,
) -> bool {
    match target {
        Target::Global { symbol, .. } => symbol.name != "_",
        Target::Member(member) => {
            if member.name == "_" {
                return false;
            }
            !member.embedded || at_member_declaration(workspace, source, file, cursor, member)
        }
        Target::Local { selection, .. } => &source[selection.clone()] != "_",
        Target::Import { name, .. } => name != "_",
        Target::Label { .. } => true,
        Target::AnonymousField { name, .. } => name != "_",
    }
}

fn check_renamable(
    workspace: &Workspace,
    source: &str,
    file: &Path,
    cursor: &Range<usize>,
    target: &Target,
) -> Option<String> {
    match target {
        Target::Local { selection, .. } if &source[selection.clone()] == "_" => {
            Some("can't rename \"_\"".into())
        }
        Target::Global { symbol, .. } if symbol.name == "_" => Some("can't rename \"_\"".into()),
        Target::Member(member) => {
            if member.name == "_" {
                return Some("can't rename \"_\"".into());
            }
            if member.embedded && !at_member_declaration(workspace, source, file, cursor, member) {
                return Some(
                    "an embedded field must be renamed at its declaration (since it renames the type too)"
                        .into(),
                );
            }
            None
        }
        _ => None,
    }
}

/// Whether the cursor is exactly on this member's declaration identifier.
fn at_member_declaration(
    workspace: &Workspace,
    source: &str,
    file: &Path,
    cursor: &Range<usize>,
    member: &MemberKey,
) -> bool {
    if member.file != file {
        return false;
    }
    let Some(member) = member.resolve(workspace) else {
        return false;
    };
    member_declaration_span(&member, source).is_some_and(|span| span == *cursor)
}

// ---------------------------------------------------------------------------
// Edit collection
// ---------------------------------------------------------------------------

impl<'a> Context<'a> {
    fn target_edits(
        &self,
        file: &Path,
        target: &Target,
        old_name: &str,
        new_name: &str,
    ) -> Result<Vec<RenameEdit>, (i32, String)> {
        let mut rows = Vec::new();
        match target {
            Target::Global {
                file: target_file,
                symbol,
            } => {
                let index = NameIndex::new(self.workspace(), &symbol.name);
                let started = Instant::now();
                let locations = self.global_references(
                    file,
                    &index,
                    target_file.as_path(),
                    symbol,
                    true,
                    started,
                )?;
                for location in locations {
                    rows.push(RenameEdit {
                        file: location.file.to_path_buf(),
                        range: location.range,
                        text: RenameText::Name,
                    });
                }
                rows.push(RenameEdit {
                    file: target_file.clone(),
                    range: lsp_range(symbol.selection),
                    text: RenameText::Name,
                });
                self.embedded_type_consequences(target_file.as_path(), symbol, &mut rows)?;
            }
            Target::Member(key) => {
                let Some(member) = key.resolve(self.workspace()) else {
                    return Ok(rows);
                };
                let index = NameIndex::new(self.workspace(), &key.name);
                let locations = self.member_references(member, &index, true, file)?;
                for location in locations {
                    rows.push(RenameEdit {
                        file: location.file.to_path_buf(),
                        range: location.range,
                        text: RenameText::Name,
                    });
                }
                if let Some(range) = member_declaration_range(self.workspace(), &member) {
                    rows.push(RenameEdit {
                        file: member.file.clone(),
                        range,
                        text: RenameText::Name,
                    });
                }
                if member.kind == MemberKind::Field {
                    self.field_reference_supplement(&member, file, &mut rows)?;
                }
            }
            Target::Local { selection, spans } => {
                let source = self.source();
                let map = SourceMap::new(source);
                for span in spans {
                    rows.push(RenameEdit {
                        file: file.to_path_buf(),
                        range: SourceRange::new(&map, span.clone()),
                        text: RenameText::Name,
                    });
                }
                if !spans.contains(selection) {
                    rows.push(RenameEdit {
                        file: file.to_path_buf(),
                        range: SourceRange::new(&map, selection.clone()),
                        text: RenameText::Name,
                    });
                }
                if let Some(owner) = self.receiver_declaration(source, selection)? {
                    self.receiver_group_edits(file, &owner, old_name, new_name, &mut rows)?;
                }
            }
            Target::Import { .. } => {}
            Target::AnonymousField {
                file: decl_file,
                selection,
                ..
            } => {
                // The declaration is the identity. Selector/keyed occurrences of
                // inline-struct fields are not yet collected; prepareRename and
                // definition use this target without edit expansion.
                let Some(source) = self.workspace().source(decl_file) else {
                    return Ok(rows);
                };
                let map = SourceMap::new(&source);
                rows.push(RenameEdit {
                    file: decl_file.clone(),
                    range: SourceRange::new(&map, selection.clone()),
                    text: RenameText::Name,
                });
            }
            Target::Label { spans } => {
                let source = self.source();
                let map = SourceMap::new(source);
                for span in spans {
                    rows.push(RenameEdit {
                        file: file.to_path_buf(),
                        range: SourceRange::new(&map, span.clone()),
                        text: RenameText::Name,
                    });
                }
            }
        }
        Ok(rows)
    }

    /// Supplementary field-reference rows for candidates where the shared
    /// Resolver cannot project a binding's native type. Identity is proven by
    /// the receiver/key type declaration, never by spelling alone.
    fn field_reference_supplement(
        &self,
        target: &MemberDeclaration<'_>,
        origin: &Path,
        rows: &mut Vec<RenameEdit>,
    ) -> Result<(), (i32, String)> {
        let workspace = self.workspace();
        let Some(target_stub) = workspace.files.get(target.file) else {
            return Ok(());
        };
        let mut candidates = if resolve::exported(target.name) {
            workspace.dependent_candidates(target.file)
        } else {
            let package = target_stub.package.as_str();
            workspace
                .files
                .iter()
                .filter(|(path, stub)| {
                    path.parent() == target.file.parent() && stub.package == package
                })
                .collect()
        };
        let scope = workspace.reference_scope(origin);
        candidates.retain(|(file, _)| scope.contains(file, target.file));
        let fingerprint = Fingerprint::new(target.name.as_bytes());
        for (file, stub) in candidates {
            if !stub.identifiers.may_contain_fingerprint(fingerprint) {
                continue;
            }
            let Some(source) = workspace.source(file) else {
                continue;
            };
            if !source.contains(target.name) {
                continue;
            }
            let Ok(analysis) = SourceAnalysis::for_name(&source, stub, target.name) else {
                continue;
            };
            let map = SourceMap::new(&source);
            for name in &analysis.names {
                if &source[name.span.clone()] != target.name
                    || matches!(name.role, Role::FieldDeclaration(_))
                {
                    continue;
                }
                let matched = match &name.role {
                    Role::Member(Some(qualifier)) => {
                        let text = &source[qualifier.clone()];
                        let binding = analysis.locals.resolve_at(qualifier.start, text);
                        let ty = binding.and_then(|binding| {
                            workspace
                                .binding_type(file, &source, &analysis.locals, binding)
                                .or_else(|| {
                                    call_initializer_type(workspace, file, &source, binding)
                                })
                        });
                        let in_scope = ty.as_ref().is_some_and(|ty| {
                            workspace.receiver_in_scope(file, ty, &analysis.locals, qualifier.start)
                        });
                        let same = ty
                            .as_ref()
                            .is_some_and(|ty| same_field_owner(workspace, file, ty, target));
                        in_scope && same
                    }
                    Role::LiteralKey(ty) => {
                        same_field_owner(workspace, file, &source[ty.clone()], target)
                    }
                    Role::LiteralElementKey(context) => analysis
                        .literal_element_type(workspace, file, &source, *context)
                        .is_some_and(|(ty, _)| same_field_owner(workspace, file, &ty, target)),
                    _ => false,
                };
                if !matched {
                    continue;
                }
                if rows.len() == super::budget::MAX_REFERENCES {
                    return Err((
                        crate::error::REQUEST_FAILED,
                        "reference result exceeds 4096 locations".into(),
                    ));
                }
                rows.push(RenameEdit {
                    file: file.clone(),
                    range: SourceRange::new(&map, name.span.clone()),
                    text: RenameText::Name,
                });
            }
        }
        Ok(())
    }

    /// Recognizes a receiver declaration at the selection and returns the
    /// receiver type base name.
    fn receiver_declaration(
        &self,
        source: &str,
        selection: &Range<usize>,
    ) -> Result<Option<String>, (i32, String)> {
        let file = self.file();
        let Some(stub) = self.workspace().files.get(file) else {
            return Ok(None);
        };
        for symbol in &stub.symbols {
            if !symbol.is_method() || symbol.receiver.is_none() {
                continue;
            }
            let Some(declaration) = symbol.declaration() else {
                continue;
            };
            if !(declaration.start <= selection.start && selection.start < symbol.start) {
                continue;
            }
            let header = &source[declaration.start..symbol.start];
            let Some(token) = Lexer::new(header).find(|token| token.kind == TokenKind::Ident)
            else {
                continue;
            };
            let span = declaration.start + token.span.start..declaration.start + token.span.end;
            if span == *selection {
                return Ok(Some(
                    receiver_base(symbol.receiver.as_deref().unwrap_or("")).to_owned(),
                ));
            }
        }
        Ok(None)
    }

    /// gopls renames receivers of every other method of the same type when the
    /// rename is initiated at a receiver declaration.
    fn receiver_group_edits(
        &self,
        file: &Path,
        owner: &str,
        old_name: &str,
        new_name: &str,
        rows: &mut Vec<RenameEdit>,
    ) -> Result<(), (i32, String)> {
        let workspace = self.workspace();
        let Some(stub) = workspace.files.get(file) else {
            return Ok(());
        };
        let directory = file.parent().map(Path::to_path_buf);
        let package = stub.package.clone();
        let index = NameIndex::new(workspace, old_name);
        let methods: Vec<(&PathBuf, &Symbol)> = workspace
            .files
            .iter()
            .filter(|(path, candidate)| {
                path.parent().map(Path::to_path_buf) == directory && candidate.package == package
            })
            .flat_map(|(path, candidate)| {
                candidate
                    .symbols
                    .iter()
                    .filter(move |symbol| symbol.is_method())
                    .map(move |symbol| (path, symbol))
            })
            .collect();
        for (method_file, symbol) in methods {
            if symbol.receiver.as_deref() != Some(owner) {
                continue;
            }
            let Some(source) = workspace.source(method_file) else {
                continue;
            };
            let Some(span) = receiver_span(&source, symbol) else {
                continue;
            };
            let receiver_name = &source[span.clone()];
            if receiver_name != old_name || receiver_name == new_name {
                continue;
            }
            let Some(candidate_stub) = workspace.files.get(method_file) else {
                continue;
            };
            let analysis = SourceAnalysis::for_name(&source, candidate_stub, old_name)?;
            if analysis.names.is_empty() {
                continue;
            }
            let map = SourceMap::new(&source);
            let mut resolver = Resolver::for_name(workspace, method_file, &index);
            for name in &analysis.names {
                if &source[name.span.clone()] != old_name {
                    continue;
                }
                if matches!(resolver.resolve(&analysis, &source, name), Some(Identity::Local(binding)) if binding == span)
                {
                    rows.push(RenameEdit {
                        file: method_file.clone(),
                        range: SourceRange::new(&map, name.span.clone()),
                        text: RenameText::Name,
                    });
                }
            }
        }
        Ok(())
    }

    /// Renaming an exported type whose declaration is embedded in another
    /// package must also rename the embedding field and its promoted selectors.
    fn embedded_type_consequences(
        &self,
        target_file: &Path,
        symbol: &Symbol,
        rows: &mut Vec<RenameEdit>,
    ) -> Result<(), (i32, String)> {
        if !symbol.is_type() {
            return Ok(());
        }
        let workspace = self.workspace();
        let mut candidates = workspace.reference_candidates(target_file, symbol);
        let scope = workspace.reference_scope(self.file());
        candidates.retain(|(candidate, _)| scope.contains(candidate, target_file));
        let target_import = workspace.import_path(target_file);
        for (file, stub) in candidates {
            let qualifier = stub
                .imports
                .iter()
                .find(|import| Some(import.path.as_str()) == target_import.as_deref())
                .and_then(|import| workspace.import_name(import))
                .unwrap_or_default()
                .to_owned();
            let owners: Vec<&str> = stub
                .symbols
                .iter()
                .filter(|owner| {
                    owner.type_info.as_ref().is_some_and(|info| {
                        info.embedded.iter().any(|member| {
                            let name = member.rsplit('.').next().unwrap_or(member);
                            name == symbol.name
                                && (member == &symbol.name
                                    || member.starts_with(&format!("{qualifier}.")))
                        })
                    })
                })
                .map(|owner| owner.name.as_str())
                .collect();
            if owners.is_empty() {
                continue;
            }
            let Some(source) = workspace.source(file) else {
                continue;
            };
            if !source.contains(&symbol.name) {
                continue;
            }
            let map = SourceMap::new(&source);
            for owner_name in owners {
                let Some(owner) = stub
                    .symbols
                    .iter()
                    .find(|candidate| candidate.is_type() && candidate.name == owner_name)
                else {
                    continue;
                };
                let member = MemberDeclaration {
                    file,
                    owner,
                    name: &symbol.name,
                    kind: MemberKind::Field,
                    embedded: true,
                };
                let Some(span) = member_declaration_span(&member, &source) else {
                    continue;
                };
                rows.push(RenameEdit {
                    file: file.clone(),
                    range: SourceRange::new(&map, span),
                    text: RenameText::Name,
                });
                let index = NameIndex::new(workspace, &symbol.name);
                let locations = self.member_references(member, &index, true, file)?;
                for location in locations {
                    rows.push(RenameEdit {
                        file: location.file.to_path_buf(),
                        range: location.range,
                        text: RenameText::Name,
                    });
                }
            }
        }
        Ok(())
    }
}

/// Receiver identifier span of one method declaration.
fn receiver_span(source: &str, symbol: &Symbol) -> Option<Range<usize>> {
    let declaration = symbol.declaration()?;
    let header = source.get(declaration.start..symbol.start)?;
    let token = Lexer::new(header).find(|token| token.kind == TokenKind::Ident)?;
    Some(declaration.start + token.span.start..declaration.start + token.span.end)
}

// ---------------------------------------------------------------------------
// Export checks
// ---------------------------------------------------------------------------

impl<'a> Context<'a> {
    fn export_check(
        &self,
        target: &Target,
        edits: &[RenameEdit],
        old_name: &str,
        new_name: &str,
    ) -> Result<(), (i32, String)> {
        if !resolve::exported(old_name) || resolve::exported(new_name) {
            return Ok(());
        }
        let (decl_file, decl_range) = match target {
            Target::Global { file, symbol } => (file.as_path(), lsp_range(symbol.selection)),
            Target::Member(key) => {
                let Some(member) = key.resolve(self.workspace()) else {
                    return Ok(());
                };
                let Some(range) = member_declaration_range(self.workspace(), &member) else {
                    return Ok(());
                };
                (member.file.as_path(), range)
            }
            _ => return Ok(()),
        };
        let workspace = self.workspace();
        let Some(decl_stub) = workspace.files.get(decl_file) else {
            return Ok(());
        };
        let decl_package = (decl_file.parent(), decl_stub.package.as_str());
        for edit in edits {
            let Some(stub) = workspace.files.get(&edit.file) else {
                continue;
            };
            if (edit.file.parent(), stub.package.as_str()) == decl_package {
                continue;
            }
            let decl_position = position_text(workspace, decl_file, decl_range.start)?;
            let reference_position = position_text(workspace, &edit.file, edit.range.start)?;
            let package = workspace
                .import_path(&edit.file)
                .unwrap_or_else(|| stub.package.clone());
            return Err((
                0,
                format!(
                    "{decl_position}: renaming {old_name:?} to {new_name:?} would make it unexported\n{reference_position}:\tbreaking references from packages such as {package:?}"
                ),
            ));
        }
        Ok(())
    }

    /// gopls refuses method renames when any processed package has parse or type
    /// errors, because its assignability (satisfy) pass needs well-typed input.
    /// An offline module dependency that could not be loaded is the observable
    /// equivalent here: the declaring package cannot type-check, so report the
    /// same shape of error instead of producing a partial rename.
    fn method_package_error(
        &self,
        target: &Target,
        old_name: &str,
        new_name: &str,
    ) -> Option<String> {
        let target_file = match target {
            Target::Global { file, symbol } if symbol.is_method() && symbol.receiver.is_some() => {
                file.as_path()
            }
            Target::Member(key) if key.kind == MemberKind::Method => key.file.as_path(),
            _ => return None,
        };
        let workspace = self.workspace();
        let (path, stub) = workspace.files.get_key_value(target_file)?;
        let directory = path.parent()?;
        if stub.package.is_empty() {
            return None;
        }
        let package = stub.package.clone();
        let mut files: Vec<&PathBuf> = workspace
            .files
            .iter()
            .filter(|(candidate, stub)| {
                candidate.parent() == Some(directory) && stub.package == package
            })
            .map(|(candidate, _)| candidate)
            .collect();
        files.sort();
        let error_file = files.into_iter().find(|candidate| {
            let Some(stub) = workspace.files.get(*candidate) else {
                return false;
            };
            stub.imports
                .iter()
                .any(|import| workspace.declared_package_name(&import.path).is_none())
        })?;
        let package = workspace.import_path(target_file)?;
        let name = error_file.file_name()?.to_string_lossy().into_owned();
        Some(format!(
            "renaming {old_name:?} to {new_name:?} not possible because {name:?} in {package:?} has errors"
        ))
    }
}

// ---------------------------------------------------------------------------
// Doc comments and doc links
// ---------------------------------------------------------------------------

impl<'a> Context<'a> {
    fn declaration_doc_edits(
        &self,
        source: &str,
        file: &Path,
        target: &Target,
        old_name: &str,
    ) -> Vec<RenameEdit> {
        let workspace = self.workspace();
        let mut declarations: Vec<(PathBuf, Range<usize>)> = Vec::new();
        match target {
            Target::Global {
                file: target_file,
                symbol,
            } => {
                declarations.push((target_file.clone(), symbol.start..symbol.end));
            }
            Target::Member(key) => {
                if key.file == file
                    && let Some(member) = key.resolve(workspace)
                    && let Some(span) = member_declaration_span(&member, source)
                {
                    declarations.push((key.file.clone(), span));
                }
            }
            Target::Local { selection, .. } => {
                declarations.push((file.to_path_buf(), selection.clone()));
            }
            _ => {}
        }
        let mut edits = Vec::new();
        for (decl_file, span) in declarations {
            let text = if decl_file == file {
                source.to_owned()
            } else {
                match workspace.source(&decl_file) {
                    Some(text) => text,
                    None => continue,
                }
            };
            let Some(block) = doc_comment_block(&text, span.start) else {
                continue;
            };
            if !text[block.clone()].contains(old_name) {
                continue;
            }
            let map = SourceMap::new(&text);
            word_edits(&text, &map, block, old_name, &decl_file, &mut edits);
        }
        edits
    }

    /// gopls also rewrites doc links such as `[pkg.Type]`/`[Recv.Method]` in the
    /// package-level doc comments of every processed package.
    fn link_edits(
        &self,
        target: &Target,
        old_name: &str,
    ) -> Result<Vec<RenameEdit>, (i32, String)> {
        let workspace = self.workspace();
        let (target_file, name, mode, receiver) = match target {
            Target::Global { file, symbol } => {
                if !resolve::exported(old_name) || !resolve::exported(&symbol.name) {
                    return Ok(Vec::new());
                }
                let mode = if symbol.is_type() {
                    LinkMode::Type
                } else if symbol.is_function() || symbol.is_value() {
                    LinkMode::Object
                } else {
                    return Ok(Vec::new());
                };
                (file.clone(), symbol.name.clone(), mode, None)
            }
            Target::Member(key) => {
                let Some(member) = key.resolve(workspace) else {
                    return Ok(Vec::new());
                };
                if key.kind != MemberKind::Method
                    || !resolve::exported(old_name)
                    || !resolve::exported(&key.name)
                    || !resolve::exported(&member.owner.name)
                {
                    return Ok(Vec::new());
                }
                (
                    key.file.clone(),
                    key.name.clone(),
                    LinkMode::Method,
                    Some(member.owner.name.clone()),
                )
            }
            _ => return Ok(Vec::new()),
        };
        let target_file = target_file.as_path();
        let Some(decl_stub) = workspace.files.get(target_file) else {
            return Ok(Vec::new());
        };
        let declaring_package = decl_stub.package.clone();
        let candidates: Vec<&PathBuf> = if mode == LinkMode::Type || receiver.is_some() {
            workspace
                .dependent_candidates(target_file)
                .into_iter()
                .map(|(path, _)| path)
                .collect()
        } else {
            workspace
                .reference_candidates(
                    target_file,
                    match target {
                        Target::Global { symbol, .. } => symbol,
                        _ => return Ok(Vec::new()),
                    },
                )
                .into_iter()
                .map(|(path, _)| path)
                .collect()
        };
        let import_path = workspace.import_path(target_file);
        let mut edits = Vec::new();
        let mut seen = HashSet::new();
        for candidate in candidates {
            if !seen.insert(candidate.as_path()) {
                continue;
            }
            let Some(stub) = workspace.files.get(candidate) else {
                continue;
            };
            let same_package =
                candidate.parent() == target_file.parent() && stub.package == declaring_package;
            let expected_prefix = if same_package {
                None
            } else {
                let alias = import_path.as_deref().and_then(|import| {
                    stub.imports
                        .iter()
                        .find(|candidate| candidate.path == import)
                        .and_then(|import| workspace.import_name(import))
                });
                Some(
                    alias
                        .map_or_else(|| declaring_package.as_str(), |alias| alias)
                        .to_owned(),
                )
            };
            let Some(source) = workspace.source(candidate) else {
                continue;
            };
            if !source.contains('[') || !source.contains(name.as_str()) {
                continue;
            }
            let map = SourceMap::new(&source);
            for comment in top_level_doc_comments(&source) {
                if !source[comment.clone()].contains(name.as_str()) {
                    continue;
                }
                link_edits_in_comment(
                    &source,
                    &map,
                    comment,
                    &name,
                    receiver.as_deref(),
                    mode,
                    expected_prefix.as_deref(),
                    candidate,
                    &mut edits,
                );
            }
        }
        Ok(edits)
    }
}

/// Parses the import path suffix, used when no declared package name is indexed.
fn import_suffix(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Top-level documentation comments: comments followed (ignoring further
/// comments) by a declaration keyword at brace depth zero. Includes the package
/// documentation comment and excludes comments inside function bodies.
fn top_level_doc_comments(source: &str) -> Vec<Range<usize>> {
    let mut comments = Vec::new();
    let mut depth = 0usize;
    let mut pending: Vec<Range<usize>> = Vec::new();
    for token in Lexer::with_comments(source) {
        match token.kind {
            TokenKind::Comment => {
                if depth == 0 {
                    pending.push(token.span);
                }
                continue;
            }
            TokenKind::LBrace => depth += 1,
            TokenKind::RBrace => depth = depth.saturating_sub(1),
            _ => {}
        }
        if depth != 0 {
            pending.clear();
            continue;
        }
        match token.kind {
            TokenKind::Func
            | TokenKind::Type
            | TokenKind::Var
            | TokenKind::Const
            | TokenKind::Import
            | TokenKind::Package => comments.append(&mut pending),
            _ => pending.clear(),
        }
    }
    comments
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LinkMode {
    Type,
    Object,
    Method,
}

#[allow(clippy::too_many_arguments)]
fn link_edits_in_comment(
    source: &str,
    map: &SourceMap<'_>,
    comment: Range<usize>,
    name: &str,
    receiver: Option<&str>,
    mode: LinkMode,
    expected_prefix: Option<&str>,
    file: &Path,
    edits: &mut Vec<RenameEdit>,
) {
    let text = &source[comment.clone()];
    let mut offset = 0;
    while let Some(open) = text[offset..].find('[') {
        let open = offset + open;
        let Some(close) = text[open + 1..].find(']').map(|close| open + 1 + close) else {
            break;
        };
        if close - open > 256 {
            offset = close + 1;
            continue;
        }
        let content = &text[open + 1..close];
        let follow = text.as_bytes().get(close + 1).copied();
        if !matches!(follow, Some(b':'))
            && let Some(segment) = link_segment(content, name, receiver, mode, expected_prefix)
        {
            let start = comment.start + open + 1 + segment;
            edits.push(RenameEdit {
                file: file.to_path_buf(),
                range: SourceRange::new(map, start..start + name.len()),
                text: RenameText::Name,
            });
        }
        offset = close + 1;
    }
}

/// Returns the byte offset of the object name segment inside a doc link, or
/// `None` when the link does not denote the target object.
fn link_segment(
    content: &str,
    name: &str,
    receiver: Option<&str>,
    mode: LinkMode,
    expected_prefix: Option<&str>,
) -> Option<usize> {
    let (rest, mut offset) = match content.strip_prefix('*') {
        Some(rest) => (rest, 1usize),
        None => (content, 0usize),
    };
    if rest.is_empty()
        || rest.contains(['\n', '\r', ' ', '\t'])
        || rest.starts_with('.')
        || rest.ends_with('.')
    {
        return None;
    }
    let segments: Vec<&str> = rest.split('.').collect();
    if segments.iter().any(|segment| segment.is_empty()) {
        return None;
    }
    let (index, matched) = match (mode, expected_prefix) {
        (LinkMode::Type, None) => (0, segments[0] == name),
        (LinkMode::Type, Some(prefix)) => {
            (1, segments[0] == prefix && segments.get(1) == Some(&name))
        }
        (LinkMode::Object, None) => (0, segments.len() == 1 && segments[0] == name),
        (LinkMode::Object, Some(prefix)) => (
            1,
            segments.len() == 2 && segments[0] == prefix && segments[1] == name,
        ),
        (LinkMode::Method, None) => (
            1,
            segments.len() == 2 && receiver == Some(segments[0]) && segments[1] == name,
        ),
        (LinkMode::Method, Some(prefix)) => (
            2,
            segments.len() == 3
                && segments[0] == prefix
                && receiver == Some(segments[1])
                && segments[2] == name,
        ),
    };
    if !matched {
        return None;
    }
    for (position, segment) in segments.iter().enumerate() {
        if position == index {
            return Some(offset);
        }
        offset += segment.len() + 1;
    }
    None
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn finalize(mut edits: Vec<RenameEdit>) -> Vec<RenameEdit> {
    edits.sort_by(|left, right| {
        left.file
            .cmp(&right.file)
            .then_with(|| left.range.start.line.cmp(&right.range.start.line))
            .then_with(|| {
                left.range
                    .start
                    .utf16_column
                    .cmp(&right.range.start.utf16_column)
            })
            .then_with(|| left.range.end.line.cmp(&right.range.end.line))
            .then_with(|| {
                left.range
                    .end
                    .utf16_column
                    .cmp(&right.range.end.utf16_column)
            })
    });
    edits.dedup_by(|left, right| {
        left.file == right.file
            && left.range.start.line == right.range.start.line
            && left.range.start.utf16_column == right.range.start.utf16_column
            && left.range.end.line == right.range.end.line
            && left.range.end.utf16_column == right.range.end.utf16_column
            && match (&left.text, &right.text) {
                (RenameText::Name, RenameText::Name) => true,
                (
                    RenameText::ImportPrefix { imported: left },
                    RenameText::ImportPrefix { imported: right },
                ) => left == right,
                _ => false,
            }
    });
    edits
}

fn target_name(source: &str, target: &Target) -> String {
    match target {
        Target::Global { symbol, .. } => symbol.name.clone(),
        Target::Member(member) => member.name.clone(),
        Target::Local { selection, .. } => source[selection.clone()].to_owned(),
        Target::Import { name, .. } => name.clone(),
        Target::Label { spans } => spans
            .first()
            .map(|span| source[span.clone()].to_owned())
            .unwrap_or_default(),
        Target::AnonymousField { name, .. } => name.clone(),
    }
}

fn same_field_owner(
    workspace: &Workspace,
    file: &Path,
    ty: &str,
    target: &MemberDeclaration<'_>,
) -> bool {
    let Some(name) = scope::named_type(ty) else {
        return false;
    };
    let Some((owner_file, owner)) = workspace
        .underlying_target(file, &name)
        .or_else(|| workspace.type_declaration(file, &name))
    else {
        return false;
    };
    owner_file == target.file && owner.start == target.owner.start
}

/// Result slot of a direct call initializer (`x, _, err := f(...)`), resolved
/// from the callee's declaration. The shared binding inference can fail to
/// project this slot for some paths; the call syntax stays an exact proof.
fn call_initializer_type(
    workspace: &Workspace,
    file: &Path,
    source: &str,
    binding: &scope::Binding,
) -> Option<String> {
    let (span, slot, count) = binding.initializer()?;
    if span.len() > 16 * 1024 {
        return None;
    }
    let text = source.get(span.clone())?;
    Context::borrowed(workspace, file, source).direct_call_result_type(
        file,
        text,
        slot,
        count,
        false,
        |_| false,
    )
}

fn receiver_base(receiver: &str) -> &str {
    receiver.trim_start_matches('*')
}

fn contains_span(span: &Range<usize>, offset: usize) -> bool {
    span.start <= offset && offset <= span.end
}

/// Identifier token containing the cursor, inclusive of its end boundary.
fn ident_at(source: &str, offset: usize) -> Option<Range<usize>> {
    Lexer::new(source)
        .take_while(|token| token.span.start <= offset)
        .find(|token| {
            token.kind == TokenKind::Ident && token.span.start <= offset && offset <= token.span.end
        })
        .map(|token| token.span)
}

/// Cursor identifier with gopls' `*`-operand nudge: a selection on `*T` or
/// `*expr` is treated as a selection on the following identifier.
fn cursor_identifier(source: &str, offset: usize) -> Option<Range<usize>> {
    if let Some(ident) = ident_at(source, offset) {
        return Some(ident);
    }
    let mut tokens = Lexer::new(source)
        .filter(|token| token.kind != TokenKind::Comment)
        .peekable();
    while let Some(token) = tokens.next() {
        if token.span.start > offset {
            return None;
        }
        if token.span.start <= offset && offset <= token.span.end && token.kind == TokenKind::Mul {
            return tokens
                .next()
                .filter(|next| next.kind == TokenKind::Ident)
                .map(|next| next.span);
        }
    }
    None
}

/// Whether the identifier is used as a selector qualifier (`ident.`).
fn qualifier_use(source: &str, ident: &Range<usize>) -> bool {
    let mut tokens = Lexer::new(source).filter(|token| token.kind != TokenKind::Comment);
    let mut previous = None;
    for token in tokens.by_ref() {
        if token.span.start == ident.start {
            if token.kind != TokenKind::Ident || token.span != *ident {
                return false;
            }
            let next = tokens.next();
            return next.is_some_and(|next| next.kind == TokenKind::Period)
                && !matches!(
                    previous.map(|previous: crate::syntax::Token| previous.kind),
                    Some(TokenKind::Period)
                );
        }
        if token.span.start > ident.start {
            return false;
        }
        previous = Some(token);
    }
    false
}

/// Package clause name span, if any.
fn package_name_span(source: &str) -> Option<Range<usize>> {
    let mut tokens = Lexer::new(source).filter(|token| token.kind != TokenKind::Comment);
    let first = tokens.next()?;
    if first.kind != TokenKind::Package {
        return None;
    }
    let name = tokens.find(|token| token.kind != TokenKind::Semicolon)?;
    (name.kind == TokenKind::Ident).then_some(name.span)
}

/// Whether the cursor is inside a `func` keyword of a declaration or literal.
fn func_keyword_at(source: &str, offset: usize) -> bool {
    Lexer::new(source)
        .take_while(|token| token.span.start <= offset)
        .any(|token| {
            token.kind == TokenKind::Func && token.span.start <= offset && offset <= token.span.end
        })
}

fn valid_identifier(name: &str) -> bool {
    let mut characters = name.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    if name == "_" || !crate::syntax::is_identifier_start(first) {
        return false;
    }
    if !characters.all(crate::syntax::is_identifier_continue) {
        return false;
    }
    TokenKind::from_keyword(name).is_none()
}

/// Byte span of a member declaration identifier in `source`.
fn member_declaration_span(member: &MemberDeclaration<'_>, source: &str) -> Option<Range<usize>> {
    let map = SourceMap::new(source);
    let base = member.owner.end;
    let end = map.offset(member.owner.range.end)?;
    let declaration = source.get(base..end)?;
    let entries = crate::syntax::aggregate_members(declaration);
    let entry = member.entry(declaration, &entries)?;
    let span = if member.embedded {
        let annotation = declaration.get(entry.annotation.clone())?;
        let token = Lexer::new(annotation)
            .take_while(|token| token.kind != TokenKind::LBrack)
            .filter(|token| token.kind == TokenKind::Ident)
            .last()?;
        entry.annotation.start + token.span.start..entry.annotation.start + token.span.end
    } else {
        entry.selection.clone()
    };
    Some(base + span.start..base + span.end)
}

fn member_declaration_range(
    workspace: &Workspace,
    member: &MemberDeclaration<'_>,
) -> Option<SourceRange> {
    let source = workspace.source(member.file)?;
    let span = member_declaration_span(member, &source)?;
    let map = SourceMap::new(&source);
    Some(SourceRange::new(&map, span))
}

/// Documentation comment block immediately preceding the declaration line.
fn doc_comment_block(source: &str, offset: usize) -> Option<Range<usize>> {
    let line_start = source[..offset.min(source.len())]
        .rfind('\n')
        .map_or(0, |index| index + 1);
    let mut start = line_start;
    let mut found = false;
    for _ in 0..512 {
        if start == 0 {
            break;
        }
        let previous_end = start - 1;
        let previous_start = source[..previous_end]
            .rfind('\n')
            .map_or(0, |index| index + 1);
        let line = source[previous_start..previous_end].trim();
        if line.is_empty() {
            break;
        }
        if line.starts_with("//") {
            found = true;
            start = previous_start;
            continue;
        }
        if line.ends_with("*/") {
            let open = source[..previous_end].rfind("/*")?;
            if previous_end - open > 16 * 1024 {
                return None;
            }
            found = true;
            start = open;
        }
        break;
    }
    found.then_some(start..line_start)
}

/// Replaces whole-word occurrences of `from` inside a comment block.
fn word_edits(
    source: &str,
    map: &SourceMap<'_>,
    block: Range<usize>,
    from: &str,
    file: &Path,
    edits: &mut Vec<RenameEdit>,
) {
    let text = &source[block.clone()];
    let mut line_start = 0;
    for line in text.split('\n') {
        let trimmed = line.trim_start();
        let directive = trimmed.starts_with("//") && is_directive(trimmed);
        if !directive {
            let mut search = 0;
            while let Some(index) = line[search..].find(from) {
                let index = search + index;
                let before = line[..index].chars().next_back();
                let after = line[index + from.len()..].chars().next();
                let before_ok = before
                    .is_none_or(|character| !crate::syntax::is_identifier_continue(character));
                let after_ok =
                    after.is_none_or(|character| !crate::syntax::is_identifier_continue(character));
                if before_ok && after_ok {
                    let start = block.start + line_start + index;
                    edits.push(RenameEdit {
                        file: file.to_path_buf(),
                        range: SourceRange::new(map, start..start + from.len()),
                        text: RenameText::Name,
                    });
                }
                search = index + from.len();
            }
        }
        line_start += line.len() + 1;
    }
}

/// Port of gopls' `isDirective` for `//` comments.
fn is_directive(comment: &str) -> bool {
    let Some(text) = comment.strip_prefix("//") else {
        return false;
    };
    if text.is_empty() {
        return false;
    }
    if text.starts_with("line ") {
        return true;
    }
    let Some(colon) = text.find(':') else {
        return false;
    };
    if colon == 0 || colon + 1 >= text.len() {
        return false;
    }
    for (index, character) in text.char_indices() {
        if index > colon + 1 {
            break;
        }
        if index == colon {
            continue;
        }
        if !matches!(character, 'a'..='z' | '0'..='9') {
            return false;
        }
    }
    true
}

/// Whether a blank identifier is a declaration (rename forbidden with gopls'
/// "can't rename" message) rather than a plain `_ =` value.
fn blank_declaration(source: &str, ident: &Range<usize>) -> bool {
    let mut tokens = Lexer::new(source)
        .filter(|token| token.kind != TokenKind::Comment)
        .peekable();
    let mut previous = None;
    while let Some(token) = tokens.next() {
        if token.span.start == ident.start {
            let next = tokens.next().map(|next| next.kind);
            return match next {
                Some(TokenKind::Assign) => {
                    matches!(previous, Some(TokenKind::Var) | Some(TokenKind::Const))
                }
                Some(
                    TokenKind::Comma
                    | TokenKind::RParen
                    | TokenKind::RBrack
                    | TokenKind::Ident
                    | TokenKind::Mul
                    | TokenKind::LBrack
                    | TokenKind::Map
                    | TokenKind::Chan
                    | TokenKind::Func
                    | TokenKind::Interface
                    | TokenKind::Struct
                    | TokenKind::Ellipsis
                    | TokenKind::Period,
                ) => true,
                _ => false,
            };
        }
        if token.span.start > ident.start {
            return false;
        }
        previous = Some(token.kind);
    }
    false
}

/// Protocol position of a byte offset: `dir/file.go:line:column` (1-based).
fn position_text(
    workspace: &Workspace,
    file: &Path,
    position: SourcePosition,
) -> Result<String, (i32, String)> {
    let source = workspace
        .source(file)
        .ok_or((crate::error::INVALID_PARAMS, "file unavailable".to_string()))?;
    let line = usize::try_from(position.line)
        .map_err(|_| (crate::error::INVALID_PARAMS, "bad line".to_string()))?;
    let mut start = 0;
    for _ in 0..line {
        start = source[start..]
            .find('\n')
            .map(|index| start + index + 1)
            .ok_or((crate::error::INVALID_PARAMS, "bad line".to_string()))?;
    }
    let end = source[start..]
        .find('\n')
        .map_or(source.len(), |index| start + index);
    let column = utf16_to_byte_column(&source[start..end], position.utf16_column);
    let path = file.to_string_lossy().replace('\\', "/");
    let segments: Vec<&str> = path.split('/').collect();
    let short = if segments.len() > 2 {
        segments[segments.len() - 2..].join("/")
    } else {
        path
    };
    Ok(format!("{short}:{}:{column}", position.line + 1))
}

fn utf16_to_byte_column(line: &str, utf16_column: u32) -> usize {
    let mut units = 0u32;
    for (index, character) in line.char_indices() {
        if units >= utf16_column {
            return index + 1;
        }
        units += u32::try_from(character.len_utf16()).unwrap_or(1);
    }
    line.len() + 1
}

fn lsp_range(range: tower_lsp::lsp_types::Range) -> SourceRange {
    SourceRange {
        start: lsp_position(range.start),
        end: lsp_position(range.end),
    }
}

fn lsp_position(position: tower_lsp::lsp_types::Position) -> SourcePosition {
    SourcePosition {
        line: position.line,
        utf16_column: position.character,
    }
}
