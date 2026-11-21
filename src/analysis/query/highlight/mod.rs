//! Request-local document highlights resolved by symbol identity.
//!
//! Occurrences are matched by resolved identity, never by spelling alone. The
//! read/write classification follows checked region facts plus a bounded
//! query-local walk of checked function bodies for declaration and assignment
//! write positions. Return/result control-flow highlights mirror the reference
//! server's identifier behavior. No source, syntax tree or identity survives the
//! request; bodies are parsed at most once inside region checking and only spans
//! are retained.
mod format;
use super::navigation;
use super::{Context, SourceRange};
use crate::analysis::sema::{
    MemberDeclId, MethodOrigin, RegionView, SemanticObject, SemanticSession, SymbolOrigin, VarKind,
};
use crate::syntax::{BlockTree, ExprId, ExprKind, Lexer, Parameter, StmtKind, TokenKind};
use crate::text::SourceMap;
use crate::workspace::Workspace;
pub(crate) use format::collect_region_format_directives;
use std::ops::Range;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

const MAX_HIGHLIGHTS: usize = 4096;
const MAX_FUNCTIONS: usize = 512;
const MAX_STATEMENTS: usize = 16_384;
const MAX_NESTED_SCOPES: usize = 64;
const MAX_FORMAT_BYTES: usize = 16 * 1024;
const MAX_PRINTF_CALLS: usize = 1024;
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Variable,
    Text,
}

#[derive(PartialEq, Eq)]
enum Target<'a> {
    Object(SemanticObject<'a>),
    Package(&'a str),
    Singleton,
    Unresolved(&'a str),
}

struct Candidate<'a> {
    span: Range<usize>,
    after_dot: bool,
    is_import_alias: bool,
    is_package_clause: bool,
    obj: Option<SemanticObject<'a>>,
    def_site: bool,
    embedded_struct_field: bool,
    in_writes: bool,
    in_declarations: bool,
    is_type_switch_guard: bool,
    in_sig_name: bool,
}

impl<'a> Context<'a> {
    /// Highlight rows in file order, or `None` for the protocol's null result.
    pub(crate) fn document_highlights(
        &mut self,
        offset: usize,
        cancellation: Option<&AtomicBool>,
    ) -> Result<Option<Vec<HighlightRow>>, (i32, String)> {
        let (doc, sema) = self.split();
        let workspace = doc.workspace();
        let file = doc.file();
        let source = doc.source();
        let Some(selection) = identifier_at(source, offset) else {
            return Ok(format_highlights(workspace, sema, file, source, offset));
        };
        let Some(stub) = workspace.files.get(file) else {
            return Ok(None);
        };
        let Some(interned_file) = sema.intern_file(file) else {
            return Ok(None);
        };
        let trace = workspace.trace.queries.then(std::time::Instant::now);
        let spelling = &source[selection.clone()];
        let scan = lexical_scan(source, spelling);
        if scan.candidates.len() > MAX_HIGHLIGHTS {
            return Ok(None);
        }
        let Some(cursor_idx) = scan
            .candidates
            .iter()
            .position(|(span, _)| *span == selection)
        else {
            return Ok(format_highlights(workspace, sema, file, source, offset));
        };
        crate::error::check_cancelled(cancellation)?;

        let mut import_aliases = Vec::new();
        crate::syntax::visit_import_spans(source, |spec| {
            if let Some(alias) = spec.alias
                && matches!(&source[alias.clone()], name if name != "." && name != "_")
            {
                import_aliases.push(alias);
            }
        });

        let mut candidates: Vec<Candidate<'_>> = scan
            .candidates
            .into_iter()
            .map(|(span, after_dot)| Candidate {
                is_import_alias: import_aliases.contains(&span),
                is_package_clause: scan.package_name.as_ref() == Some(&span),
                span,
                after_dot,
                obj: None,
                def_site: false,
                embedded_struct_field: false,
                in_writes: false,
                in_declarations: false,
                is_type_switch_guard: false,
                in_sig_name: false,
            })
            .collect();

        for symbol in &stub.symbols {
            if (symbol.is_var() || symbol.is_const())
                && let Ok(index) = candidates.binary_search_by_key(&symbol.start, |c| c.span.start)
                && candidates[index].span.end == symbol.end
            {
                candidates[index].in_declarations = true;
                candidates[index].in_writes = true;
            }
        }

        let map = SourceMap::new(source);
        let mut functions = Vec::new();
        let mut collector = BodyCollector {
            source,
            statements: 0,
            functions: &mut functions,
            nested: 0,
            exhausted: false,
        };

        let mut aborted = false;
        let region_spans: Vec<[u32; 2]> = candidates
            .iter()
            .map(|c| [c.span.start as u32, c.span.end as u32])
            .collect();

        sema.visit_matching_file_regions(
            interned_file,
            source,
            |[rs, re]| {
                ((rs as usize) <= offset && offset <= (re as usize))
                    || has_span_in(&region_spans, rs, re)
            },
            |view| {
                if aborted || collector.exhausted {
                    return;
                }
                if cancellation.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
                    aborted = true;
                    return;
                }
                let r_span = view.span();
                let lo = candidates.partition_point(|c| c.span.start < r_span[0] as usize);
                let hi = candidates.partition_point(|c| c.span.end <= r_span[1] as usize);
                let slice = &mut candidates[lo..hi];
                collector.visit_region(view, offset, slice);
                for candidate in slice {
                    resolve_region_candidate(
                        workspace,
                        interned_file,
                        source,
                        &map,
                        view,
                        candidate,
                    );
                }
            },
        );
        crate::error::check_cancelled(cancellation)?;
        if aborted || collector.exhausted {
            return Ok(None);
        }

        let imported = sema.imported_name(interned_file, spelling);
        let (target, class) = cursor_target(&candidates[cursor_idx], spelling, imported);

        if workspace.trace.queries {
            eprintln!(
                "highlight cursor {:?} target {}",
                spelling,
                target_tag(&target)
            );
        }

        let mut rows: Vec<(Range<usize>, HighlightKind)> = Vec::new();
        let printf_calls = format::calls_at(workspace, sema, file, source, offset);
        printf_rows(&printf_calls, source, offset, &mut rows);
        control_flow_rows(&functions, offset, &mut rows);
        if let Target::Package(alias) = &target
            && let Some(span) = import_spec_span(workspace, source, file, alias)
        {
            rows.push((span, HighlightKind::Text));
        }

        for candidate in &candidates {
            crate::error::check_cancelled(cancellation)?;
            if !candidate_matches(&target, candidate, selection.clone(), spelling, imported) {
                continue;
            }
            let kind = highlight_kind(candidate, &target, class);
            push_row(&mut rows, candidate.span.clone(), kind);
        }

        if rows.is_empty() {
            return Ok(None);
        }
        if let Some(start) = trace {
            eprintln!(
                "highlight timing: candidates {}; rows {}; total {:?}",
                candidates.len(),
                rows.len(),
                start.elapsed()
            );
        }
        Ok(finish_rows(&map, rows))
    }
}

struct LexicalScan {
    package_name: Option<Range<usize>>,
    candidates: Vec<(Range<usize>, bool)>,
}

fn lexical_scan(source: &str, spelling: &str) -> LexicalScan {
    let mut package_name = None;
    let mut after_package = false;
    let mut candidates = Vec::new();
    let mut prev_dot = false;
    for token in Lexer::new(source) {
        if token.kind == TokenKind::Package && package_name.is_none() {
            after_package = true;
            prev_dot = false;
            continue;
        }
        if after_package {
            after_package = false;
            if token.kind == TokenKind::Ident {
                package_name = Some(token.span.clone());
            }
        }
        if token.kind == TokenKind::Ident && &source[token.span.clone()] == spelling {
            candidates.push((token.span, prev_dot));
        }
        prev_dot = token.kind == TokenKind::Period;
    }
    LexicalScan {
        package_name,
        candidates,
    }
}

fn identifier_at(source: &str, offset: usize) -> Option<Range<usize>> {
    let mut found = None;
    for token in Lexer::new(source) {
        if token.span.start > offset {
            break;
        }
        if token.kind == TokenKind::Ident && offset <= token.span.end {
            found = Some(token.span);
        }
    }
    found
}

fn has_span_in(spans: &[[u32; 2]], start: u32, end: u32) -> bool {
    let idx = spans.partition_point(|s| s[0] < start);
    spans.get(idx).is_some_and(|s| s[1] <= end)
}

fn resolve_region_candidate<'a>(
    workspace: &'a Workspace,
    file: &'a Path,
    source: &str,
    map: &SourceMap<'_>,
    view: &RegionView<'_, 'a>,
    candidate: &mut Candidate<'a>,
) {
    let key = [candidate.span.start as u32, candidate.span.end as u32];
    if let Some(member) =
        navigation::member_declaration_target(workspace, file, source, &candidate.span, map)
        && member.embedded
    {
        candidate.obj = Some(SemanticObject::Field(MemberDeclId {
            file,
            owner: Some(member.owner),
            selection: key,
            embedded: true,
            builtin: false,
        }));
        candidate.def_site = true;
        candidate.embedded_struct_field = true;
        return;
    }
    if let Some(def) = view.def_by_span(key) {
        candidate.obj = Some(def);
        candidate.def_site = true;
    } else if let Some(use_obj) = view.use_or_selection(key) {
        candidate.obj = Some(use_obj);
        candidate.def_site = false;
    }
}

fn cursor_target<'a>(
    cursor: &Candidate<'a>,
    spelling: &'a str,
    imported: bool,
) -> (Target<'a>, Class) {
    if cursor.is_type_switch_guard {
        return (Target::Unresolved(spelling), Class::Text);
    }
    if let Some(obj) = cursor.obj {
        if matches!(obj, SemanticObject::PkgName { .. }) {
            return (Target::Package(spelling), Class::Text);
        }
        return (Target::Object(obj), object_class(obj));
    }
    if cursor.is_import_alias && !cursor.is_package_clause && imported {
        return (Target::Package(spelling), Class::Text);
    }
    if spelling == "_" && cursor.in_declarations {
        return (Target::Singleton, Class::Text);
    }
    (Target::Unresolved(spelling), Class::Text)
}

fn object_class(obj: SemanticObject<'_>) -> Class {
    match obj {
        SemanticObject::Var { .. } => Class::Variable,
        SemanticObject::Field(member) => {
            if member
                .owner
                .and_then(|owner| owner.type_info.as_ref())
                .is_some_and(|info| info.is_interface())
            {
                Class::Text
            } else {
                Class::Variable
            }
        }
        _ => Class::Text,
    }
}

fn target_tag(target: &Target<'_>) -> &'static str {
    match target {
        Target::Object(obj) => match obj {
            SemanticObject::Var {
                origin: SymbolOrigin::Local { .. },
                case_scope: Some(_),
                ..
            } => "case",
            SemanticObject::Var {
                origin: SymbolOrigin::Local { .. },
                ..
            }
            | SemanticObject::Const(SymbolOrigin::Local { .. })
            | SemanticObject::TypeName {
                origin: SymbolOrigin::Local { .. },
                ..
            }
            | SemanticObject::TypeParam { .. }
            | SemanticObject::Label { .. } => "local",
            SemanticObject::Field(_) => "field",
            SemanticObject::Method(MethodOrigin::Interface(m)) if m.builtin => "builtin-method",
            SemanticObject::Method(MethodOrigin::Interface(_)) => "interface-method",
            SemanticObject::Func(_)
            | SemanticObject::Method(MethodOrigin::Concrete(_))
            | SemanticObject::Var { .. }
            | SemanticObject::Const(_)
            | SemanticObject::TypeName { .. } => "global",
            SemanticObject::PkgName { .. } => "package",
            SemanticObject::BuiltinFunction(_)
            | SemanticObject::BuiltinType(_)
            | SemanticObject::BuiltinConst(_)
            | SemanticObject::Nil => "builtin",
        },
        Target::Package(_) => "package",
        Target::Singleton => "singleton",
        Target::Unresolved(_) => "unresolved",
    }
}

fn candidate_matches<'a>(
    target: &Target<'a>,
    candidate: &Candidate<'a>,
    cursor_span: Range<usize>,
    spelling: &str,
    imported: bool,
) -> bool {
    match target {
        Target::Singleton => candidate.span == cursor_span,
        Target::Package(_) => {
            !candidate.is_package_clause
                && imported
                && (candidate.is_import_alias
                    || matches!(candidate.obj, Some(SemanticObject::PkgName { .. })))
        }
        Target::Object(target_obj) => candidate
            .obj
            .is_some_and(|other| target_obj.same_reference_target(other, true)),
        Target::Unresolved(_) => {
            if candidate.is_type_switch_guard {
                return true;
            }
            if candidate.obj.is_some()
                || candidate.is_import_alias
                || candidate.after_dot
                || candidate.in_sig_name
                || (spelling == "_" && candidate.in_declarations)
            {
                return false;
            }
            if imported && !candidate.is_package_clause {
                return false;
            }
            true
        }
    }
}

fn highlight_kind(candidate: &Candidate<'_>, target: &Target<'_>, class: Class) -> HighlightKind {
    if candidate.in_writes {
        return HighlightKind::Write;
    }
    if candidate.embedded_struct_field {
        return match target {
            Target::Object(SemanticObject::Field(_)) => HighlightKind::Read,
            _ => HighlightKind::Text,
        };
    }
    if matches!(candidate.obj, Some(SemanticObject::Field(_)))
        && !candidate.def_site
        && !candidate.after_dot
    {
        return HighlightKind::Write;
    }
    if candidate.is_import_alias {
        return HighlightKind::Text;
    }
    if candidate.def_site
        && matches!(
            candidate.obj,
            Some(
                SemanticObject::Var {
                    kind: VarKind::Param | VarKind::ResultVar | VarKind::Receiver,
                    ..
                } | SemanticObject::Field(_)
                    | SemanticObject::Method(MethodOrigin::Interface(_))
                    | SemanticObject::PkgName { .. }
            )
        )
    {
        return HighlightKind::Text;
    }
    match class {
        Class::Variable => HighlightKind::Read,
        Class::Text => HighlightKind::Text,
    }
}

/// Literal/operator cursors do not have identifier identities. Their
/// format associations still come from the same bounded signature scan.
fn format_highlights(
    workspace: &Workspace,
    sema: &mut SemanticSession<'_>,
    file: &Path,
    source: &str,
    offset: usize,
) -> Option<Vec<HighlightRow>> {
    let calls = format::calls_at(workspace, sema, file, source, offset);
    let mut rows = Vec::new();
    printf_rows(&calls, source, offset, &mut rows);
    finish_rows(&SourceMap::new(source), rows)
}

fn finish_rows(
    map: &SourceMap<'_>,
    rows: Vec<(Range<usize>, HighlightKind)>,
) -> Option<Vec<HighlightRow>> {
    if rows.is_empty() {
        return None;
    }
    let mut result: Vec<_> = rows
        .into_iter()
        .map(|(span, kind)| HighlightRow {
            range: SourceRange::new(map, span),
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
    Some(result)
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

struct BodyCollector<'s, 'f> {
    source: &'s str,
    statements: usize,
    functions: &'f mut Vec<FunctionFacts>,
    nested: usize,
    exhausted: bool,
}

impl<'s, 'f> BodyCollector<'s, 'f> {
    fn visit_region(
        &mut self,
        view: &RegionView<'_, '_>,
        offset: usize,
        slice: &mut [Candidate<'_>],
    ) {
        let Some((sig_base, sig)) = view.syntax_signature() else {
            return;
        };
        if !slice.is_empty() {
            for param in sig
                .receiver
                .iter()
                .chain(&sig.parameters)
                .chain(&sig.results)
            {
                if let Some(name) = &param.name {
                    let span = sig_base + name.start..sig_base + name.end;
                    if let Ok(idx) = slice.binary_search_by_key(&span.start, |c| c.span.start)
                        && slice[idx].span == span
                    {
                        slice[idx].in_sig_name = true;
                    }
                }
            }
        }
        let Some((body_base, tree)) = view.syntax_body() else {
            return;
        };
        let header = sig_base..body_base;
        let body_end = view.span()[1] as usize;
        let body_range = body_base..body_end;
        if body_range.len() > crate::analysis::budget::MAX_BODY_BYTES {
            self.exhausted = true;
            return;
        }
        let collect_returns = header.contains(&offset) || body_range.contains(&offset);
        if self.functions.len() >= MAX_FUNCTIONS {
            if collect_returns {
                self.exhausted = true;
            }
            return;
        }
        let mut facts = FunctionFacts {
            body: body_range,
            header,
            results: if collect_returns && sig.diagnostics.is_empty() {
                result_fields(&sig.results, sig_base)
            } else {
                Vec::new()
            },
            returns: Vec::new(),
        };
        self.walk_block(tree, body_base, collect_returns, &mut facts, slice);
        self.collect_literals_in(tree, body_base, offset, slice);
        if collect_returns {
            self.functions.push(facts);
        }
    }

    fn visit_literal(
        &mut self,
        sig_range: Range<usize>,
        body_range: Range<usize>,
        offset: usize,
        slice: &mut [Candidate<'_>],
    ) {
        if self.exhausted {
            return;
        }
        if body_range.len() > crate::analysis::budget::MAX_BODY_BYTES {
            self.exhausted = true;
            return;
        }
        if self.functions.len() >= MAX_FUNCTIONS {
            self.exhausted = true;
            return;
        }
        self.nested += 1;
        if self.nested > MAX_NESTED_SCOPES {
            self.exhausted = true;
            return;
        }
        let Some(text) = self.source.get(body_range.clone()) else {
            self.nested -= 1;
            return;
        };
        let tree = crate::syntax::parse_block(text);
        if !tree.diagnostics.is_empty() {
            self.nested -= 1;
            return;
        }
        let collect_returns = sig_range.contains(&offset) || body_range.contains(&offset);
        let mut results = Vec::new();
        if let Some(header_text) = self.source.get(sig_range.clone()) {
            let parsed = crate::syntax::parse_literal_signature(header_text);
            if parsed.diagnostics.is_empty() {
                if !slice.is_empty() {
                    for param in parsed.parameters.iter().chain(&parsed.results) {
                        if let Some(name) = &param.name {
                            let span = sig_range.start + name.start..sig_range.start + name.end;
                            if let Ok(idx) =
                                slice.binary_search_by_key(&span.start, |c| c.span.start)
                                && slice[idx].span == span
                            {
                                slice[idx].in_sig_name = true;
                            }
                        }
                    }
                }
                if collect_returns {
                    results = result_fields(&parsed.results, sig_range.start);
                }
            }
        }
        let mut facts = FunctionFacts {
            body: body_range.clone(),
            header: sig_range,
            results,
            returns: Vec::new(),
        };
        self.walk_block(&tree, body_range.start, collect_returns, &mut facts, slice);
        self.collect_literals_in(&tree, body_range.start, offset, slice);
        if collect_returns {
            self.functions.push(facts);
        }
        self.nested -= 1;
    }

    fn collect_literals_in(
        &mut self,
        tree: &BlockTree,
        base: usize,
        offset: usize,
        slice: &mut [Candidate<'_>],
    ) {
        for expr in &tree.expressions {
            if let ExprKind::FunctionLiteral { signature, body } = &expr.kind {
                let sig_r = base + signature.start..base + signature.end;
                let body_r = base + body.start..base + body.end;
                let lo = slice.partition_point(|c| c.span.start < sig_r.start);
                let hi = slice.partition_point(|c| c.span.end <= body_r.end);
                if lo < hi || sig_r.contains(&offset) || body_r.contains(&offset) {
                    self.visit_literal(sig_r, body_r, offset, &mut slice[lo..hi]);
                }
            }
        }
    }

    fn walk_block(
        &mut self,
        tree: &BlockTree,
        base: usize,
        collect_returns: bool,
        facts: &mut FunctionFacts,
        slice: &mut [Candidate<'_>],
    ) {
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
                        self.mark_write(tree, base, *expression, slice);
                    }
                    if self.source.get(base + operator.start..base + operator.end) == Some(":=") {
                        for expression in left {
                            self.mark_declaration(tree, base, *expression, slice);
                        }
                    }
                }
                StmtKind::Increment { expression, .. } => {
                    self.mark_write(tree, base, *expression, slice);
                }
                StmtKind::Send { channel, .. } => {
                    self.mark_write(tree, base, *channel, slice);
                }
                StmtKind::Declaration { keyword, names, .. } => {
                    if matches!(
                        self.source.get(base + keyword.start..base + keyword.end),
                        Some("var" | "const")
                    ) {
                        for name in names {
                            let span = base + name.start..base + name.end;
                            if let Ok(idx) =
                                slice.binary_search_by_key(&span.start, |c| c.span.start)
                                && slice[idx].span == span
                            {
                                slice[idx].in_writes = true;
                                slice[idx].in_declarations = true;
                            }
                        }
                    }
                }
                StmtKind::DeclarationGroup { declarations } => {
                    pending.extend(declarations.iter().copied());
                }
                StmtKind::Return { values } if collect_returns => {
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
                        self.mark_write(tree, base, *expression, slice);
                        self.mark_declaration(tree, base, *expression, slice);
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
                    if let Some(guard_stmt) = tree.statements.get(guard.0)
                        && let StmtKind::Assignment { left, operator, .. } = &guard_stmt.kind
                        && self.source.get(base + operator.start..base + operator.end) == Some(":=")
                    {
                        for expr_id in left {
                            if let Some(expr) = tree.expressions.get(expr_id.0)
                                && matches!(expr.kind, ExprKind::Name)
                            {
                                let span = base + expr.span.start..base + expr.span.end;
                                if let Ok(idx) =
                                    slice.binary_search_by_key(&span.start, |c| c.span.start)
                                    && slice[idx].span == span
                                {
                                    slice[idx].is_type_switch_guard = true;
                                }
                            }
                        }
                    }
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

    fn mark_write(&self, tree: &BlockTree, base: usize, id: ExprId, slice: &mut [Candidate<'_>]) {
        let Some(expression) = tree.expressions.get(id.0) else {
            return;
        };
        match &expression.kind {
            ExprKind::Name => {
                let span = base + expression.span.start..base + expression.span.end;
                if let Ok(idx) = slice.binary_search_by_key(&span.start, |c| c.span.start)
                    && slice[idx].span == span
                {
                    slice[idx].in_writes = true;
                }
            }
            ExprKind::Selector { field, .. } => {
                let span = base + field.start..base + field.end;
                if let Ok(idx) = slice.binary_search_by_key(&span.start, |c| c.span.start)
                    && slice[idx].span == span
                {
                    slice[idx].in_writes = true;
                }
            }
            ExprKind::Unary { operator, operand } => {
                if self.source.get(operator.start..operator.end) == Some("*") {
                    self.mark_write(tree, base, *operand, slice);
                }
            }
            ExprKind::Parenthesized { expression } => {
                self.mark_write(tree, base, *expression, slice);
            }
            _ => {}
        }
    }

    fn mark_declaration(
        &self,
        tree: &BlockTree,
        base: usize,
        id: ExprId,
        slice: &mut [Candidate<'_>],
    ) {
        let Some(expression) = tree.expressions.get(id.0) else {
            return;
        };
        if matches!(expression.kind, ExprKind::Name) {
            let span = base + expression.span.start..base + expression.span.end;
            if let Ok(idx) = slice.binary_search_by_key(&span.start, |c| c.span.start)
                && slice[idx].span == span
            {
                slice[idx].in_declarations = true;
            }
        }
    }
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

fn result_fields(parameters: &[Parameter], base: usize) -> Vec<ResultField> {
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
        ExprKind::Name | ExprKind::Literal(_) | ExprKind::Type | ExprKind::Missing => {
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

/// Format-operation and argument highlights for a cursor. Rows are added before
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
        let actual = call.arguments.get(argument);
        if range.contains(&offset)
            || actual.is_some_and(|span| span.start <= offset && offset <= span.end)
        {
            push_row(rows, range, HighlightKind::Write);
            if let Some(actual) = actual {
                push_row(rows, actual.clone(), HighlightKind::Read);
                succeeded = argument;
            }
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
        b'u' | b'U' => {
            let width = if first == b'u' { 4 } else { 8 };
            let digits = bytes.get(1..1 + width)?;
            let value = std::str::from_utf8(digits).ok()?;
            let value = u32::from_str_radix(value, 16).ok()?;
            char::from_u32(value).map(|character| (1 + width, character.len_utf8()))
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
        index_position = position;
        (position, argument, _) = parse_index(format, position, first_arg)?;
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
    if let Some(function) = enclosing_body(functions, offset)
        && let Some(index) = return_index(function, offset)
    {
        let mut marked = vec![false; function.results.len()];
        highlight_result_field(function, index, &mut marked, rows);
        highlight_returns(function, &marked, rows);
        return;
    }
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
