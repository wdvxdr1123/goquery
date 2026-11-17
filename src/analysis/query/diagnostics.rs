//! Compiler-grade file diagnostics computed from one admitted source snapshot.
//!
//! The gopls baseline is the go/types and go/parser diagnostic set with source
//! `compiler` or `syntax`. Every check here is conservative: a name is only
//! reported when its failure is provable from the request-local source and
//! workspace metadata, so valid files never produce false positives. Unsupported
//! constructs stay unreported; the remaining gaps are listed in the design doc.
use super::Context;
use crate::analysis::check::diagnostics::MAX_DIAGNOSTICS;
pub(crate) use crate::analysis::check::diagnostics::{Code, Diagnostic, Kind};
use crate::syntax::{Lexer, TokenKind};
use crate::text::SourceMap;
use crate::workspace::Workspace;
use std::collections::HashSet;
use std::ops::Range;

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
    if !workspace.files.contains_key(file) {
        return Vec::new();
    }
    let syntax = syntax_diagnostics(source);
    if !syntax.is_empty() {
        // A package that cannot be parsed is not type-checked, so go/types
        // compiler diagnostics must not be invented from the partial source.
        return syntax;
    }
    let mut rows = Vec::new();
    unused_import_diagnostics(workspace, source, &mut rows);
    context.with_sema(|sema| {
        sema.file_diagnostics(file, source, &mut rows);
    });
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
