//! Request-local document highlights resolved by symbol identity.
//!
//! Occurrences are matched by resolved identity, never by spelling alone. The
//! read/write classification follows the shared source occurrence roles plus a
//! bounded query-local scan of function bodies for declaration and assignment
//! write positions. Return/result control-flow highlights mirror the reference
//! server's identifier behavior. No source, syntax tree or identity survives the
//! request; bodies are parsed on demand and only spans are retained.
use super::{Context, SourceRange};
use crate::analysis::check::navigation::anonymous;
use crate::analysis::member::MemberDeclaration;
use crate::analysis::method_set::ReceiverMember;
use crate::analysis::scope::{self, DeclarationKind, Identity, Locals, Resolver};
use crate::analysis::source::{Name, Role, SourceAnalysis};
use crate::analysis::types::TypeForm;
use crate::index::{IndexedSymbol as Symbol, SymbolKind};
use crate::syntax::{BlockTree, Expr, ExprId, ExprKind, Lexer, StmtKind, TokenKind};
use crate::text::SourceMap;
use crate::workspace::Workspace;
use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};

const MAX_HIGHLIGHTS: usize = 4096;
const MAX_FUNCTIONS: usize = 512;
const MAX_STATEMENTS: usize = 16_384;
const MAX_NESTED_SCOPES: usize = 64;
const MAX_COMPOSITES: usize = 256;
const MAX_SHAPE_BYTES: usize = 16 * 1024;
const MAX_PRINTF_CALLS: usize = 1024;
const MAX_FORMAT_BYTES: usize = 16 * 1024;
const MAX_FORMAT_OPS: usize = 256;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum HighlightKind {
    Text,
    Read,
    Write,
}

pub(crate) struct HighlightRow {
    pub(crate) range: SourceRange,
    pub(crate) kind: HighlightKind,
}

/// Coarse object class shared by every occurrence of the matched identity.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Variable,
    Text,
}

#[derive(PartialEq, Eq)]
enum Canonical<'a> {
    Local(Range<usize>),
    /// A type-switch case variable: the shared guard declaration cannot
    /// distinguish clauses, so the clause scope start is part of the identity.
    Case {
        selection: usize,
        scope_start: usize,
    },
    Global(&'a Path, usize),
    Member(&'a Path, usize, &'a str),
    /// A field/method whose declaration is only reachable through the native
    /// selector-chain fallback: identity is the declaration site itself, so two
    /// structurally identical anonymous literals never merge.
    Declaration {
        file: PathBuf,
        start: usize,
    },
    /// A method declared by the universe `error` interface; the pinned SDK
    /// declaration is the only owner and the method spelling identifies it.
    BuiltinMethod(&'a str),
    Package(&'a str),
    Unresolved(&'a str),
    Singleton,
}

struct Cursor<'a> {
    selection: Range<usize>,
    spelling: &'a str,
    canonical: Canonical<'a>,
    class: Class,
    /// Declaration site of a member cursor, used to compare identities that the
    /// spelling resolver reaches through different paths.
    declaration: Option<(PathBuf, usize)>,
    /// True when the cursor identity came from the native fallback, so candidate
    /// occurrences must be resolved through the same fallback.
    fallback: bool,
    /// Symbol kind when the cursor is a package-level object; selector candidates
    /// can only resolve to methods (kind 6) for this identity.
    global_kind: Option<SymbolKind>,
}

impl<'a> Context<'a> {
    /// Highlight rows in file order, or `None` for the protocol's null result.
    pub(crate) fn document_highlights(
        &self,
        offset: usize,
    ) -> Result<Option<Vec<HighlightRow>>, (i32, String)> {
        let source = self.source();
        let Some(selection) = crate::analysis::source::identifier_at(source, offset) else {
            return Ok(None);
        };
        let file = self.file();
        let workspace = self.workspace;
        let Some(stub) = workspace.files.get(file) else {
            return Ok(None);
        };
        let trace = workspace.trace.queries.then(std::time::Instant::now);
        let phase = workspace.trace.queries.then(std::time::Instant::now);
        let Ok(analysis) = SourceAnalysis::new(source, stub) else {
            return Ok(None);
        };
        let analysis_time = phase.map(|phase| phase.elapsed());
        let Some(cursor_name) = analysis.names.iter().find(|name| name.span == selection) else {
            return Ok(None);
        };
        let map = SourceMap::new(source);
        let phase = workspace.trace.queries.then(std::time::Instant::now);
        let mut scanner = Scanner::new(workspace, file, source);
        scanner.collect(stub);
        let collect_time = phase.map(|phase| phase.elapsed());
        if scanner.exhausted {
            return Ok(None);
        }
        let phase = workspace.trace.queries.then(std::time::Instant::now);
        let inline_index = anonymous::InlineIndex::new(source);
        let cursor = cursor(
            workspace,
            file,
            source,
            &map,
            &analysis,
            cursor_name,
            &scanner.declarations,
            scanner.package_name.as_ref(),
            &inline_index,
        );
        let cursor_time = phase.map(|phase| phase.elapsed());
        let loop_phase = workspace.trace.queries.then(std::time::Instant::now);
        if workspace.trace.queries {
            eprintln!(
                "highlight cursor {:?} canonical {} declaration {:?}",
                cursor.spelling,
                canonical_tag(&cursor.canonical),
                cursor.declaration
            );
        }
        let mut rows: Vec<(Range<usize>, HighlightKind)> = Vec::new();
        printf_rows(&scanner.printf_calls, source, offset, &mut rows);
        control_flow_rows(&scanner.functions, offset, &mut rows);
        if let Canonical::Package(alias) = &cursor.canonical
            && let Some(span) = import_spec_span(workspace, source, file, alias)
        {
            rows.push((span, HighlightKind::Text));
        }
        let mut resolver = Resolver::new(workspace, file);
        let mut candidates = 0usize;
        let mut match_time = std::time::Duration::ZERO;
        let mut extra_time = std::time::Duration::ZERO;
        let mut kind_time = std::time::Duration::ZERO;
        for name in &analysis.names {
            if &source[name.span.clone()] != cursor.spelling {
                continue;
            }
            candidates += 1;
            if candidates > MAX_HIGHLIGHTS {
                return Ok(None);
            }
            if !candidate_role_plausible(&cursor, name) {
                continue;
            }
            let phase = workspace.trace.queries.then(std::time::Instant::now);
            let declaration_decision = cursor.declaration.as_ref().and_then(|target| {
                if matches!(cursor.canonical, Canonical::Declaration { .. }) {
                    Some(inline_declaration_matches(
                        workspace,
                        file,
                        source,
                        name,
                        &inline_index,
                        target,
                    ))
                } else if matches!(
                    cursor.canonical,
                    Canonical::Member(_, _, _) | Canonical::Global(_, _)
                ) && matches!(name.role, Role::Member(_) | Role::ExpressionMember(_))
                {
                    anonymous::chain_location(workspace, file, source, &name.span, name.span.start)
                        .map(|(decl_file, selection)| {
                            decl_file == target.0 && selection.start == target.1
                        })
                } else {
                    None
                }
            });
            let matched = match declaration_decision {
                Some(decision) => decision,
                None => occurrence_matches(
                    workspace,
                    file,
                    source,
                    &map,
                    &analysis,
                    &mut resolver,
                    name,
                    &cursor,
                    scanner.package_name.as_ref(),
                ),
            };
            if let Some(phase) = phase {
                let elapsed = phase.elapsed();
                match_time += elapsed;
                if elapsed > std::time::Duration::from_millis(2) {
                    eprintln!(
                        "highlight slow candidate {:?} role {} match {elapsed:?}",
                        name.span,
                        role_tag_str(&name.role)
                    );
                }
            }
            let phase = workspace.trace.queries.then(std::time::Instant::now);
            if !matched {
                let matches_builtin = match &cursor.canonical {
                    Canonical::BuiltinMethod(method) => {
                        builtin_interface_method(workspace, file, source, &analysis, name)
                            == Some(*method)
                    }
                    _ => false,
                };
                let matches_declaration = cursor.declaration.as_ref().is_some_and(|target| {
                    if matches!(cursor.canonical, Canonical::Member(_, _, _)) {
                        // Named member identity already compares the complete
                        // declaration; only anonymous/interface declarations
                        // need the fallback declaration walk.
                        false
                    } else if matches!(cursor.canonical, Canonical::Declaration { .. }) {
                        inline_declaration_matches(
                            workspace,
                            file,
                            source,
                            name,
                            &inline_index,
                            target,
                        )
                    } else {
                        occurrence_declaration(
                            workspace,
                            file,
                            source,
                            &analysis,
                            &mut resolver,
                            name,
                            &inline_index,
                        )
                        .as_ref()
                            == Some(target)
                    }
                });
                let matches_fallback = cursor.fallback
                    && matches!(
                        cursor.canonical,
                        Canonical::Global(_, _)
                            | Canonical::Declaration { .. }
                            | Canonical::Member(_, _, _)
                    )
                    && fallback_member(workspace, file, source, &analysis, name, &inline_index)
                        .is_some_and(|(canonical, _, _)| canonical == cursor.canonical);
                if !(matches_builtin || matches_declaration || matches_fallback) {
                    if let Some(phase) = phase {
                        extra_time += phase.elapsed();
                    }
                    continue;
                }
            }
            if let Some(phase) = phase {
                let elapsed = phase.elapsed();
                extra_time += elapsed;
                if elapsed > std::time::Duration::from_millis(2) {
                    eprintln!(
                        "highlight slow candidate {:?} role {} extra {elapsed:?}",
                        name.span,
                        role_tag_str(&name.role)
                    );
                }
            }
            let phase = workspace.trace.queries.then(std::time::Instant::now);
            let kind = occurrence_kind(name, &cursor, &scanner.writes);
            let kind = if kind != HighlightKind::Write
                && matches!(name.role, Role::LiteralKey(_) | Role::LiteralElementKey(_))
                && matches!(
                    cursor.canonical,
                    Canonical::Member(_, _, _)
                        | Canonical::Declaration { .. }
                        | Canonical::BuiltinMethod(_)
                ) {
                // A matched keyed element of a member identity writes that
                // field; the match already proved the concrete member.
                HighlightKind::Write
            } else {
                kind
            };
            if let Some(phase) = phase {
                kind_time += phase.elapsed();
            }
            if !rows.iter().any(|(span, _)| *span == name.span) {
                rows.push((name.span.clone(), kind));
            }
        }
        if rows.is_empty() {
            return Ok(None);
        }
        if let Some(start) = trace {
            eprintln!(
                "highlight timing: analysis {analysis_time:?}; collect {collect_time:?}; cursor {cursor_time:?}; loop {:?}; match {match_time:?}; extra {extra_time:?}; kind {kind_time:?}; names {} candidates {candidates}; rows {}; total {:?}",
                loop_phase.map_or(std::time::Duration::ZERO, |phase| phase.elapsed()),
                analysis.names.len(),
                rows.len(),
                start.elapsed()
            );
        }
        let mut result: Vec<_> = rows
            .into_iter()
            .map(|(span, kind)| HighlightRow {
                range: SourceRange::new(&map, span),
                kind,
            })
            .collect();
        result.sort_by_key(|row| {
            (
                row.range.start.line,
                row.range.start.utf16_column,
                row.range.end.line,
                row.range.end.utf16_column,
            )
        });
        result.truncate(MAX_HIGHLIGHTS);
        Ok(Some(result))
    }
}

#[allow(clippy::too_many_arguments)]
fn cursor<'a>(
    workspace: &'a Workspace,
    file: &'a Path,
    source: &'a str,
    map: &SourceMap<'_>,
    analysis: &SourceAnalysis,
    name: &Name,
    declarations: &[Range<usize>],
    package_name: Option<&Range<usize>>,
    inline: &anonymous::InlineIndex,
) -> Cursor<'a> {
    let spelling = &source[name.span.clone()];
    let identity = Resolver::new(workspace, file).resolve(analysis, source, name);
    let member = workspace.member_declaration_target(file, source, &name.span, map);
    let imported = scope::universe::imported_name(workspace, file, spelling);
    // Embedded struct fields resolve to their implicit field variable, while an
    // embedded interface member is a type reference. The member inventory
    // distinguishes the two forms before the type resolution is used.
    let embedded_member = matches!(name.role, Role::Field | Role::EmbeddedType(_));
    let identity_member = match &identity {
        Some(Identity::Member(member)) => Some(*member),
        _ => None,
    };
    let (canonical, class) = match (identity, member) {
        (_, Some(member)) if embedded_member => (
            Canonical::Member(member.file, member.owner.start, member.name),
            member_class(member),
        ),
        (Some(Identity::Global(path, symbol)), _) => (
            Canonical::Global(path, symbol.start),
            if symbol.is_var() {
                Class::Variable
            } else {
                Class::Text
            },
        ),
        (Some(Identity::Member(member)), _) => (
            Canonical::Member(member.file, member.owner.start, member.name),
            member_class(member),
        ),
        (Some(Identity::Local(selection)), _) => {
            if name.span != selection
                && let Some(binding) = analysis.locals.resolve_at(name.span.start, spelling)
                && binding.selection == selection
                && analysis.locals.switch_case(binding).is_some()
            {
                (
                    Canonical::Case {
                        selection: selection.start,
                        scope_start: binding.scope_start(),
                    },
                    Class::Variable,
                )
            } else {
                let class = analysis
                    .locals
                    .definition_at(&selection, spelling)
                    .filter(|binding| binding.selection == selection)
                    .map_or(Class::Variable, |binding| match binding.declaration {
                        DeclarationKind::Constant
                        | DeclarationKind::Type
                        | DeclarationKind::TypeParameter => Class::Text,
                        DeclarationKind::Value | DeclarationKind::Variadic => Class::Variable,
                    });
                (Canonical::Local(selection), class)
            }
        }
        (None, Some(member)) => (
            Canonical::Member(member.file, member.owner.start, member.name),
            member_class(member),
        ),
        (None, None) => {
            let package_clause = package_name == Some(&name.span);
            if imported && !package_clause {
                (Canonical::Package(spelling), Class::Text)
            } else if spelling == "_" && declarations.contains(&name.span) {
                (Canonical::Singleton, Class::Text)
            } else {
                (Canonical::Unresolved(spelling), Class::Text)
            }
        }
    };
    let mut declaration = identity_member
        .and_then(|member| member_offset(workspace, member))
        .or_else(|| member.and_then(|member| member_offset(workspace, member)));
    let (mut canonical, mut class) = (canonical, class);
    let mut fallback = false;
    if matches!(canonical, Canonical::Unresolved(_))
        && let Some((fallback_canonical, fallback_class, fallback_declaration)) =
            fallback_member(workspace, file, source, analysis, name, inline)
    {
        canonical = fallback_canonical;
        class = fallback_class;
        fallback = true;
        if fallback_declaration.is_some() {
            declaration = fallback_declaration;
        }
        if let Canonical::Declaration { file, start } = &canonical {
            declaration = Some((file.clone(), *start));
        }
    }
    let mut global_kind = None;
    if let Canonical::Global(path, start) = &canonical
        && let Some(stub) = workspace.files.get(*path)
        && let Some(symbol) = stub.symbols.iter().find(|symbol| symbol.start == *start)
    {
        global_kind = Some(symbol.kind);
    }
    if fallback && matches!(canonical, Canonical::Global(_, _)) {
        // The native fallback only produces Global identities for method
        // declarations (receiver_member Method), never for other globals.
        global_kind = Some(SymbolKind::Method);
    }
    Cursor {
        selection: name.span.clone(),
        spelling,
        canonical,
        class,
        declaration,
        fallback,
        global_kind,
    }
}

fn canonical_tag(canonical: &Canonical<'_>) -> &'static str {
    match canonical {
        Canonical::Local(_) => "local",
        Canonical::Case { .. } => "case",
        Canonical::Global(_, _) => "global",
        Canonical::Member(_, _, _) => "member",
        Canonical::Declaration { .. } => "declaration",
        Canonical::BuiltinMethod(_) => "builtin-method",
        Canonical::Package(_) => "package",
        Canonical::Unresolved(_) => "unresolved",
        Canonical::Singleton => "singleton",
    }
}

fn role_tag_str(role: &Role) -> &'static str {
    match role {
        Role::Name => "name",
        Role::Type(_) => "type",
        Role::EmbeddedType(_) => "embedded",
        Role::Member(Some(_)) => "member-qualified",
        Role::Member(None) => "member",
        Role::ExpressionMember(_) => "expression-member",
        Role::LiteralKey(_) => "literal-key",
        Role::LiteralElementKey(_) => "literal-element-key",
        Role::BlankDeclaration { .. } => "blank",
        Role::Parameter => "parameter",
        Role::Result => "result",
        Role::Field => "field",
        Role::FieldDeclaration(_) => "field-declaration",
        Role::Namespace => "namespace",
        Role::Ignored => "ignored",
    }
}

/// Cheap role prefilter: the spelling filter already bounds candidates, and a
/// selector role can never resolve to a local or a non-method global identity,
/// so those candidate sets never need a full resolution.
fn candidate_role_plausible(cursor: &Cursor<'_>, name: &Name) -> bool {
    let selector = matches!(
        name.role,
        Role::Member(_)
            | Role::ExpressionMember(_)
            | Role::LiteralKey(_)
            | Role::LiteralElementKey(_)
    );
    match &cursor.canonical {
        Canonical::Local(_) | Canonical::Case { .. } => !selector,
        Canonical::Global(_, _) => {
            // Package-qualified uses are member-shaped but can denote any
            // imported global; a bare selector chain resolves to a method only.
            !selector
                || cursor.global_kind == Some(SymbolKind::Method)
                || matches!(name.role, Role::Member(Some(_)))
        }
        Canonical::Member(_, _, _)
        | Canonical::Declaration { .. }
        | Canonical::BuiltinMethod(_) => matches!(
            name.role,
            Role::Member(_)
                | Role::ExpressionMember(_)
                | Role::LiteralKey(_)
                | Role::LiteralElementKey(_)
                | Role::Field
                | Role::EmbeddedType(_)
                | Role::FieldDeclaration(_)
        ),
        Canonical::Package(_) | Canonical::Unresolved(_) | Canonical::Singleton => true,
    }
}

/// Inline-struct field declaration matching without the general resolver: the
/// field's identity is the written declaration site, so literal keys and plain
/// selector chains are checked directly and unrelated selectors are rejected
/// before any type inference runs.
fn inline_declaration_matches(
    workspace: &Workspace,
    file: &Path,
    source: &str,
    name: &Name,
    inline: &anonymous::InlineIndex,
    target: &(PathBuf, usize),
) -> bool {
    if let Some(selection) = anonymous::field_target_in(source, inline, &name.span) {
        return target.0 == file && target.1 == selection.start;
    }
    if !plain_selector_chain(&source[name.span.clone()]) {
        return false;
    }
    match anonymous::field_declaration_in(
        workspace,
        file,
        source,
        &name.span,
        name.span.start,
        inline,
    ) {
        Some(field) => field.file == target.0 && field.selection.start == target.1,
        None => false,
    }
}

/// True for `a.b.c`-shaped selectors (identifiers separated by dots), the only
/// chains that can resolve through the plain binding/field walk.
fn plain_selector_chain(text: &str) -> bool {
    let mut tokens = crate::syntax::Lexer::new(text);
    let Some(first) = tokens.next() else {
        return false;
    };
    if first.kind != crate::syntax::TokenKind::Ident {
        return false;
    }
    let mut expect_ident = false;
    for token in tokens {
        match (expect_ident, token.kind) {
            (false, crate::syntax::TokenKind::Period) => expect_ident = true,
            (true, crate::syntax::TokenKind::Ident) => expect_ident = false,
            _ => return false,
        }
    }
    !expect_ident
}

/// Declaration site (file and byte offset of the name) of a resolved member.
fn member_offset(workspace: &Workspace, member: MemberDeclaration<'_>) -> Option<(PathBuf, usize)> {
    let data = workspace.member_data(member)?;
    let source = workspace.source(member.file)?;
    let map = SourceMap::new(&source);
    Some((member.file.to_path_buf(), map.offset(data.selection.start)?))
}

/// Field or method identity for occurrences that the spelling resolver cannot
/// key: anonymous struct type literals, comma-ok assertion bindings and nested
/// selector chains. Identity is the declaration site, never a type shape.
#[allow(clippy::type_complexity)]
fn fallback_member<'a>(
    workspace: &'a Workspace,
    file: &'a Path,
    source: &'a str,
    analysis: &SourceAnalysis,
    name: &Name,
    inline: &anonymous::InlineIndex,
) -> Option<(Canonical<'a>, Class, Option<(PathBuf, usize)>)> {
    if let Some(builtin) = builtin_interface_method(workspace, file, source, analysis, name) {
        return Some((Canonical::BuiltinMethod(builtin), Class::Text, None));
    }
    if let Some(receiver) = assertion_receiver_type(workspace, file, source, analysis, name)
        && let Some(member) = workspace.receiver_member(file, &receiver, &source[name.span.clone()])
    {
        return match member {
            ReceiverMember::Method(member_file, symbol) => Some((
                Canonical::Global(member_file.as_path(), symbol.start),
                Class::Text,
                None,
            )),
            ReceiverMember::Declaration(declaration) => Some((
                Canonical::Member(declaration.file, declaration.owner.start, declaration.name),
                member_class(declaration),
                member_offset(workspace, declaration),
            )),
        };
    }
    if let Some(field) = anonymous::field_declaration_in(
        workspace,
        file,
        source,
        &name.span,
        name.span.start,
        inline,
    ) {
        return Some((
            Canonical::Declaration {
                file: field.file,
                start: field.selection.start,
            },
            Class::Variable,
            None,
        ));
    }
    let (decl_file, selection) =
        anonymous::chain_location(workspace, file, source, &name.span, name.span.start)?;
    let decl_source = workspace.source(&decl_file)?;
    let map = SourceMap::new(&decl_source);
    let class = workspace
        .member_declaration_target(&decl_file, &decl_source, &selection, &map)
        .map_or(Class::Variable, member_class);
    Some((
        Canonical::Declaration {
            file: decl_file,
            start: selection.start,
        },
        class,
        None,
    ))
}

/// Receiver type of a selector whose base is a comma-ok type-assertion binding:
/// `cp, ok := x.(T)` binds the value slot to `T`, which legacy binding inference
/// projects as a sequence and rejects.
fn assertion_receiver_type(
    workspace: &Workspace,
    file: &Path,
    source: &str,
    analysis: &SourceAnalysis,
    name: &Name,
) -> Option<String> {
    let _ = (workspace, file);
    let span = match &name.role {
        Role::Member(Some(qualifier)) => qualifier.start..name.span.end,
        Role::ExpressionMember(operand) => operand.start..name.span.end,
        _ => return None,
    };
    let parsed = crate::syntax::parse_expression(source.get(span.clone())?);
    if !parsed.diagnostics.is_empty() {
        return None;
    }
    let ExprKind::Selector { operand, .. } = &parsed.nodes[parsed.root.0].kind else {
        return None;
    };
    let expression = &parsed.nodes[operand.0];
    if !matches!(expression.kind, ExprKind::Name) {
        return None;
    }
    let base = span.start + expression.span.start;
    let spelling = source.get(base..span.start + expression.span.end)?;
    let binding = analysis.locals.resolve_at(base, spelling)?;
    let (initializer_span, result, count) = binding.initializer()?;
    if result != 0 || count != 2 {
        return None;
    }
    let initializer = crate::syntax::parse_expression(source.get(initializer_span.clone())?);
    if !initializer.diagnostics.is_empty() {
        return None;
    }
    let ExprKind::TypeAssertion { asserted_type, .. } = &initializer.nodes[initializer.root.0].kind
    else {
        return None;
    };
    source
        .get(
            initializer_span.start + asserted_type.start
                ..initializer_span.start + asserted_type.end,
        )
        .map(str::to_owned)
}

/// The universe `error` interface method behind an interface-valued selector.
fn builtin_interface_method<'a>(
    workspace: &Workspace,
    file: &Path,
    source: &'a str,
    analysis: &SourceAnalysis,
    name: &Name,
) -> Option<&'a str> {
    if &source[name.span.clone()] != "Error" {
        return None;
    }
    let span = match &name.role {
        Role::Member(Some(qualifier)) => qualifier.start..name.span.end,
        Role::ExpressionMember(operand) => operand.start..name.span.end,
        _ => return None,
    };
    let (receiver, field) = crate::analysis::check::interface_selector(
        workspace,
        file,
        source,
        &analysis.locals,
        span,
    )?;
    if receiver != "error" {
        return None;
    }
    let spelling = &source[name.span.clone()];
    (field == spelling).then_some(spelling)
}

/// Declaration site of any occurrence, including the native fallbacks.
fn occurrence_declaration(
    workspace: &Workspace,
    file: &Path,
    source: &str,
    analysis: &SourceAnalysis,
    resolver: &mut Resolver<'_>,
    name: &Name,
    inline: &anonymous::InlineIndex,
) -> Option<(PathBuf, usize)> {
    match resolver.resolve(analysis, source, name) {
        Some(Identity::Member(member)) => return member_offset(workspace, member),
        // Non-member identities can never equal a field/method declaration
        // identity, so the expensive declaration lookup is unnecessary.
        Some(_) => return None,
        None => {}
    }
    if matches!(
        name.role,
        Role::Field | Role::EmbeddedType(_) | Role::FieldDeclaration(_)
    ) {
        let map = SourceMap::new(source);
        if let Some(member) = workspace.member_declaration_target(file, source, &name.span, &map) {
            return member_offset(workspace, member);
        }
    }
    match fallback_member(workspace, file, source, analysis, name, inline) {
        Some((_, _, Some(declaration))) => Some(declaration),
        Some((Canonical::Declaration { file, start }, _, _)) => Some((file, start)),
        _ => None,
    }
}

fn member_class(member: MemberDeclaration<'_>) -> Class {
    if member.kind == crate::syntax::MemberKind::Method {
        Class::Text
    } else {
        Class::Variable
    }
}

/// The import spec span matched by a package-name cursor: the alias identifier
/// for explicit aliases, or the path literal for implicit names.
fn import_spec_span(
    workspace: &Workspace,
    source: &str,
    file: &Path,
    alias: &str,
) -> Option<Range<usize>> {
    let stub = workspace.files.get(file)?;
    let import = stub
        .imports
        .iter()
        .find(|import| workspace.import_name(import) == Some(alias))?;
    let explicit = import.explicit_name();
    let mut importing = false;
    let mut grouped = false;
    let mut alias_span = None;
    for token in Lexer::new(source) {
        match token.kind {
            TokenKind::Import => {
                importing = true;
                grouped = false;
                alias_span = None;
            }
            TokenKind::LParen if importing => grouped = true,
            TokenKind::RParen if importing && grouped => {
                importing = false;
                grouped = false;
            }
            TokenKind::Semicolon if importing && !grouped => {
                importing = false;
                alias_span = None;
            }
            TokenKind::Ident if importing => alias_span = Some(token.span.clone()),
            TokenKind::String if importing => {
                let path = crate::syntax::decode_import_path(&source[token.span.clone()])?;
                if path == import.path {
                    return Some(match (explicit, alias_span.take()) {
                        (Some(_), Some(span)) => span,
                        _ => token.span,
                    });
                }
                alias_span = None;
            }
            _ => {}
        }
    }
    None
}

/// A candidate occurrence matches when its resolved identity is the cursor's.
#[allow(clippy::too_many_arguments)]
fn occurrence_matches(
    workspace: &Workspace,
    file: &Path,
    source: &str,
    map: &SourceMap<'_>,
    analysis: &SourceAnalysis,
    resolver: &mut Resolver<'_>,
    name: &Name,
    cursor: &Cursor<'_>,
    package_name: Option<&Range<usize>>,
) -> bool {
    match &cursor.canonical {
        Canonical::Singleton => name.span == cursor.selection,
        Canonical::Package(alias) => {
            if &source[name.span.clone()] != *alias
                || !scope::universe::imported_name(workspace, file, alias)
            {
                return false;
            }
            match name.role {
                Role::Namespace => package_name != Some(&name.span),
                Role::Name => resolver.resolve(analysis, source, name).is_none(),
                _ => false,
            }
        }
        Canonical::Unresolved(spelling) => {
            if &source[name.span.clone()] != *spelling {
                return false;
            }
            if matches!(name.role, Role::Namespace) {
                // Import aliases and qualifier uses are package identities; the
                // package clause and blank imports stay in the nil-object group.
                if scope::universe::imported_name(workspace, file, spelling)
                    && package_name != Some(&name.span)
                {
                    return false;
                }
            } else if scope::universe::imported_name(workspace, file, spelling) {
                // A qualifier use belongs to the package-name identity.
                return false;
            }
            if !matches!(
                name.role,
                Role::Name | Role::Type(None) | Role::Namespace | Role::Member(None)
            ) {
                return false;
            }
            resolver.resolve(analysis, source, name).is_none()
        }
        Canonical::Global(path, start) => {
            // A struct-embedded type identifier defines the implicit field
            // variable, so it is not an occurrence of the type name.
            if matches!(name.role, Role::EmbeddedType(_))
                && workspace
                    .member_declaration_target(file, source, &name.span, map)
                    .is_some()
            {
                return false;
            }
            matches!(
                resolver.resolve(analysis, source, name),
                Some(Identity::Global(other, symbol)) if other == *path && symbol.start == *start
            )
        }
        Canonical::Member(target_file, owner_start, target_name) => {
            if let Some(Identity::Member(member)) = resolver.resolve(analysis, source, name) {
                return member.file == *target_file
                    && member.owner.start == *owner_start
                    && member.name == *target_name;
            }
            if matches!(name.role, Role::Field | Role::EmbeddedType(_))
                && let Some(member) =
                    workspace.member_declaration_target(file, source, &name.span, map)
            {
                return member.file == *target_file
                    && member.owner.start == *owner_start
                    && member.name == *target_name;
            }
            false
        }
        Canonical::Local(selection) => matches!(
            resolver.resolve(analysis, source, name),
            Some(Identity::Local(other)) if other == *selection
        ),
        Canonical::Declaration { .. } | Canonical::BuiltinMethod(_) => {
            // Compared through the cursor's declaration/builtin-method identity.
            false
        }
        Canonical::Case {
            selection,
            scope_start,
        } => {
            // The guard name is redeclared once per clause with the same
            // selection span; the containing clause scope separates them. Only
            // clause-body uses resolve, so the shared declaration itself stays
            // out of the highlighted set, matching the reference server.
            analysis
                .locals
                .resolve_at(name.span.start, &source[name.span.clone()])
                .is_some_and(|binding| {
                    binding.selection.start == *selection && binding.scope_start() == *scope_start
                })
        }
    }
}

fn occurrence_kind(name: &Name, cursor: &Cursor<'_>, writes: &[Range<usize>]) -> HighlightKind {
    if contains(writes, &name.span) {
        return HighlightKind::Write;
    }
    if matches!(cursor.canonical, Canonical::Declaration { .. })
        && matches!(name.role, Role::LiteralKey(_) | Role::LiteralElementKey(_))
    {
        // A keyed element of an anonymous struct literal writes that exact
        // field declaration identified by the fallback.
        return HighlightKind::Write;
    }
    match name.role {
        Role::Parameter
        | Role::Result
        | Role::Field
        | Role::FieldDeclaration(_)
        | Role::Namespace => HighlightKind::Text,
        Role::EmbeddedType(_) => match cursor.canonical {
            Canonical::Member(_, _, _) => HighlightKind::Read,
            _ => HighlightKind::Text,
        },
        _ => match cursor.class {
            Class::Variable => HighlightKind::Read,
            Class::Text => HighlightKind::Text,
        },
    }
}

fn contains(spans: &[Range<usize>], span: &Range<usize>) -> bool {
    spans
        .binary_search_by(|candidate| candidate.start.cmp(&span.start))
        .ok()
        .is_some_and(|index| spans[index] == *span)
}

/// Bounded query-local scan: declaration/assignment write positions and the
/// return/result metadata needed for control-flow highlights.
struct Scanner<'a, 'q> {
    workspace: &'a Workspace,
    file: &'a Path,
    source: &'q str,
    writes: Vec<Range<usize>>,
    declarations: Vec<Range<usize>>,
    /// The package clause name; it is a nil-object identity even when it
    /// spells an imported package's name.
    package_name: Option<Range<usize>>,
    functions: Vec<FunctionFacts>,
    printf_calls: Vec<PrintfCall>,
    statements: usize,
    composites: usize,
    nested: usize,
    exhausted: bool,
    shapes: HashMap<String, Shape>,
}

/// A printf-like call whose format literal and argument spans are query facts.
struct PrintfCall {
    span: Range<usize>,
    arguments: Vec<Range<usize>>,
    format_span: Range<usize>,
    format_index: usize,
}

/// One parsed printf operation with byte ranges in the logical format string.
struct FormatOp {
    range: Range<usize>,
    verb_range: Range<usize>,
    verb: u8,
    verb_arg: usize,
    width: Option<(Range<usize>, usize)>,
    precision: Option<(Range<usize>, usize)>,
}

struct FunctionFacts {
    body: Range<usize>,
    header: Range<usize>,
    results: Vec<ResultField>,
    returns: Vec<ReturnFacts>,
}

struct ResultField {
    names: Vec<Range<usize>>,
    annotation: Range<usize>,
    span: Range<usize>,
}

struct ReturnFacts {
    results: Vec<ReturnExpr>,
}

struct ReturnExpr {
    span: Range<usize>,
    eligible: Vec<Range<usize>>,
}

#[derive(Clone, PartialEq, Eq)]
enum Shape {
    Struct,
    Sequence(String),
    Map(String, String),
    Other,
}

impl<'a, 'q> Scanner<'a, 'q> {
    fn new(workspace: &'a Workspace, file: &'a Path, source: &'q str) -> Self {
        Self {
            workspace,
            file,
            source,
            writes: Vec::new(),
            declarations: Vec::new(),
            package_name: None,
            functions: Vec::new(),
            printf_calls: Vec::new(),
            statements: 0,
            composites: 0,
            nested: 0,
            exhausted: false,
            shapes: HashMap::new(),
        }
    }

    fn collect(&mut self, stub: &crate::index::IndexedFile) {
        self.collect_package_name();
        for symbol in &stub.symbols {
            if self.exhausted {
                break;
            }
            match symbol.kind {
                SymbolKind::Variable | SymbolKind::Constant => {
                    self.declarations.push(symbol.start..symbol.end);
                    self.writes.push(symbol.start..symbol.end);
                    self.initializer_keys(symbol);
                }
                SymbolKind::Function | SymbolKind::Method => {
                    if let (Some(declaration), Some(body)) =
                        (symbol.declaration(), symbol.body.clone())
                    {
                        self.function(declaration.start..body.start, body);
                    }
                }
                _ => {}
            }
        }
        self.writes.sort_by_key(|span| span.start);
        self.writes.dedup();
        self.declarations.sort_by_key(|span| span.start);
        self.declarations.dedup();
    }

    /// Records the package clause name: it stays a nil-object identity even
    /// when it spells an imported package's name.
    fn collect_package_name(&mut self) {
        for token in Lexer::new(self.source) {
            if token.kind == TokenKind::Package {
                self.package_name = Lexer::new(&self.source[token.span.end..])
                    .find(|token| token.kind == TokenKind::Ident)
                    .map(|next| token.span.end + next.span.start..token.span.end + next.span.end);
                return;
            }
        }
    }

    /// Package-level variable/constant initializers may contain struct literals.
    fn initializer_keys(&mut self, symbol: &Symbol) {
        let Some(declaration) = symbol.declaration() else {
            return;
        };
        let Some(text) = self.source.get(declaration.clone()) else {
            return;
        };
        let Some(offset) = top_level_assign(text) else {
            return;
        };
        let start = declaration.start + offset + 1;
        let Some(initializer) = self.source.get(start..declaration.end) else {
            return;
        };
        let parsed = crate::syntax::parse_expression(initializer);
        if parsed.diagnostics.is_empty() {
            let mut visited = vec![false; parsed.nodes.len()];
            // Parents are stored after their children; process them first so
            // elided literal elements inherit the expected element type.
            for id in (0..parsed.nodes.len()).rev() {
                self.collect_expression(&parsed.nodes, start, ExprId(id), None, 0, &mut visited);
            }
        }
    }

    fn function(&mut self, header: Range<usize>, body: Range<usize>) {
        if !self.enter_function() {
            return;
        }
        let Some(text) = self.source.get(body.clone()) else {
            return;
        };
        let tree = crate::syntax::parse_block(text);
        if !tree.diagnostics.is_empty() {
            return;
        }
        let mut facts = FunctionFacts {
            body: body.clone(),
            header: header.clone(),
            results: Vec::new(),
            returns: Vec::new(),
        };
        if let Some(header) = self.source.get(header) {
            let signature = crate::syntax::parse_signature(header);
            if signature.diagnostics.is_empty() {
                facts.results = result_fields(&signature.results, facts.header.start);
            }
        }
        self.walk_statements(&tree, body.start, &mut facts);
        self.walk_tree(&tree, body.start);
        self.functions.push(facts);
    }

    fn literal(&mut self, signature: Range<usize>, body: Range<usize>) {
        if !self.enter_function() {
            self.exhausted = self.functions.len() >= MAX_FUNCTIONS;
            return;
        }
        self.nested += 1;
        if self.nested > MAX_NESTED_SCOPES {
            self.exhausted = true;
            return;
        }
        let Some(text) = self.source.get(body.clone()) else {
            return;
        };
        let tree = crate::syntax::parse_block(text);
        if !tree.diagnostics.is_empty() {
            return;
        }
        let mut facts = FunctionFacts {
            body: body.clone(),
            header: signature.clone(),
            results: Vec::new(),
            returns: Vec::new(),
        };
        if let Some(header) = self.source.get(signature.clone()) {
            let parsed = crate::syntax::parse_literal_signature(header);
            if parsed.diagnostics.is_empty() {
                facts.results = result_fields(&parsed.results, signature.start);
            }
        }
        self.walk_statements(&tree, body.start, &mut facts);
        self.walk_tree(&tree, body.start);
        self.functions.push(facts);
        self.nested -= 1;
    }

    fn enter_function(&mut self) -> bool {
        if self.exhausted || self.functions.len() >= MAX_FUNCTIONS {
            return false;
        }
        true
    }

    fn walk_tree(&mut self, tree: &BlockTree, base: usize) {
        let mut visited = vec![false; tree.expressions.len()];
        // Children are stored before their parents; processing parents first
        // lets elided literal elements inherit the expected element type.
        for id in (0..tree.expressions.len()).rev() {
            self.collect_expression(&tree.expressions, base, ExprId(id), None, 0, &mut visited);
        }
    }

    fn walk_statements(&mut self, tree: &BlockTree, base: usize, facts: &mut FunctionFacts) {
        let mut pending = vec![tree.root];
        while let Some(id) = pending.pop() {
            if self.exhausted {
                return;
            }
            self.statements += 1;
            if self.statements > MAX_STATEMENTS {
                self.exhausted = true;
                return;
            }
            let Some(statement) = tree.statements.get(id.0) else {
                continue;
            };
            match &statement.kind {
                StmtKind::Block { statements } => pending.extend(statements.iter().copied()),
                StmtKind::Assignment { left, operator, .. } => {
                    for expression in left {
                        self.write_position(tree, base, *expression);
                    }
                    if self.source.get(base + operator.start..base + operator.end) == Some(":=") {
                        for expression in left {
                            self.declaration_position(tree, base, *expression);
                        }
                    }
                }
                StmtKind::Increment { expression, .. } => {
                    self.write_position(tree, base, *expression)
                }
                StmtKind::Send { channel, .. } => self.write_position(tree, base, *channel),
                StmtKind::Declaration { keyword, names, .. } => {
                    if matches!(
                        self.source.get(base + keyword.start..base + keyword.end),
                        Some("var" | "const")
                    ) {
                        for name in names {
                            self.writes.push(base + name.start..base + name.end);
                            self.declarations.push(base + name.start..base + name.end);
                        }
                    }
                }
                StmtKind::DeclarationGroup { declarations } => {
                    pending.extend(declarations.iter().copied())
                }
                StmtKind::Return { values } => {
                    let mut results = Vec::with_capacity(values.len());
                    for value in values {
                        let Some(span) = tree
                            .expressions
                            .get(value.0)
                            .map(|expression| expression.span.clone())
                        else {
                            continue;
                        };
                        let mut eligible = Vec::new();
                        eligible_spans(tree, *value, &mut eligible);
                        results.push(ReturnExpr {
                            span: base + span.start..base + span.end,
                            eligible: eligible
                                .into_iter()
                                .map(|span| base + span.start..base + span.end)
                                .collect(),
                        });
                    }
                    facts.returns.push(ReturnFacts { results });
                }
                StmtKind::RangeClause { left, .. } => {
                    for expression in left {
                        self.write_position(tree, base, *expression);
                        self.declaration_position(tree, base, *expression);
                    }
                }
                StmtKind::RangeFor { clause, body } => pending.extend([*clause, *body]),
                StmtKind::If {
                    init,
                    then_block,
                    otherwise,
                    ..
                } => {
                    pending.extend(init.iter().copied());
                    pending.push(*then_block);
                    pending.extend(otherwise.iter().copied());
                }
                StmtKind::For {
                    init, post, body, ..
                } => {
                    pending.extend(init.iter().copied());
                    pending.extend(post.iter().copied());
                    pending.push(*body);
                }
                StmtKind::Switch { init, cases, .. } => {
                    pending.extend(init.iter().copied());
                    pending.extend(cases.iter().copied());
                }
                StmtKind::TypeSwitch {
                    init, guard, cases, ..
                } => {
                    pending.extend(init.iter().copied());
                    pending.push(*guard);
                    pending.extend(cases.iter().copied());
                }
                StmtKind::Select { cases } => pending.extend(cases.iter().copied()),
                StmtKind::CommunicationCase {
                    communication,
                    statements,
                } => {
                    pending.extend(communication.iter().copied());
                    pending.extend(statements.iter().copied());
                }
                StmtKind::Case { statements, .. } => pending.extend(statements.iter().copied()),
                StmtKind::Label { statement, .. } => pending.extend(statement.iter().copied()),
                _ => {}
            }
        }
    }

    /// gopls marks a name assignment target when it is a name, selector field,
    /// dereference operand or parenthesized target.
    fn write_position(&mut self, tree: &BlockTree, base: usize, id: ExprId) {
        let Some(expression) = tree.expressions.get(id.0) else {
            return;
        };
        match &expression.kind {
            ExprKind::Name => self
                .writes
                .push(base + expression.span.start..base + expression.span.end),
            ExprKind::Selector { field, .. } => {
                self.writes.push(base + field.start..base + field.end)
            }
            ExprKind::Unary { operator, operand } => {
                if self.source.get(operator.start..operator.end) == Some("*") {
                    self.write_position(tree, base, *operand);
                }
            }
            ExprKind::Parenthesized { expression } => self.write_position(tree, base, *expression),
            _ => {}
        }
    }

    fn declaration_position(&mut self, tree: &BlockTree, base: usize, id: ExprId) {
        let Some(expression) = tree.expressions.get(id.0) else {
            return;
        };
        if matches!(expression.kind, ExprKind::Name) {
            self.declarations
                .push(base + expression.span.start..base + expression.span.end);
        }
    }

    fn collect_expression(
        &mut self,
        expressions: &[Expr],
        base: usize,
        id: ExprId,
        expected: Option<&Shape>,
        depth: usize,
        visited: &mut [bool],
    ) {
        if self.exhausted || depth > MAX_NESTED_SCOPES {
            self.exhausted |= depth > MAX_NESTED_SCOPES;
            return;
        }
        let index = id.0;
        if index >= expressions.len() {
            return;
        }
        if visited.get(index).copied().unwrap_or(true) {
            return;
        }
        visited[index] = true;
        let expression = &expressions[index];
        match &expression.kind {
            ExprKind::Composite {
                literal_type,
                elements,
            } => {
                if self.composites >= MAX_COMPOSITES {
                    return;
                }
                self.composites += 1;
                let shape = literal_type
                    .and_then(|literal_type| {
                        let span = expressions.get(literal_type.0)?.span.clone();
                        let annotation = self.source[base + span.start..base + span.end].to_owned();
                        self.shape(&annotation, depth)
                    })
                    .or_else(|| expected.cloned());
                let shape = shape.unwrap_or(Shape::Other);
                if shape == Shape::Struct {
                    for element in elements {
                        if let Some(Expr {
                            kind: ExprKind::KeyValue { key, .. },
                            ..
                        }) = expressions.get(element.0)
                            && let Some(Expr {
                                kind: ExprKind::Name,
                                span,
                                ..
                            }) = expressions.get(key.0)
                        {
                            self.writes.push(base + span.start..base + span.end);
                        }
                    }
                }
                let element_shape = match &shape {
                    Shape::Sequence(element) => self.shape(element, depth + 1),
                    _ => None,
                };
                let map_shape = match &shape {
                    Shape::Map(key, value) => {
                        Some((self.shape(key, depth + 1), self.shape(value, depth + 1)))
                    }
                    _ => None,
                };
                for element in elements {
                    let child = *element;
                    let child_index = child.0;
                    let Some(Expr {
                        kind: ExprKind::KeyValue { key, value },
                        ..
                    }) = expressions.get(child_index)
                    else {
                        self.collect_expression(
                            expressions,
                            base,
                            child,
                            element_shape.as_ref(),
                            depth + 1,
                            visited,
                        );
                        continue;
                    };
                    if let Some((key_shape, value_shape)) = &map_shape {
                        self.collect_expression(
                            expressions,
                            base,
                            *key,
                            key_shape.as_ref(),
                            depth + 1,
                            visited,
                        );
                        self.collect_expression(
                            expressions,
                            base,
                            *value,
                            value_shape.as_ref(),
                            depth + 1,
                            visited,
                        );
                    } else {
                        self.collect_expression(expressions, base, *key, None, depth + 1, visited);
                        self.collect_expression(
                            expressions,
                            base,
                            *value,
                            element_shape.as_ref(),
                            depth + 1,
                            visited,
                        );
                    }
                }
            }
            ExprKind::FunctionLiteral { signature, body } => {
                let signature = base + signature.start..base + signature.end;
                self.literal(signature, base + body.start..base + body.end);
            }
            _ => self.descend(expressions, base, id, depth, visited),
        }
    }

    /// Visits nested expressions that may contain literals or function literals.
    fn descend(
        &mut self,
        expressions: &[Expr],
        base: usize,
        id: ExprId,
        depth: usize,
        visited: &mut [bool],
    ) {
        let Some(expression) = expressions.get(id.0) else {
            return;
        };
        match &expression.kind {
            ExprKind::Composite { .. } | ExprKind::FunctionLiteral { .. } => {
                self.collect_expression(expressions, base, id, None, depth, visited)
            }
            ExprKind::Unary { operand, .. }
            | ExprKind::Parenthesized {
                expression: operand,
            }
            | ExprKind::TypeSwitchGuard { operand } => {
                self.collect_expression(expressions, base, *operand, None, depth + 1, visited)
            }
            ExprKind::Binary { left, right, .. } => {
                self.collect_expression(expressions, base, *left, None, depth + 1, visited);
                self.collect_expression(expressions, base, *right, None, depth + 1, visited);
            }
            ExprKind::Selector { operand, .. } => {
                self.collect_expression(expressions, base, *operand, None, depth + 1, visited)
            }
            ExprKind::Call {
                function,
                arguments,
                ..
            } => {
                self.record_printf(expressions, base, id);
                self.collect_expression(expressions, base, *function, None, depth + 1, visited);
                for argument in arguments {
                    self.collect_expression(expressions, base, *argument, None, depth + 1, visited);
                }
            }
            ExprKind::Index { operand, arguments } => {
                self.collect_expression(expressions, base, *operand, None, depth + 1, visited);
                for argument in arguments {
                    self.collect_expression(expressions, base, *argument, None, depth + 1, visited);
                }
            }
            ExprKind::Slice {
                operand,
                low,
                high,
                max,
                ..
            } => {
                self.collect_expression(expressions, base, *operand, None, depth + 1, visited);
                for child in low.iter().chain(high.iter()).chain(max.iter()) {
                    self.collect_expression(expressions, base, *child, None, depth + 1, visited);
                }
            }
            ExprKind::TypeAssertion { operand, .. } => {
                self.collect_expression(expressions, base, *operand, None, depth + 1, visited)
            }
            _ => {}
        }
    }

    /// Records a printf-like call: a variadic-looking callable whose name ends
    /// with 'f' and whose leading string-literal argument contains a '%'. The
    /// reference server reads the signature and format parameter from type
    /// information; this bounded approximation covers the standard formatting
    /// helpers without another type query.
    fn record_printf(&mut self, expressions: &[Expr], base: usize, id: ExprId) {
        if self.printf_calls.len() >= MAX_PRINTF_CALLS {
            return;
        }
        let Some(Expr {
            kind:
                ExprKind::Call {
                    function,
                    arguments,
                    ..
                },
            span,
            ..
        }) = expressions.get(id.0)
        else {
            return;
        };
        if arguments.len() < 2 {
            return;
        }
        let Some(name) = callable_name(expressions, *function) else {
            return;
        };
        let name = self
            .source
            .get(base + name.start..base + name.end)
            .unwrap_or("");
        if !name.ends_with('f') {
            return;
        }
        for (index, argument) in arguments.iter().enumerate() {
            let Some(Expr {
                kind: ExprKind::Literal,
                span: literal,
                ..
            }) = expressions.get(argument.0)
            else {
                continue;
            };
            let Some(text) = self.source.get(base + literal.start..base + literal.end) else {
                continue;
            };
            let Some(bytes) = crate::syntax::decode_string_literal(text) else {
                continue;
            };
            if bytes.len() > MAX_FORMAT_BYTES || !bytes.contains(&b'%') {
                continue;
            }
            let Ok(format) = String::from_utf8(bytes) else {
                continue;
            };
            let operations = parse_format(&format, index + 1);
            if operations.is_empty() {
                continue;
            }
            let arguments = arguments
                .iter()
                .filter_map(|argument| {
                    let span = expressions.get(argument.0)?.span.clone();
                    Some(base + span.start..base + span.end)
                })
                .collect();
            self.printf_calls.push(PrintfCall {
                span: base + span.start..base + span.end,
                arguments,
                format_span: base + literal.start..base + literal.end,
                format_index: index,
            });
            return;
        }
    }

    /// Classifies a written composite-literal type spelling conservatively.
    fn shape(&mut self, annotation: &str, depth: usize) -> Option<Shape> {
        if depth > 3 || annotation.len() > MAX_SHAPE_BYTES {
            return None;
        }
        if let Some(shape) = self.shapes.get(annotation) {
            return Some(shape.clone());
        }
        let shape = self.shape_uncached(annotation, depth)?;
        if self.shapes.len() < 128 {
            self.shapes.insert(annotation.to_owned(), shape.clone());
        }
        Some(shape)
    }

    #[allow(clippy::only_used_in_recursion)]
    fn shape_uncached(&mut self, annotation: &str, depth: usize) -> Option<Shape> {
        let form = self
            .workspace
            .type_form(self.file, annotation, &Locals::empty(), 0);
        match form {
            Some(TypeForm::Struct) => Some(Shape::Struct),
            Some(TypeForm::Slice) | Some(TypeForm::Array) => match array_element(annotation) {
                Some(element) => Some(Shape::Sequence(element)),
                None => {
                    let rest = self.underlying_text(annotation)?;
                    (rest != annotation)
                        .then(|| self.shape_uncached(&rest, depth + 1))
                        .flatten()
                }
            },
            Some(TypeForm::Map) => match map_types(annotation) {
                Some((key, value)) => Some(Shape::Map(key, value)),
                None => {
                    let rest = self.underlying_text(annotation)?;
                    (rest != annotation)
                        .then(|| self.shape_uncached(&rest, depth + 1))
                        .flatten()
                }
            },
            Some(TypeForm::Pointer) => {
                let rest = annotation.trim_start().strip_prefix('*')?.trim_start();
                (rest != annotation)
                    .then(|| self.shape_uncached(rest, depth + 1))
                    .flatten()
            }
            Some(_) => Some(Shape::Other),
            None => {
                let rest = self.underlying_text(annotation)?;
                (rest != annotation)
                    .then(|| self.shape_uncached(&rest, depth + 1))
                    .flatten()
            }
        }
    }

    /// The written declaration body of a defined type or alias in this module.
    fn underlying_text(&mut self, annotation: &str) -> Option<String> {
        let (declaration_file, symbol) = self.workspace.type_declaration(self.file, annotation)?;
        let text = if declaration_file == self.file {
            self.source.to_owned()
        } else {
            self.workspace.source(declaration_file)?
        };
        let map = SourceMap::new(&text);
        let end = map.offset(symbol.range.end)?;
        let body = text.get(symbol.end..end)?;
        let parameters = crate::syntax::type_parameter_end(body).unwrap_or(0);
        let mut rest = body.get(parameters..)?.trim_start();
        if let Some(stripped) = rest.strip_prefix('=') {
            rest = stripped.trim_start();
        }
        (!rest.is_empty() && rest.len() <= MAX_SHAPE_BYTES).then(|| rest.to_owned())
    }
}

/// Returns the element type spelling of a slice/array annotation.
fn array_element(annotation: &str) -> Option<String> {
    let trimmed = annotation.trim_start();
    let rest = trimmed.strip_prefix('[')?;
    let mut depth = 1usize;
    for (index, byte) in rest.bytes().enumerate() {
        match byte {
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    let element = rest.get(index + 1..)?.trim();
                    return (!element.is_empty()).then(|| element.to_owned());
                }
            }
            _ => {}
        }
    }
    None
}

/// Returns the key and value type spellings of a map annotation.
fn map_types(annotation: &str) -> Option<(String, String)> {
    let rest = annotation.trim_start().strip_prefix("map")?;
    let rest = rest.trim_start().strip_prefix('[')?;
    let mut depth = 1usize;
    for (index, byte) in rest.bytes().enumerate() {
        match byte {
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    let key = rest.get(..index)?.trim();
                    let value = rest.get(index + 1..)?.trim();
                    return (!key.is_empty() && !value.is_empty())
                        .then(|| (key.to_owned(), value.to_owned()));
                }
            }
            _ => {}
        }
    }
    None
}

fn result_fields(parameters: &[crate::syntax::Parameter], base: usize) -> Vec<ResultField> {
    let mut fields: Vec<ResultField> = Vec::new();
    for parameter in parameters {
        let annotation = base + parameter.annotation.start..base + parameter.annotation.end;
        let name = parameter
            .name
            .as_ref()
            .map(|name| base + name.start..base + name.end);
        if let Some(last) = fields.last_mut()
            && last.annotation == annotation
            && !last.names.is_empty()
        {
            if let Some(name) = name {
                last.span.end = last.span.end.max(name.end);
                last.names.push(name);
            }
            continue;
        }
        let mut field = ResultField {
            names: name.iter().cloned().collect(),
            annotation: annotation.clone(),
            span: annotation.clone(),
        };
        if let Some(name) = &name {
            field.span.start = field.span.start.min(name.start);
            field.span.end = field.span.end.max(name.end);
        }
        fields.push(field);
    }
    fields
}

/// Ranges of a return expression where an identifier cursor keeps control-flow
/// highlighting. Call arguments and key/value pairs disable it.
fn eligible_spans(tree: &BlockTree, id: ExprId, out: &mut Vec<Range<usize>>) {
    let Some(expression) = tree.expressions.get(id.0) else {
        return;
    };
    match &expression.kind {
        ExprKind::KeyValue { .. } | ExprKind::FunctionLiteral { .. } => {}
        ExprKind::Call { function, .. } => {
            eligible_spans(tree, *function, out);
        }
        ExprKind::Name | ExprKind::Literal | ExprKind::Type | ExprKind::Missing => {
            out.push(expression.span.clone());
        }
        ExprKind::Composite {
            literal_type,
            elements,
        } => {
            if let Some(literal_type) = literal_type {
                eligible_spans(tree, *literal_type, out);
            }
            for element in elements {
                eligible_spans(tree, *element, out);
            }
        }
        ExprKind::Unary { operand, .. }
        | ExprKind::Parenthesized {
            expression: operand,
        }
        | ExprKind::TypeSwitchGuard { operand } => eligible_spans(tree, *operand, out),
        ExprKind::Binary { left, right, .. } => {
            eligible_spans(tree, *left, out);
            eligible_spans(tree, *right, out);
        }
        ExprKind::Selector { operand, field } => {
            eligible_spans(tree, *operand, out);
            out.push(field.clone());
        }
        ExprKind::Index { operand, arguments } => {
            eligible_spans(tree, *operand, out);
            for argument in arguments {
                eligible_spans(tree, *argument, out);
            }
        }
        ExprKind::Slice {
            operand,
            low,
            high,
            max,
            ..
        } => {
            eligible_spans(tree, *operand, out);
            for child in low.iter().chain(high.iter()).chain(max.iter()) {
                eligible_spans(tree, *child, out);
            }
        }
        ExprKind::TypeAssertion {
            operand,
            asserted_type,
            ..
        } => {
            eligible_spans(tree, *operand, out);
            out.push(asserted_type.clone());
        }
    }
}

/// The callable's written name span: unwraps parentheses, selectors and generic
/// instantiations to the trailing identifier.
fn callable_name(expressions: &[Expr], id: ExprId) -> Option<Range<usize>> {
    let expression = expressions.get(id.0)?;
    match &expression.kind {
        ExprKind::Name => Some(expression.span.clone()),
        ExprKind::Selector { field, .. } => Some(field.clone()),
        ExprKind::Parenthesized { expression } => callable_name(expressions, *expression),
        ExprKind::Index { operand, .. } => callable_name(expressions, *operand),
        _ => None,
    }
}

/// Printf-family highlights for the cursor identifier. Rows are added before
/// control-flow and identity rows, like the reference server's result map.
fn printf_rows(
    calls: &[PrintfCall],
    source: &str,
    offset: usize,
    rows: &mut Vec<(Range<usize>, HighlightKind)>,
) {
    let mut enclosing: Vec<&PrintfCall> = calls
        .iter()
        .filter(|call| call.span.start <= offset && offset < call.span.end)
        .collect();
    enclosing.sort_by_key(|call| call.span.len());
    for call in enclosing {
        printf_call_rows(call, source, offset, rows);
    }
}

fn printf_call_rows(
    call: &PrintfCall,
    source: &str,
    offset: usize,
    rows: &mut Vec<(Range<usize>, HighlightKind)>,
) {
    let Some(text) = source.get(call.format_span.clone()) else {
        return;
    };
    let Some(bytes) = crate::syntax::decode_string_literal(text) else {
        return;
    };
    let Ok(format) = String::from_utf8(bytes) else {
        return;
    };
    let operations = parse_format(&format, call.format_index + 1);
    if operations.is_empty() {
        return;
    }
    let mut visited: Vec<(Range<usize>, usize)> = Vec::new();
    let mut succeeded = 0usize;
    let mut highlight_pair = |logical: &Range<usize>, argument: usize| {
        let Some(range) = format_range(source, &call.format_span, logical) else {
            return;
        };
        if !visited
            .iter()
            .any(|(known, index)| *known == range && *index == argument)
        {
            visited.push((range.clone(), argument));
        }
        if argument < call.arguments.len()
            && call.arguments[argument].start <= offset
            && offset < call.arguments[argument].end
        {
            push_row(rows, range, HighlightKind::Write);
            push_row(rows, call.arguments[argument].clone(), HighlightKind::Read);
            succeeded = argument;
        }
    };
    for operation in &operations {
        let mut has_asterisk = false;
        if let Some((logical, argument)) = &operation.width {
            has_asterisk = true;
            highlight_pair(logical, *argument);
        }
        if let Some((logical, argument)) = &operation.precision {
            has_asterisk = true;
            highlight_pair(logical, *argument);
        }
        if operation.verb != b'%' {
            let logical = if has_asterisk {
                &operation.verb_range
            } else {
                &operation.range
            };
            highlight_pair(logical, operation.verb_arg);
        }
    }
    for (range, argument) in visited {
        if argument == succeeded {
            push_row(rows, range, HighlightKind::Write);
        }
    }
}

fn push_row(
    rows: &mut Vec<(Range<usize>, HighlightKind)>,
    span: Range<usize>,
    kind: HighlightKind,
) {
    if !rows.iter().any(|(known, _)| *known == span) {
        rows.push((span, kind));
    }
}

/// The source range of a logical byte range inside a string literal.
fn format_range(
    source: &str,
    literal: &Range<usize>,
    logical: &Range<usize>,
) -> Option<Range<usize>> {
    let text = source.get(literal.clone())?;
    if text.starts_with('`') {
        let start = literal.start.checked_add(1 + logical.start)?;
        let end = literal.start.checked_add(1 + logical.end)?;
        return (end <= literal.end).then_some(start..end);
    }
    let content = text.get(1..text.len().checked_sub(1)?)?;
    let start = literal.start + 1 + source_offset_in_content(content, logical.start)?;
    let end = literal.start + 1 + source_offset_in_content(content, logical.end)?;
    Some(start..end)
}

/// Maps a logical byte offset in an interpreted string literal to its raw
/// source byte offset. Escape widths follow Go's string literal grammar.
fn source_offset_in_content(content: &str, logical: usize) -> Option<usize> {
    let bytes = content.as_bytes();
    let mut source_index = 0usize;
    let mut logical_index = 0usize;
    while source_index < bytes.len() {
        if logical_index == logical {
            return Some(source_index);
        }
        if logical_index > logical {
            return None;
        }
        let byte = bytes[source_index];
        if byte == b'\\' {
            let (consumed, produced) = escape_length(&bytes[source_index + 1..])?;
            if logical_index + produced > logical {
                return None;
            }
            source_index += 1 + consumed;
            logical_index += produced;
        } else {
            let length = utf8_length(byte);
            source_index += length;
            logical_index += length;
        }
    }
    (logical_index == logical).then_some(source_index)
}

/// Returns raw bytes after the backslash and decoded bytes produced.
fn escape_length(bytes: &[u8]) -> Option<(usize, usize)> {
    let first = *bytes.first()?;
    match first {
        b'a' | b'b' | b'f' | b'n' | b'r' | b't' | b'v' | b'\\' | b'\'' | b'"' => Some((1, 1)),
        b'x' => Some((3, 1)),
        b'u' => Some((5, 3)),
        b'U' => {
            let digits = bytes.get(1..9)?;
            let value = std::str::from_utf8(digits).ok()?;
            let value = u32::from_str_radix(value, 16).ok()?;
            char::from_u32(value).map(|character| (9, character.len_utf8()))
        }
        b'0'..=b'7' => {
            let mut length = 1;
            while length < 3
                && bytes
                    .get(length)
                    .is_some_and(|byte| (b'0'..=b'7').contains(byte))
            {
                length += 1;
            }
            Some((length, 1))
        }
        _ => None,
    }
}

fn utf8_length(byte: u8) -> usize {
    if byte < 0x80 {
        1
    } else if byte < 0xE0 {
        2
    } else if byte < 0xF0 {
        3
    } else {
        4
    }
}

/// Parses printf format operations with absolute logical byte ranges.
fn parse_format(format: &str, first_arg: usize) -> Vec<FormatOp> {
    let bytes = format.as_bytes();
    let mut operations = Vec::new();
    let mut index = 0usize;
    let mut argument = first_arg;
    while index < bytes.len() {
        if bytes[index] != b'%' {
            index += 1;
            continue;
        }
        if operations.len() >= MAX_FORMAT_OPS {
            return Vec::new();
        }
        let Some((operation, next_argument)) = parse_operation(bytes, index, first_arg, argument)
        else {
            return Vec::new();
        };
        let has_verb = operation.verb != b'%';
        operations.push(operation);
        index = operations
            .last()
            .map_or(index + 1, |operation| operation.range.end);
        if has_verb {
            argument = next_argument.saturating_add(1);
        }
    }
    operations
}

/// Parses one operation starting at the '%' byte; returns the operation and the
/// next argument index.
fn parse_operation(
    format: &[u8],
    start: usize,
    first_arg: usize,
    argument: usize,
) -> Option<(FormatOp, usize)> {
    let mut position = start + 1;
    while position < format.len() && matches!(format[position], b'#' | b'0' | b'+' | b'-' | b' ') {
        position += 1;
    }
    let mut argument = argument;
    let mut pending = false;
    let mut index_position = 0usize;
    if position < format.len() && format[position] == b'[' {
        (position, argument, _) = parse_index(format, position, first_arg)?;
        index_position = position;
        pending = true;
    }
    let (width, position_after_width) =
        parse_size(format, position, argument, false, pending, index_position);
    position = position_after_width;
    if width.is_some() {
        argument += 1;
        pending = false;
    }
    let mut precision = None;
    if position < format.len() && format[position] == b'.' {
        position += 1;
        if position < format.len() && format[position] == b'[' {
            let (after, indexed_argument, _) = parse_index(format, position, first_arg)?;
            index_position = position;
            pending = true;
            argument = indexed_argument;
            position = after;
        }
        let (size, after) = parse_size(format, position, argument, true, pending, index_position);
        position = after;
        if size.is_some() {
            argument += 1;
            pending = false;
        }
        precision = size;
    }
    if !pending && position < format.len() && format[position] == b'[' {
        let (after, indexed_argument, _) = parse_index(format, position, first_arg)?;
        index_position = position;
        argument = indexed_argument;
        position = after;
        pending = true;
    }
    if position >= format.len() {
        return None;
    }
    let verb = format[position];
    if !verb.is_ascii() {
        return None;
    }
    let verb_range = if pending {
        index_position..position + 1
    } else {
        position..position + 1
    };
    let operation = FormatOp {
        range: start..position + 1,
        verb_range,
        verb,
        verb_arg: argument,
        width,
        precision,
    };
    Some((operation, argument))
}

/// Parses an `[n]` argument index; returns the next position and the absolute
/// zero-based call argument index.
fn parse_index(format: &[u8], position: usize, first_arg: usize) -> Option<(usize, usize, bool)> {
    let mut cursor = position + 1;
    let start = cursor;
    while cursor < format.len() && format[cursor].is_ascii_digit() {
        cursor += 1;
    }
    if cursor == start {
        return None;
    }
    let digits = std::str::from_utf8(&format[start..cursor]).ok()?;
    let index: i64 = digits.parse().ok()?;
    if index <= 0 {
        return None;
    }
    if format.get(cursor) != Some(&b']') {
        return None;
    }
    let argument = index
        .checked_sub(1)?
        .checked_add(i64::try_from(first_arg).ok()?)?;
    Some((cursor + 1, usize::try_from(argument).ok()?, true))
}

/// Parses an optional width or precision size specifier.
#[allow(clippy::too_many_arguments)]
fn parse_size(
    format: &[u8],
    position: usize,
    argument: usize,
    precision: bool,
    pending: bool,
    index_position: usize,
) -> (Option<(Range<usize>, usize)>, usize) {
    if format.get(position) == Some(&b'*') {
        let start = if pending { index_position } else { position } - usize::from(precision);
        return (Some((start..position + 1, argument)), position + 1);
    }
    let mut cursor = position;
    while cursor < format.len() && format[cursor].is_ascii_digit() {
        cursor += 1;
    }
    (None, if cursor > position { cursor } else { position })
}

/// Return/result identifier highlights. Control-flow rows precede occurrence
/// rows, so the first range wins like the reference server's result map.
fn control_flow_rows(
    functions: &[FunctionFacts],
    offset: usize,
    rows: &mut Vec<(Range<usize>, HighlightKind)>,
) {
    // Return-statement case: the cursor sits in one of the return expressions.
    if let Some(function) = enclosing_body(functions, offset)
        && let Some(index) = return_index(function, offset)
    {
        let mut marked = vec![false; function.results.len()];
        highlight_result_field(function, index, &mut marked, rows);
        highlight_returns(function, &marked, rows);
        return;
    }
    // Result-parameter case: the cursor sits in the results list.
    let Some(function) = functions
        .iter()
        .find(|function| function.header.contains(&offset) || function.body.contains(&offset))
    else {
        return;
    };
    let Some((field_index, slot, in_name)) = result_cursor(function, offset) else {
        return;
    };
    let field = &function.results[field_index];
    let mut marked = vec![false; function.results.len()];
    if in_name {
        rows.push((field.names[slot].clone(), HighlightKind::Text));
        if slot < marked.len() {
            marked[slot] = true;
        }
    } else {
        rows.push((field.span.clone(), HighlightKind::Text));
        let end = (slot + field.names.len().max(1)).min(marked.len());
        for flag in &mut marked[slot..end] {
            *flag = true;
        }
    }
    highlight_returns(function, &marked, rows);
}

fn enclosing_body(functions: &[FunctionFacts], offset: usize) -> Option<&FunctionFacts> {
    functions
        .iter()
        .filter(|function| function.body.contains(&offset))
        .min_by_key(|function| function.body.len())
}

fn return_index(function: &FunctionFacts, offset: usize) -> Option<usize> {
    for facts in &function.returns {
        for (index, result) in facts.results.iter().enumerate() {
            if result.span.contains(&offset)
                && result.eligible.iter().any(|span| span.contains(&offset))
            {
                return Some(index);
            }
        }
    }
    None
}

fn result_cursor(function: &FunctionFacts, offset: usize) -> Option<(usize, usize, bool)> {
    let mut slot = 0usize;
    for (field_index, field) in function.results.iter().enumerate() {
        if field.names.is_empty() {
            if field.span.contains(&offset) {
                return Some((field_index, slot, false));
            }
            slot += 1;
            continue;
        }
        for (index, name) in field.names.iter().enumerate() {
            if name.contains(&offset) {
                return Some((field_index, slot + index, true));
            }
        }
        if field.span.contains(&offset) {
            return Some((field_index, slot, false));
        }
        slot += field.names.len();
    }
    None
}

/// Marks the result slot range selected by a return expression index.
fn highlight_result_field(
    function: &FunctionFacts,
    index: usize,
    marked: &mut [bool],
    rows: &mut Vec<(Range<usize>, HighlightKind)>,
) {
    let mut slot = 0usize;
    for field in &function.results {
        let end = slot + field.names.len().max(1);
        if index < end {
            if field.names.is_empty() {
                rows.push((field.span.clone(), HighlightKind::Text));
            } else {
                rows.push((field.names[index - slot].clone(), HighlightKind::Text));
            }
            if index < marked.len() {
                marked[index] = true;
            }
            return;
        }
        slot = end;
    }
}

fn highlight_returns(
    function: &FunctionFacts,
    marked: &[bool],
    rows: &mut Vec<(Range<usize>, HighlightKind)>,
) {
    for facts in &function.returns {
        for (index, expression) in facts.results.iter().enumerate() {
            if marked.get(index).copied().unwrap_or(false) {
                rows.push((expression.span.clone(), HighlightKind::Text));
            }
        }
    }
}

/// Finds the top-level assignment offset of a declaration header.
fn top_level_assign(source: &str) -> Option<usize> {
    let mut depth = 0usize;
    for token in Lexer::new(source) {
        match token.kind {
            TokenKind::Assign if depth == 0 => return Some(token.span.start),
            TokenKind::LParen | TokenKind::LBrack | TokenKind::LBrace => depth += 1,
            TokenKind::RParen | TokenKind::RBrack | TokenKind::RBrace => {
                depth = depth.checked_sub(1)?
            }
            _ => {}
        }
    }
    None
}

/// One printf-style format string literal and its directive source ranges.
pub(crate) struct FormatDirective {
    pub(crate) literal: Range<usize>,
    pub(crate) directives: Vec<Range<usize>>,
}

const MAX_FORMAT_LITERALS: usize = 1024;
const MAX_FORMAT_SIGNATURES: usize = 64;

/// Format string directives inside a source snapshot, for semantic token
/// splitting. This mirrors gopls' `formatString`: a variadic call whose last
/// non-variadic parameter receives a literal string containing `%`. Known
/// `...f` helpers keep the token-level fast path; other selector calls resolve
/// one unique variadic declaration from workspace metadata. Directive ranges are
/// raw source byte ranges, matching gopls' `RangeInStringLiteral`.
pub(crate) fn format_directives(
    workspace: &Workspace,
    file: &Path,
    source: &str,
) -> Vec<FormatDirective> {
    let tokens: Vec<_> = Lexer::new(source).collect();
    let mut out = Vec::new();
    let mut index = 0usize;
    let mut signatures: std::collections::HashMap<String, Option<usize>> =
        std::collections::HashMap::new();
    while index + 1 < tokens.len() && out.len() < MAX_FORMAT_LITERALS {
        let open = index;
        if tokens[open].kind != TokenKind::LParen || open == 0 {
            index += 1;
            continue;
        }
        let callee = &tokens[open - 1];
        let name = source.get(callee.span.clone()).unwrap_or("");
        let function_literal = open >= 2 && tokens[open - 2].kind == TokenKind::Func;
        if callee.kind != TokenKind::Ident || function_literal {
            index += 1;
            continue;
        }
        let Some((close, arguments)) = call_arguments(&tokens, open) else {
            index += 1;
            continue;
        };
        let selector = open >= 2 && tokens[open - 2].kind == TokenKind::Period;
        if !name.ends_with('f') {
            // Cheap pre-filter: only calls with a bare `%` string literal can
            // project a format parameter, so most selector calls avoid metadata.
            let literal = arguments.iter().position(|(start, end)| {
                *end == start + 1
                    && tokens[*start].kind == TokenKind::String
                    && source
                        .get(tokens[*start].span.clone())
                        .is_some_and(|text| !text.contains('\n') && text.contains('%'))
            });
            if selector
                && let Some(literal) = literal
                && let Some(format_index) = format_parameter_index(
                    workspace,
                    file,
                    name,
                    &tokens,
                    open,
                    source,
                    &mut signatures,
                )
                && format_index == literal
            {
                let literal = tokens[arguments[format_index].0].span.clone();
                if push_format_literal(source, literal, &mut out) {
                    index = close.max(index + 1);
                    continue;
                }
            }
            index += 1;
            continue;
        }
        if arguments.len() >= 2 {
            for (start, end) in arguments {
                // Only a bare string-literal argument counts, as in gopls.
                if end != start + 1 || tokens[start].kind != TokenKind::String {
                    continue;
                }
                let literal = tokens[start].span.clone();
                if push_format_literal(source, literal, &mut out) {
                    break;
                }
            }
        }
        index = close.max(index + 1);
    }
    out
}

fn push_format_literal(
    source: &str,
    literal: Range<usize>,
    out: &mut Vec<FormatDirective>,
) -> bool {
    let Some(text) = source.get(literal.clone()) else {
        return false;
    };
    // gopls skips multiline strings and non-format literals.
    if text.contains('\n') || text.len() > MAX_FORMAT_BYTES {
        return false;
    }
    let directives = format_literal_directives(source, literal.clone());
    if directives.is_empty() {
        return false;
    }
    out.push(FormatDirective {
        literal,
        directives,
    });
    true
}

/// Index of the format parameter (the last non-variadic parameter) for a
/// selector call whose callee does not end in `f`. The declaration must be the
/// unique variadic match among the receiver's methods, or among the imported
/// package's functions when the qualifier names an import. Ambiguous, missing or
/// unparsable declarations project no format parameter.
fn format_parameter_index(
    workspace: &Workspace,
    file: &Path,
    name: &str,
    tokens: &[crate::syntax::Token],
    open: usize,
    source: &str,
    cache: &mut std::collections::HashMap<String, Option<usize>>,
) -> Option<usize> {
    if cache.len() >= MAX_FORMAT_SIGNATURES {
        return None;
    }
    let receiver = (open >= 3 && tokens[open - 3].kind == TokenKind::Ident)
        .then(|| source.get(tokens[open - 3].span.clone()))
        .flatten();
    let key = format!("{name}\u{0}{}", receiver.unwrap_or(""));
    if let Some(cached) = cache.get(&key) {
        return *cached;
    }
    let resolved = resolve_format_parameter(workspace, file, name, receiver)
        .and_then(|parameters| parameters.checked_sub(2));
    cache.insert(key, resolved);
    resolved
}

fn resolve_format_parameter(
    workspace: &Workspace,
    file: &Path,
    name: &str,
    receiver: Option<&str>,
) -> Option<usize> {
    if let Some(receiver) = receiver
        && scope::universe::imported_name(workspace, file, receiver)
    {
        let mut found = None;
        for (path, symbol) in workspace.imported_symbol_iter(file, receiver) {
            if symbol.receiver.is_some() || symbol.name != name {
                continue;
            }
            if let Some(parameters) = variadic_parameter_count(workspace, path, symbol) {
                if found.is_some() {
                    return None;
                }
                found = Some(parameters);
            }
        }
        return found.filter(|parameters| *parameters >= 2);
    }
    let mut found = None;
    for (path, stub) in &workspace.files {
        for symbol in &stub.symbols {
            if symbol.receiver.is_none() || symbol.name != name {
                continue;
            }
            if let Some(parameters) = variadic_parameter_count(workspace, path, symbol) {
                if found.is_some() {
                    return None;
                }
                found = Some(parameters);
            }
        }
    }
    found.filter(|parameters| *parameters >= 2)
}

fn variadic_parameter_count(workspace: &Workspace, file: &Path, symbol: &Symbol) -> Option<usize> {
    let declaration = symbol.declaration()?;
    let end = symbol
        .body
        .as_ref()
        .map_or(declaration.end, |body| body.start);
    let header = workspace.source_slice(file, declaration.start..end)?;
    let signature = crate::syntax::parse_signature(&header);
    if !signature.diagnostics.is_empty()
        || !signature
            .parameters
            .last()
            .is_some_and(|parameter| parameter.variadic)
    {
        return None;
    }
    Some(signature.parameters.len())
}

/// Top-level argument token ranges of one call, starting at its `(`.
fn call_arguments(
    tokens: &[crate::syntax::Token],
    open: usize,
) -> Option<(usize, Vec<(usize, usize)>)> {
    let mut depth = 0usize;
    let mut arguments = Vec::new();
    let mut start = open + 1;
    let mut index = open + 1;
    while index < tokens.len() {
        match tokens[index].kind {
            TokenKind::LParen | TokenKind::LBrack | TokenKind::LBrace => depth += 1,
            TokenKind::RParen if depth == 0 => {
                if start < index {
                    arguments.push((start, index));
                }
                return Some((index, arguments));
            }
            TokenKind::RParen | TokenKind::RBrack | TokenKind::RBrace => {
                depth = depth.checked_sub(1)?;
            }
            TokenKind::Comma if depth == 0 => {
                if start < index {
                    arguments.push((start, index));
                }
                start = index + 1;
            }
            _ => {}
        }
        index += 1;
    }
    None
}

/// Raw source ranges of the directives in one string literal.
fn format_literal_directives(source: &str, literal: Range<usize>) -> Vec<Range<usize>> {
    let Some(text) = source.get(literal.clone()) else {
        return Vec::new();
    };
    let Some(bytes) = crate::syntax::decode_string_literal(text) else {
        return Vec::new();
    };
    if bytes.len() > MAX_FORMAT_BYTES || !bytes.contains(&b'%') {
        return Vec::new();
    }
    let Ok(format) = String::from_utf8(bytes) else {
        return Vec::new();
    };
    parse_format(&format, 1)
        .iter()
        .filter(|operation| operation.verb != b'%')
        .filter_map(|operation| format_range(source, &literal, &operation.range))
        .collect()
}
