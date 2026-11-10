//! Compiler-grade file diagnostics computed from one admitted source snapshot.
//!
//! The gopls baseline is the go/types and go/parser diagnostic set with source
//! `compiler` or `syntax`. Every check here is conservative: a name is only
//! reported when its failure is provable from the request-local source and
//! workspace metadata, so valid files never produce false positives. Unsupported
//! constructs stay unreported; the remaining gaps are listed in the design doc.
use super::Context;
use crate::analysis::scope::DeclarationKind;
use crate::analysis::scope::Locals;
use crate::analysis::scope::universe;
use crate::analysis::source::{Role, SourceAnalysis};
use crate::index::IndexedFile;
use crate::syntax::{Lexer, TokenKind};
use crate::text::SourceMap;
use crate::workspace::Workspace;
use std::collections::HashSet;
use std::ops::Range;
use std::path::Path;

/// Push diagnostics are bounded; the parser already truncates at this limit.
pub(crate) const MAX_DIAGNOSTICS: usize = 4096;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Code {
    UndeclaredName,
    UndeclaredImportedName,
    UnusedVar,
    UnusedImport,
    DuplicateDecl,
    DuplicateMethod,
}

impl Code {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::UndeclaredName => "UndeclaredName",
            Self::UndeclaredImportedName => "UndeclaredImportedName",
            Self::UnusedVar => "UnusedVar",
            Self::UnusedImport => "UnusedImport",
            Self::DuplicateDecl => "DuplicateDecl",
            Self::DuplicateMethod => "DuplicateMethod",
        }
    }
}

pub(crate) enum Kind {
    Compiler(Code),
    Syntax,
}

/// A same-file related location, as reported by go/types for redeclarations.
pub(crate) struct Related {
    pub(crate) span: Range<usize>,
    pub(crate) message: String,
}

pub(crate) struct Diagnostic {
    pub(crate) span: Range<usize>,
    pub(crate) kind: Kind,
    pub(crate) message: String,
    pub(crate) unnecessary: bool,
    pub(crate) related: Vec<Related>,
}

/// One provably unused import, with the deletion gopls applies for
/// `source.organizeImports` when exactly one import must be removed.
pub(crate) struct UnusedImport {
    pub(crate) span: Range<usize>,
    pub(crate) message: String,
    pub(crate) removal: Option<Range<usize>>,
}

struct ImportSpec {
    /// Decoded import path.
    path: String,
    /// Explicit alias token, including `_` and `.`.
    alias: Option<String>,
    /// Span of the quoted import path literal.
    span: Range<usize>,
    /// Whole `import ...` declaration, excluded from identifier scans.
    declaration: Range<usize>,
    /// Byte offset just after the group's opening parenthesis.
    group_start: Option<usize>,
    /// End of the previous spec's path literal in the same declaration.
    previous_end: Option<usize>,
}

/// All diagnostics for the admitted file, or an empty list when the workspace
/// has no metadata for it.
pub(crate) fn diagnostics(context: &Context<'_>) -> Vec<Diagnostic> {
    let workspace = context.workspace();
    let file = context.file();
    let source = context.source();
    let Some(stub) = workspace.files.get(file) else {
        return Vec::new();
    };
    let syntax = syntax_diagnostics(source);
    if !syntax.is_empty() {
        // A package that cannot be parsed is not type-checked, so go/types
        // compiler diagnostics must not be invented from the partial source.
        return syntax;
    }
    let mut rows = Vec::new();
    unused_import_diagnostics(workspace, source, &mut rows);
    duplicate_declarations(file, stub, source, &mut rows);
    if rows.len() < MAX_DIAGNOSTICS
        && let Ok(analysis) = SourceAnalysis::for_semantic(source, stub)
    {
        semantic_diagnostics(workspace, file, stub, source, &analysis, &mut rows);
    }
    rows.truncate(MAX_DIAGNOSTICS);
    rows
}

/// Provably unused imports of the admitted file, for organize-import actions.
pub(crate) fn unused_imports(context: &Context<'_>) -> Vec<UnusedImport> {
    let source = context.source();
    if !syntax_diagnostics(source).is_empty() {
        return Vec::new();
    }
    if !context.workspace().files.contains_key(context.file()) {
        return Vec::new();
    }
    unused_imports_of(context.workspace(), source)
}

fn syntax_diagnostics(source: &str) -> Vec<Diagnostic> {
    let mut diagnostics: Vec<Diagnostic> = crate::syntax::validate_file(source)
        .into_iter()
        .take(MAX_DIAGNOSTICS)
        .map(|error| {
            let (offset, message) = syntax_position(source, &error);
            Diagnostic {
                span: offset..offset,
                kind: Kind::Syntax,
                message,
                unnecessary: false,
                related: Vec::new(),
            }
        })
        .collect();
    // gopls publishes only the first parse error of a file (go/parser's
    // AllErrors list is position-sorted); recovery errors after it are noise.
    if let Some(first) = diagnostics
        .iter_mut()
        .min_by_key(|diagnostic| diagnostic.span.start)
    {
        let span = first.span.clone();
        let message = std::mem::take(&mut first.message);
        return vec![Diagnostic {
            span,
            kind: Kind::Syntax,
            message,
            unnecessary: false,
            related: Vec::new(),
        }];
    }
    diagnostics
}

/// Aligns gosyntax's compact errors with go/parser's `AllErrors` vocabulary.
/// That is what gopls parses with: a missing expression at a statement keyword
/// becomes "expected ';', found X" while a missing expression at a closing
/// delimiter becomes "expected operand, found X".
fn syntax_position(source: &str, error: &crate::syntax::Diagnostic) -> (usize, String) {
    match error.message {
        "if requires condition" => (
            next_token_start(source, error.span.end).unwrap_or(error.span.end),
            "missing condition in if statement".to_owned(),
        ),
        "expected expression" => {
            let (offset, kind, found) = token_at(source, error.span.start);
            let missing = if matches!(
                kind,
                TokenKind::RBrace | TokenKind::RParen | TokenKind::RBrack | TokenKind::Eof
            ) {
                "expected operand"
            } else {
                "expected ';'"
            };
            (offset, format!("{missing}, found {found}"))
        }
        "expected declaration separator" | "expected statement separator" => {
            let (offset, _, found) = token_at(source, error.span.start.max(error.span.end));
            (offset, format!("expected ';', found {found}"))
        }
        "expected ')'" | "expected ']'" | "expected '}'" => {
            let delimiter = error.message.trim_start_matches("expected ");
            let (offset, _, found) = token_at(source, error.span.start);
            (offset, format!("expected {delimiter}, found {found}"))
        }
        _ => (error.span.start, error.message.to_owned()),
    }
}

fn next_token_start(source: &str, offset: usize) -> Option<usize> {
    Lexer::new(source.get(offset..)?)
        .next()
        .map(|token| offset + token.span.start)
}

/// Token at `offset`, with the go/parser "found" spelling.
fn token_at(source: &str, offset: usize) -> (usize, TokenKind, String) {
    let Some(rest) = source.get(offset..) else {
        return (offset, TokenKind::Eof, "EOF".to_owned());
    };
    let Some(token) = Lexer::new(rest).next() else {
        return (offset, TokenKind::Eof, "EOF".to_owned());
    };
    let start = offset + token.span.start;
    if token.kind == TokenKind::Semicolon && token.span.start == token.span.end {
        // go/parser names an automatic semicolon by its source form.
        return (start, TokenKind::Semicolon, "newline".to_owned());
    }
    (start, token.kind, found_text(source, start, &token))
}

/// Renders the go/parser "found" name for the token at `offset`.
fn found_text(source: &str, offset: usize, token: &crate::syntax::Token) -> String {
    if let Some(text) = token.kind.fixed_text() {
        return format!("'{text}'");
    }
    let start = offset + token.span.start;
    let end = offset + token.span.end;
    source
        .get(start..end)
        .map_or_else(|| "EOF".to_owned(), |text| format!("'{text}'"))
}

fn unused_import_diagnostics(workspace: &Workspace, source: &str, rows: &mut Vec<Diagnostic>) {
    for import in unused_imports_of(workspace, source) {
        rows.push(Diagnostic {
            span: import.span,
            kind: Kind::Compiler(Code::UnusedImport),
            message: import.message,
            unnecessary: true,
            related: Vec::new(),
        });
    }
}

fn unused_imports_of(workspace: &Workspace, source: &str) -> Vec<UnusedImport> {
    let specs = import_specs(source);
    if specs.is_empty() || specs.len() > 4096 {
        return Vec::new();
    }
    // A `C` import turns the file into cgo input whose names are resolved by the
    // cgo tool, and a dot import contributes names we cannot enumerate. In both
    // cases a "not used" claim is not provable from this source alone.
    if specs
        .iter()
        .any(|spec| spec.path == "C" || spec.alias.as_deref() == Some("."))
    {
        return Vec::new();
    }
    let used = used_qualifiers(source, &specs);
    let mut candidates = Vec::new();
    for spec in &specs {
        let name = match spec.alias.as_deref() {
            Some("_") | Some(".") => continue,
            Some(alias) => alias.to_owned(),
            None => match workspace.declared_package_name(&spec.path) {
                // An unindexed dependency's declared name is unknown, so its use
                // cannot be disproved.
                None => continue,
                Some(name) => name.to_owned(),
            },
        };
        if used.contains(&name) {
            continue;
        }
        let message = match spec.alias.as_deref() {
            Some(alias) => format!("\"{}\" imported as {} and not used", spec.path, alias),
            None => format!("\"{}\" imported and not used", spec.path),
        };
        candidates.push(UnusedImport {
            span: spec.span.clone(),
            message,
            removal: None,
        });
    }
    if candidates.len() == 1 {
        let spec = specs
            .iter()
            .find(|spec| candidates[0].span == spec.span)
            .expect("candidate came from specs");
        candidates[0].removal = removal_span(source, spec);
    }
    // Several removals need gopls' minimized multi-edit; diagnostics stay, but
    // this module declines to guess the edit shape.
    candidates
}

fn used_qualifiers(source: &str, specs: &[ImportSpec]) -> HashSet<String> {
    let mut used = HashSet::new();
    let mut previous: Option<(TokenKind, Range<usize>)> = None;
    for token in Lexer::new(source) {
        if token.kind == TokenKind::Period
            && let Some((kind, span)) = &previous
            && *kind == TokenKind::Ident
            && !specs
                .iter()
                .any(|spec| spec.declaration.contains(&span.start))
        {
            used.insert(source[span.clone()].to_owned());
        }
        previous = Some((token.kind, token.span));
    }
    used
}

/// Deletes the import spec the way gopls' formatted diff does when it is the
/// only removal: a later group member loses the newline before it, a first
/// group member loses the newline after the opening parenthesis, and a lone
/// import declaration loses its whole line plus one preceding blank line.
fn removal_span(source: &str, spec: &ImportSpec) -> Option<Range<usize>> {
    match (spec.group_start, spec.previous_end) {
        (Some(_), Some(previous_end)) if previous_end < spec.span.start => {
            Some(previous_end..spec.span.end)
        }
        (Some(group_start), None) => Some(group_start..spec.span.end),
        (Some(_), _) => None,
        (None, _) => {
            let start_of_line = line_start(source, spec.declaration.start);
            let end_of_line = next_line_start(source, spec.span.end)?;
            let start = if start_of_line == 0 {
                0
            } else {
                let previous_line = line_start(source, start_of_line - 1);
                if source
                    .get(previous_line..start_of_line - 1)
                    .is_some_and(|line| line.trim().is_empty())
                {
                    previous_line
                } else {
                    start_of_line
                }
            };
            Some(start..end_of_line)
        }
    }
}

/// Deletion range for the `Remove variable` quickfix gopls offers for a local
/// declaration that occupies exactly one line. Both `name := value` and
/// `var name type` shapes qualify; multi-name declarations and statements that
/// continue past the line are left alone because gopls rewrites those
/// differently.
pub(crate) fn variable_removal(context: &Context<'_>, span: &Range<usize>) -> Option<Range<usize>> {
    let source = context.source();
    let map = SourceMap::new(source);
    let mut tokens = Lexer::new(source);
    let mut previous: Option<crate::syntax::Token> = None;
    let mut before_previous: Option<crate::syntax::Token> = None;
    while let Some(token) = tokens.next() {
        if token.kind != TokenKind::Ident || token.span != *span {
            before_previous = previous.replace(token);
            continue;
        }
        let following = tokens.next();
        let starts_short_declaration = following
            .as_ref()
            .is_some_and(|next| next.kind == TokenKind::Define);
        let starts_var_declaration = previous
            .as_ref()
            .is_some_and(|keyword| keyword.kind == TokenKind::Var)
            && before_previous
                .as_ref()
                .is_some_and(|start| statement_boundary(start.kind));
        let mut single_name = true;
        let keyword_start = if starts_short_declaration {
            span.start
        } else if starts_var_declaration {
            if following
                .as_ref()
                .is_some_and(|next| next.kind == TokenKind::Comma)
            {
                single_name = false;
            }
            previous
                .as_ref()
                .map_or(span.start, |keyword| keyword.span.start)
        } else {
            return None;
        };
        if !single_name {
            return None;
        }
        let start_of_line = line_start(source, keyword_start);
        if !source
            .get(start_of_line..keyword_start)
            .is_some_and(|prefix| prefix.trim().is_empty())
        {
            return None;
        }
        let line = map.position(span.start).line;
        let mut statement_end = None;
        for inner in tokens.by_ref() {
            match inner.kind {
                TokenKind::Semicolon => {
                    statement_end = Some(inner.span.start);
                    break;
                }
                TokenKind::Comma => single_name = false,
                _ => {}
            }
        }
        if !single_name {
            return None;
        }
        let statement_end = statement_end?;
        if map.position(statement_end).line != line {
            return None;
        }
        let end_of_line = source
            .get(statement_end..)
            .and_then(|rest| rest.find('\n').map(|newline| statement_end + newline))?;
        if !source
            .get(statement_end..end_of_line)
            .is_some_and(|tail| tail.trim().is_empty())
        {
            return None;
        }
        return Some(start_of_line..end_of_line + 1);
    }
    None
}

fn line_start(source: &str, offset: usize) -> usize {
    source
        .get(..offset)
        .and_then(|prefix| prefix.rfind('\n').map(|newline| newline + 1))
        .unwrap_or(0)
}

fn next_line_start(source: &str, offset: usize) -> Option<usize> {
    source
        .get(offset..)
        .and_then(|rest| rest.find('\n').map(|newline| offset + newline + 1))
}

/// Collects import declarations without retaining a whole-file token vector.
fn import_specs(source: &str) -> Vec<ImportSpec> {
    let mut specs = Vec::new();
    let mut tokens = Lexer::new(source).peekable();
    while let Some(token) = tokens.next() {
        match token.kind {
            TokenKind::Import => {
                parse_import(&mut tokens, source, token.span.start, &mut specs);
            }
            TokenKind::Func | TokenKind::Type | TokenKind::Var | TokenKind::Const => break,
            _ => {}
        }
    }
    specs
}

fn parse_import(
    tokens: &mut std::iter::Peekable<crate::syntax::Lexer<'_>>,
    source: &str,
    declaration_start: usize,
    specs: &mut Vec<ImportSpec>,
) {
    let mut first = Vec::new();
    let grouped = tokens
        .peek()
        .is_some_and(|next| next.kind == TokenKind::LParen);
    let group_start = if grouped {
        tokens.next().map(|open| open.span.end)
    } else {
        None
    };
    let mut declaration_end = declaration_start;
    let mut previous_end = None;
    loop {
        let mut alias = None;
        while let Some(next) = tokens.peek() {
            if !matches!(next.kind, TokenKind::Ident | TokenKind::Period) {
                break;
            }
            let alias_token = tokens.next().expect("peeked alias");
            alias = Some(source[alias_token.span.clone()].to_owned());
        }
        let Some(path_token) = tokens.next() else {
            break;
        };
        if path_token.kind != TokenKind::String {
            break;
        }
        let Some(path) = crate::syntax::decode_import_path(&source[path_token.span.clone()]) else {
            break;
        };
        let span = path_token.span.clone();
        declaration_end = span.end;
        first.push(ImportSpec {
            path,
            alias,
            span: span.clone(),
            declaration: 0..0,
            group_start,
            previous_end,
        });
        previous_end = Some(span.end);
        if !grouped {
            break;
        }
        match tokens.next() {
            Some(next) if next.kind == TokenKind::RParen => {
                declaration_end = next.span.end;
                break;
            }
            Some(next) if next.kind == TokenKind::Semicolon => {}
            Some(_) | None => break,
        }
    }
    let declaration = declaration_start..declaration_end;
    for spec in &mut first {
        spec.declaration = declaration.clone();
    }
    specs.append(&mut first);
}

fn duplicate_declarations(
    file: &Path,
    stub: &IndexedFile,
    source: &str,
    rows: &mut Vec<Diagnostic>,
) {
    let mut declarations: Vec<&crate::index::IndexedSymbol> = stub
        .symbols
        .iter()
        .filter(|symbol| symbol.receiver.is_none() && symbol.name != "_" && symbol.name != "init")
        .collect();
    declarations.sort_unstable_by_key(|symbol| symbol.start);
    let mut grouped: Vec<Vec<&crate::index::IndexedSymbol>> = Vec::new();
    for symbol in declarations {
        match grouped
            .iter_mut()
            .find(|group| group.first().is_some_and(|first| first.name == symbol.name))
        {
            Some(group) => group.push(symbol),
            None => grouped.push(vec![symbol]),
        }
    }
    for group in grouped.iter().filter(|group| group.len() > 1) {
        let Some(first) = group.first().copied() else {
            continue;
        };
        for later in &group[1..] {
            rows.push(Diagnostic {
                span: later.start..later.end,
                kind: Kind::Compiler(Code::DuplicateDecl),
                message: format!("{} redeclared in this block", first.name),
                unnecessary: false,
                related: vec![Related {
                    span: first.start..first.end,
                    message: format!("other declaration of {}", first.name),
                }],
            });
            rows.push(Diagnostic {
                span: first.start..first.end,
                kind: Kind::Compiler(Code::DuplicateDecl),
                message: format!(
                    "{} redeclared in this block (this error: other declaration of {})",
                    first.name, first.name
                ),
                unnecessary: false,
                related: vec![Related {
                    span: later.start..later.end,
                    message: String::new(),
                }],
            });
            if rows.len() >= MAX_DIAGNOSTICS {
                return;
            }
        }
    }
    duplicate_methods(file, stub, &SourceMap::new(source), rows);
}

fn duplicate_methods(
    file: &Path,
    stub: &IndexedFile,
    map: &SourceMap<'_>,
    rows: &mut Vec<Diagnostic>,
) {
    let mut methods: Vec<&crate::index::IndexedSymbol> = stub
        .symbols
        .iter()
        .filter(|symbol| symbol.receiver.is_some() && symbol.name != "_")
        .collect();
    methods.sort_unstable_by_key(|symbol| symbol.start);
    for (index, method) in methods.iter().enumerate() {
        let Some(receiver) = method.receiver.as_deref().and_then(method_owner) else {
            continue;
        };
        let Some(first) = methods[..index].iter().rev().find(|earlier| {
            earlier.name == method.name
                && earlier
                    .receiver
                    .as_deref()
                    .and_then(method_owner)
                    .is_some_and(|owner| owner == receiver)
        }) else {
            continue;
        };
        let position = map.position(first.start);
        rows.push(Diagnostic {
            span: method.start..method.end,
            kind: Kind::Compiler(Code::DuplicateMethod),
            message: format!(
                "method {}.{} already declared at {}:{}:{}",
                receiver,
                method.name,
                file.display(),
                position.line + 1,
                position.character + 1
            ),
            unnecessary: false,
            related: Vec::new(),
        });
        if rows.len() >= MAX_DIAGNOSTICS {
            return;
        }
    }
}

/// Method identity is the declared base type name. Pointerness does not separate
/// method sets, so `T` and `*T` share one declaration namespace.
fn method_owner(receiver: &str) -> Option<&str> {
    let receiver = receiver.strip_prefix('*').unwrap_or(receiver);
    if receiver.is_empty()
        || receiver
            .chars()
            .any(|ch| !crate::syntax::is_identifier_start(ch) && !ch.is_numeric())
    {
        return None;
    }
    Some(receiver)
}

fn semantic_diagnostics(
    workspace: &Workspace,
    file: &Path,
    stub: &IndexedFile,
    source: &str,
    analysis: &SourceAnalysis,
    rows: &mut Vec<Diagnostic>,
) {
    let local_variables = local_variable_declarations(analysis, source);
    let writes = write_targets(source);
    let mut used: HashSet<usize> = HashSet::new();
    let mut declared_names: HashSet<&str> = HashSet::new();
    for name in &analysis.names {
        let spelling = &source[name.span.clone()];
        let Some(binding) = analysis.locals.definition_at(&name.span, spelling) else {
            continue;
        };
        if binding.selection == name.span {
            declared_names.insert(binding.name.as_str());
            continue;
        }
        if !writes.contains(&name.span.start) {
            used.insert(binding.selection.start);
        }
    }
    for variable in &local_variables {
        if used.contains(&variable.selection.start) {
            continue;
        }
        rows.push(Diagnostic {
            span: variable.selection.clone(),
            kind: Kind::Compiler(Code::UnusedVar),
            message: variable.message(),
            unnecessary: true,
            related: Vec::new(),
        });
        if rows.len() >= MAX_DIAGNOSTICS {
            return;
        }
    }
    undefined_names(
        workspace,
        file,
        stub,
        source,
        analysis,
        &declared_names,
        rows,
    );
}

struct LocalVariable {
    name: String,
    selection: Range<usize>,
    switch_case: bool,
}

impl LocalVariable {
    fn message(&self) -> String {
        if self.switch_case {
            // go/types reports implicit type-switch variables without a colon.
            format!("{} declared and not used", self.name)
        } else {
            format!("declared and not used: {}", self.name)
        }
    }
}

fn local_variable_declarations(analysis: &SourceAnalysis, source: &str) -> Vec<LocalVariable> {
    let mut variables = Vec::new();
    for name in &analysis.names {
        if !matches!(&name.role, Role::Name) {
            continue;
        }
        let spelling = &source[name.span.clone()];
        if spelling == "_" {
            continue;
        }
        let Some(binding) = analysis.locals.definition_at(&name.span, spelling) else {
            continue;
        };
        if binding.selection != name.span || binding.declaration != DeclarationKind::Value {
            continue;
        }
        variables.push(LocalVariable {
            name: binding.name.clone(),
            selection: binding.selection.clone(),
            switch_case: analysis.locals.switch_case(binding).is_some(),
        });
        if variables.len() >= MAX_DIAGNOSTICS {
            break;
        }
    }
    variables
}

/// Names in write-only positions: the bare identifier list to the left of `=`
/// or `:=`. Compound assignments, increments, selectors, indexes and
/// dereferences read their operand and therefore count as uses.
fn write_targets(source: &str) -> HashSet<usize> {
    let mut targets = HashSet::new();
    let mut candidates: Vec<usize> = Vec::new();
    let mut expect_ident = false;
    let mut previous = TokenKind::Illegal;
    for token in Lexer::new(source) {
        match token.kind {
            TokenKind::Ident => {
                if candidates.is_empty() {
                    if statement_boundary(previous) {
                        candidates.push(token.span.start);
                    }
                } else if expect_ident && previous == TokenKind::Comma {
                    candidates.push(token.span.start);
                    expect_ident = false;
                } else {
                    candidates.clear();
                }
            }
            TokenKind::Comma => {
                if candidates.is_empty() || previous != TokenKind::Ident {
                    candidates.clear();
                    expect_ident = false;
                } else {
                    expect_ident = true;
                }
            }
            TokenKind::Assign | TokenKind::Define => {
                if !candidates.is_empty() && !expect_ident {
                    for start in candidates.drain(..) {
                        targets.insert(start);
                    }
                } else {
                    candidates.clear();
                }
                expect_ident = false;
            }
            _ => {
                candidates.clear();
                expect_ident = false;
            }
        }
        previous = token.kind;
    }
    targets
}

fn statement_boundary(previous: TokenKind) -> bool {
    matches!(
        previous,
        TokenKind::Semicolon
            | TokenKind::LBrace
            | TokenKind::RBrace
            | TokenKind::Colon
            | TokenKind::For
            | TokenKind::If
            | TokenKind::Switch
            | TokenKind::Case
            | TokenKind::Default
            | TokenKind::Else
    )
}

fn undefined_names(
    workspace: &Workspace,
    file: &Path,
    stub: &IndexedFile,
    source: &str,
    analysis: &SourceAnalysis,
    declared_names: &HashSet<&str>,
    rows: &mut Vec<Diagnostic>,
) {
    // Dot imports and cgo preambles contribute names that this query cannot
    // enumerate; any "undefined" claim in those files would be a guess.
    if stub
        .imports
        .iter()
        .any(|import| import.is_dot() || import.path == "C")
    {
        return;
    }
    let mut type_context = type_context_names(source);
    type_context.extend(anonymous_struct_literal_keys(source));
    let mut reported = 0;
    let mut reported_spans: HashSet<usize> = HashSet::new();
    for name in &analysis.names {
        if rows.len() >= MAX_DIAGNOSTICS || reported >= MAX_DIAGNOSTICS / 2 {
            return;
        }
        if type_context.contains(&name.span.start) {
            continue;
        }
        let spelling = &source[name.span.clone()];
        if spelling == "_" {
            continue;
        }
        let missing = match &name.role {
            Role::Name | Role::Type(None) | Role::EmbeddedType(None) => {
                let query = UnqualifiedQuery {
                    workspace,
                    file,
                    stub,
                    locals: &analysis.locals,
                    declared_names,
                    type_position: matches!(
                        &name.role,
                        Role::Type(None) | Role::EmbeddedType(None)
                    ),
                };
                (!unqualified_defined(&query, &name.span, spelling))
                    .then(|| (Code::UndeclaredName, format!("undefined: {spelling}")))
            }
            Role::Member(Some(qualifier))
            | Role::Type(Some(qualifier))
            | Role::EmbeddedType(Some(qualifier)) => {
                let qualifier_text = &source[qualifier.clone()];
                qualified_member_missing(
                    workspace,
                    file,
                    stub,
                    analysis,
                    qualifier,
                    qualifier_text,
                    spelling,
                )
                .then(|| {
                    (
                        Code::UndeclaredImportedName,
                        format!("undefined: {qualifier_text}.{spelling}"),
                    )
                })
            }
            _ => None,
        };
        let Some((code, message)) = missing else {
            continue;
        };
        reported_spans.insert(name.span.start);
        rows.push(Diagnostic {
            span: name.span.clone(),
            kind: Kind::Compiler(code),
            message,
            unnecessary: false,
            related: Vec::new(),
        });
        reported += 1;
    }
    // Selector references inside declaration headers (struct field and embedded
    // annotations, interface method signatures) are not always annotated by the
    // expression classifier, so recover `qualifier.member` token pairs directly.
    // The same proof gates apply, so unannotated cases stay conservative.
    for (qualifier, member) in selector_pairs(source) {
        if rows.len() >= MAX_DIAGNOSTICS || reported >= MAX_DIAGNOSTICS / 2 {
            return;
        }
        if reported_spans.contains(&member.start) {
            continue;
        }
        let member_text = &source[member.clone()];
        let qualifier_text = &source[qualifier.clone()];
        if member_text == "_"
            || qualifier_text == "_"
            || !qualified_member_missing(
                workspace,
                file,
                stub,
                analysis,
                &qualifier,
                qualifier_text,
                member_text,
            )
        {
            continue;
        }
        rows.push(Diagnostic {
            span: member.clone(),
            kind: Kind::Compiler(Code::UndeclaredImportedName),
            message: format!("undefined: {qualifier_text}.{member_text}"),
            unnecessary: false,
            related: Vec::new(),
        });
        reported += 1;
    }
}

/// Bare `qualifier.member` token pairs: the qualifier is not itself a selector
/// member (`a.b.c` yields only `a.b`) and both sides are identifiers.
fn selector_pairs(source: &str) -> Vec<(Range<usize>, Range<usize>)> {
    let mut pairs = Vec::new();
    let mut tokens = Lexer::new(source).peekable();
    let mut previous: Option<(TokenKind, Range<usize>)> = None;
    let mut before_previous: Option<(TokenKind, Range<usize>)> = None;
    while let Some(token) = tokens.next() {
        if token.kind == TokenKind::Period
            && let Some((TokenKind::Ident, qualifier)) = &previous
            && tokens
                .peek()
                .is_some_and(|next| next.kind == TokenKind::Ident)
        {
            let member = tokens.next().expect("peeked identifier");
            let bare = before_previous
                .as_ref()
                .is_none_or(|(kind, _)| *kind != TokenKind::Period);
            if bare && pairs.len() < MAX_DIAGNOSTICS {
                pairs.push((qualifier.clone(), member.span.clone()));
            }
            before_previous = Some((TokenKind::Period, token.span));
            previous = Some((TokenKind::Ident, member.span));
            continue;
        }
        before_previous = previous.replace((token.kind, token.span));
    }
    pairs
}

/// Query-local inputs for unqualified-name resolution.
struct UnqualifiedQuery<'a> {
    workspace: &'a Workspace,
    file: &'a Path,
    stub: &'a IndexedFile,
    locals: &'a Locals,
    declared_names: &'a HashSet<&'a str>,
    type_position: bool,
}

/// Whether an unqualified occurrence resolves in the file or its package.
///
/// Value and type positions differ only in the predeclared set: expressions may
/// name builtin functions/constants, while type positions may name predeclared
/// types. Local type parameters and local type declarations resolve through the
/// lexical bindings; undecidable files (dot imports, cgo) are rejected earlier.
fn unqualified_defined(query: &UnqualifiedQuery<'_>, span: &Range<usize>, spelling: &str) -> bool {
    let predeclared = if query.type_position {
        universe::builtin_type(spelling)
    } else {
        universe::builtin(spelling)
    };
    query.declared_names.contains(spelling)
        || predeclared
        || query.locals.definition_at(span, spelling).is_some()
        || query.workspace.unqualified_symbol(query.file, spelling).is_some()
        // A declaration of this exact name proves it defined even when the
        // package-level lookup refuses duplicates or method names.
        || query
            .stub
            .symbols
            .iter()
            .any(|symbol| symbol.start == span.start && symbol.end == span.end)
        || universe::imported_name(query.workspace, query.file, spelling)
}

/// Whether `qualifier.member` provably misses a declaration in an indexed
/// imported package. Only a unique, unshadowed import alias whose package
/// metadata is loaded can prove absence; every other case is unknown and stays
/// unreported. Unexported members are excluded because go/types reports a
/// different message ("cannot refer to unexported name") for them.
fn qualified_member_missing(
    workspace: &Workspace,
    file: &Path,
    stub: &IndexedFile,
    analysis: &SourceAnalysis,
    qualifier: &Range<usize>,
    qualifier_text: &str,
    member: &str,
) -> bool {
    if qualifier_text == "_"
        || member == "_"
        || !crate::syntax::is_exported_name(member)
        || analysis
            .locals
            .resolve_at(qualifier.start, qualifier_text)
            .is_some()
        || workspace.unqualified_symbol(file, qualifier_text).is_some()
        || !universe::imported_name(workspace, file, qualifier_text)
    {
        return false;
    }
    let mut paths = stub
        .imports
        .iter()
        .filter(|import| !import.is_dot() && !import.is_blank())
        .filter(|import| workspace.import_name(import) == Some(qualifier_text))
        .map(|import| import.path.as_str());
    let Some(path) = paths.next() else {
        return false;
    };
    // An ambiguous alias is invalid Go and cannot prove a unique package.
    if paths.next().is_some() || workspace.declared_package_name(path).is_none() {
        return false;
    }
    !workspace
        .imported_symbol_iter(file, qualifier_text)
        .any(|(_, symbol)| symbol.name == member)
}

/// Identifier offsets lexically inside type literals. Expression classification
/// covers ordinary annotations, but nested types inside assertions and
/// conversions (for example `conn.(interface{ File() (*os.File, error) })`) are
/// not annotated, so their method/field/parameter names would otherwise look
/// like unresolved expression references. Reporting no diagnostic is the
/// conservative fallback: a genuine miss stays unreported.
fn type_context_names(source: &str) -> HashSet<usize> {
    let mut names = HashSet::new();
    let mut depth = 0usize;
    let mut type_bodies: Vec<usize> = Vec::new();
    let mut tokens = Lexer::new(source).peekable();
    while let Some(token) = tokens.next() {
        match token.kind {
            TokenKind::Struct | TokenKind::Interface => {
                if tokens
                    .peek()
                    .is_some_and(|next| next.kind == TokenKind::LBrace)
                {
                    tokens.next();
                    depth += 1;
                    type_bodies.push(depth);
                }
            }
            TokenKind::Func => {
                if tokens
                    .peek()
                    .is_some_and(|next| next.kind == TokenKind::LParen)
                {
                    tokens.next();
                    let mut parens = 1usize;
                    let mut parameters = Vec::new();
                    for inner in tokens.by_ref() {
                        match inner.kind {
                            TokenKind::LParen => parens += 1,
                            TokenKind::RParen => {
                                parens -= 1;
                                if parens == 0 {
                                    break;
                                }
                            }
                            TokenKind::Ident => parameters.push(inner.span.start),
                            _ => {}
                        }
                    }
                    // A following block makes this a function literal whose
                    // parameters are real lexical bindings.
                    if !tokens
                        .peek()
                        .is_some_and(|next| next.kind == TokenKind::LBrace)
                    {
                        names.extend(parameters);
                    }
                }
            }
            TokenKind::LBrace => depth += 1,
            TokenKind::RBrace => {
                if type_bodies.last() == Some(&depth) {
                    type_bodies.pop();
                }
                depth = depth.saturating_sub(1);
            }
            TokenKind::Ident if !type_bodies.is_empty() => {
                names.insert(token.span.start);
            }
            _ => {}
        }
    }
    names
}

/// Field keys of composite literals whose type is an anonymous struct. The
/// expression classifier resolves keys only for named/selector/index literal
/// types, so these would otherwise look like unresolved expression references.
fn anonymous_struct_literal_keys(source: &str) -> HashSet<usize> {
    let mut keys = HashSet::new();
    let mut tokens = Lexer::new(source).peekable();
    while let Some(token) = tokens.next() {
        if token.kind != TokenKind::Struct {
            continue;
        }
        // Find the closing brace of the struct type expression.
        let mut depth = 0usize;
        let mut closed = false;
        for inner in tokens.by_ref() {
            match inner.kind {
                TokenKind::LBrace => depth += 1,
                TokenKind::RBrace => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        closed = true;
                        break;
                    }
                }
                _ => {}
            }
        }
        if !closed
            || !tokens
                .peek()
                .is_some_and(|next| next.kind == TokenKind::LBrace)
        {
            continue;
        }
        tokens.next();
        let mut body = 1usize;
        let mut pending_key = None;
        for inner in tokens.by_ref() {
            match inner.kind {
                TokenKind::LBrace => {
                    body += 1;
                    pending_key = None;
                }
                TokenKind::RBrace => {
                    body = body.saturating_sub(1);
                    if body == 0 {
                        break;
                    }
                    pending_key = None;
                }
                TokenKind::Ident => pending_key = Some(inner.span.start),
                TokenKind::Colon => {
                    if let Some(start) = pending_key.take() {
                        keys.insert(start);
                    }
                }
                _ => pending_key = None,
            }
        }
    }
    keys
}
