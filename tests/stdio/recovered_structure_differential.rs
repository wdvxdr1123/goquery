use crate::go_corpus::{GoCorpus, MismatchBucket, OracleBinary};
use serde_json::Value;
use std::collections::BTreeMap;
use std::ops::Range;
use std::path::PathBuf;

/// Deterministic caps, mirrored in the gate documentation.
const MAX_REJECTED_SOURCES: usize = 64;
const MUTATION_TARGET: usize = 200;
const MAX_MUTATION_SOURCES: usize = 256;
const MAX_SEED_BYTES: usize = 48 * 1024;
const MAX_SOURCE_BYTES: usize = 128 * 1024;
const MAX_NAME_BYTES: usize = 256;
const MAX_DECLARATIONS: usize = 512;
const MAX_STATEMENTS: usize = 512;
const ORACLE_CHUNK: usize = 32;
const ALIGNMENT_WINDOW: usize = 32;
/// Expression projection caps (mirrored in the oracle and documentation).
const MAX_EXPRESSIONS_PER_STATEMENT: usize = 256;
const MAX_SOURCE_EXPRESSIONS: usize = 4096;

#[derive(Default)]
struct Stats {
    packages: usize,
    files: usize,
    parser_accepted: usize,
    parser_rejected: usize,
    rejected_sources: usize,
    rejected_skipped: usize,
    mutation_sources: usize,
    mutation_skipped: usize,
    declarations_compared: usize,
    bodies_compared: usize,
    statements_compared: usize,
    truncated_sources: usize,
    oracle_bailed_sources: usize,
    diagnostic_sources: usize,
    expression_nodes: usize,
    initializers_compared: usize,
    /// Recovered-structure differences explained by go/parser's own error
    /// recovery (synthetic names, mis-nesting, swallowed declarations).
    /// They are counted and reported, never silently dropped.
    informational: MismatchBucket,
}

impl Stats {
    fn count_info(&mut self, class: &str, count: usize) {
        *self
            .informational
            .classes
            .entry(class.to_owned())
            .or_default() += count;
    }
}

struct SourceCase {
    label: String,
    source: String,
}

#[derive(Clone, Debug, PartialEq)]
struct RecoveredExpression {
    slot: usize,
    depth: usize,
    kind: String,
    name: String,
    start: usize,
    end: usize,
}

#[derive(Clone, Debug, PartialEq)]
struct RecoveredStatement {
    kind: String,
    start: usize,
    end: usize,
    expressions: Vec<RecoveredExpression>,
}

#[derive(Clone, Debug)]
struct RecoveredInitializer {
    start: usize,
    end: usize,
    expressions: Vec<RecoveredExpression>,
}

#[derive(Clone, Debug)]
struct RecoveredDeclaration {
    kind: String,
    name: String,
    start: usize,
    end: usize,
    name_start: usize,
    name_end: usize,
    body: Option<Range<usize>>,
    statements: Vec<RecoveredStatement>,
}

struct RecoveredComparison<'a> {
    label: &'a str,
    source: &'a str,
    oracle_accepted: bool,
    expected: &'a [RecoveredDeclaration],
    actual: &'a [RecoveredDeclaration],
    expected_bodies: Vec<Range<usize>>,
    actual_bodies: Vec<Range<usize>>,
    bad_declarations: &'a [Range<usize>],
}

#[test]
#[ignore = "prepare the pinned Go source corpus and set GOQUERY_GO_SOURCE"]
fn go_source_recovered_structure_corpus() {
    let corpus = GoCorpus::load();
    let mutation_target = std::env::var("GOQUERY_RECOVERED_LIMIT")
        .ok()
        .map(|value| {
            value
                .parse::<usize>()
                .expect("GOQUERY_RECOVERED_LIMIT")
                .min(MAX_MUTATION_SOURCES)
        })
        .unwrap_or(MUTATION_TARGET);
    let oracle = OracleBinary::build("recovered");
    let mut stats = Stats::default();
    let mut mismatches = MismatchBucket::default();
    let mut acceptance: BTreeMap<PathBuf, bool> = BTreeMap::new();
    let mut rejected: Vec<SourceCase> = Vec::new();
    for (package, mut files) in corpus.packages.clone() {
        if !corpus.package_enabled(&package, true, None) {
            continue;
        }
        files.sort();
        let reports = oracle.run("--negative-files", &files);
        assert_eq!(reports.len(), files.len(), "oracle must inspect every file");
        for (report, file) in reports.iter().zip(&files) {
            let accepted = report["accepted"]
                .as_bool()
                .expect("oracle acceptance bool");
            acceptance.insert(file.clone(), accepted);
            if accepted {
                stats.parser_accepted += 1;
            } else {
                stats.parser_rejected += 1;
                if rejected.len() >= MAX_REJECTED_SOURCES {
                    continue;
                }
                let bytes = std::fs::read(file).expect("read rejected corpus file");
                if bytes.len() > MAX_SOURCE_BYTES {
                    stats.rejected_skipped += 1;
                    continue;
                }
                match String::from_utf8(bytes) {
                    Ok(source) => rejected.push(SourceCase {
                        label: file.display().to_string(),
                        source,
                    }),
                    Err(_) => stats.rejected_skipped += 1,
                }
            }
        }
        stats.packages += 1;
    }
    stats.files = acceptance.len();
    stats.rejected_sources = rejected.len();

    let eligible: Vec<PathBuf> = acceptance
        .iter()
        .filter(|(file, accepted)| {
            **accepted
                && std::fs::metadata(file)
                    .is_ok_and(|meta| (32..=MAX_SEED_BYTES as u64).contains(&meta.len()))
        })
        .map(|(file, _)| file.clone())
        .collect();
    let stride = (eligible.len() / mutation_target.max(1)).max(1);
    let mut mutations: Vec<SourceCase> = Vec::new();
    for (seed_index, file) in eligible.iter().step_by(stride).enumerate() {
        if mutations.len() >= mutation_target {
            break;
        }
        let Ok(source) = std::fs::read_to_string(file) else {
            stats.mutation_skipped += 1;
            continue;
        };
        let tokens: Vec<Range<usize>> = goquery::syntax::Lexer::new(&source)
            .filter(|token| !token.inserted && token.span.start < token.span.end)
            .map(|token| token.span)
            .collect();
        if tokens.len() < 4 {
            stats.mutation_skipped += 1;
            continue;
        }
        let token_index = (seed_index * 37 + 11) % tokens.len();
        let span = &tokens[token_index];
        let kind = seed_index % 3;
        let mutated = match kind {
            0 => format!("{}{}", &source[..span.start], &source[span.end..]),
            1 => source[..span.start].to_owned(),
            _ => format!(
                "{}{}{}",
                &source[..span.end],
                &source[span.clone()],
                &source[span.end..]
            ),
        };
        if mutated == source || mutated.is_empty() || mutated.len() > MAX_SOURCE_BYTES {
            stats.mutation_skipped += 1;
            continue;
        }
        mutations.push(SourceCase {
            label: format!("{} token {token_index} kind {kind}", file.display()),
            source: mutated,
        });
    }
    stats.mutation_sources = mutations.len();
    let mut cases = rejected;
    cases.extend(mutations);
    assert!(!cases.is_empty(), "recovered differential needs sources");
    if corpus.selected.is_none() && (corpus.scope.is_empty() || corpus.scope == "all") {
        assert!(
            stats.rejected_sources > 0,
            "full recovered corpus must include go/parser-rejected files"
        );
        assert!(
            stats.mutation_sources >= 100,
            "full recovered corpus must include the mutation sample"
        );
    }

    for chunk in cases.chunks(ORACLE_CHUNK) {
        let sources: Vec<&str> = chunk.iter().map(|case| case.source.as_str()).collect();
        let reports = oracle.run("--recovered-shapes", &sources);
        assert_eq!(
            reports.len(),
            chunk.len(),
            "oracle must inspect every source"
        );
        for (report, case) in reports.iter().zip(chunk) {
            check_case(case, report, &mut stats, &mut mismatches);
        }
    }

    let informational: Vec<String> = stats
        .informational
        .classes
        .iter()
        .map(|(class, count)| format!("{class}={count}"))
        .collect();
    eprintln!(
        "recovered structure: {} packages, {} files ({} accepted, {} rejected), {} rejected sources ({} skipped), {} mutation sources ({} skipped; target {}), {} declarations / {} bodies / {} statements / {} expression nodes / {} initializer lists compared, {} truncated sources, {} go/parser-bailed sources; {} sources with native diagnostics; {} informational recovery differences [{}]",
        stats.packages,
        stats.files,
        stats.parser_accepted,
        stats.parser_rejected,
        stats.rejected_sources,
        stats.rejected_skipped,
        stats.mutation_sources,
        stats.mutation_skipped,
        mutation_target,
        stats.declarations_compared,
        stats.bodies_compared,
        stats.statements_compared,
        stats.expression_nodes,
        stats.initializers_compared,
        stats.truncated_sources,
        stats.oracle_bailed_sources,
        stats.diagnostic_sources,
        stats.informational.total(),
        informational.join(", ")
    );
    if let Some(path) = std::env::var_os("GOQUERY_RECOVERED_REPORT") {
        let report = serde_json::json!({
            "version": "go1.27.3",
            "scope": corpus.scope,
            "caps": {
                "max_rejected_sources": MAX_REJECTED_SOURCES,
                "mutation_target": mutation_target,
                "max_mutation_sources": MAX_MUTATION_SOURCES,
                "max_seed_bytes": MAX_SEED_BYTES,
                "max_source_bytes": MAX_SOURCE_BYTES,
                "max_name_bytes": MAX_NAME_BYTES,
                "max_declarations": MAX_DECLARATIONS,
                "max_statements": MAX_STATEMENTS,
                "oracle_chunk": ORACLE_CHUNK,
                "alignment_window": ALIGNMENT_WINDOW,
            },
            "sampling": "sorted-path stride selection; token index = (seed*37+11) % token_count; kind = seed % 3 (delete/truncate/duplicate)",
            "packages": stats.packages,
            "files": stats.files,
            "parser_accepted": stats.parser_accepted,
            "parser_rejected": stats.parser_rejected,
            "rejected_sources": stats.rejected_sources,
            "rejected_skipped": stats.rejected_skipped,
            "mutation_sources": stats.mutation_sources,
            "mutation_skipped": stats.mutation_skipped,
            "declarations_compared": stats.declarations_compared,
            "bodies_compared": stats.bodies_compared,
            "statements_compared": stats.statements_compared,
            "expression_nodes_compared": stats.expression_nodes,
            "initializers_compared": stats.initializers_compared,
            "truncated_sources": stats.truncated_sources,
            "oracle_bailed_sources": stats.oracle_bailed_sources,
            "diagnostic_sources": stats.diagnostic_sources,
            "informational_classes": stats.informational.classes,
            "informational_examples": stats.informational.examples,
            "mismatch_classes": mismatches.classes,
            "examples": mismatches.examples,
        });
        std::fs::write(
            path,
            serde_json::to_vec_pretty(&report).expect("encode recovered report"),
        )
        .expect("write recovered report");
    }
    mismatches.assert_empty("recovered structure divergences");
}

fn check_case(
    case: &SourceCase,
    report: &Value,
    stats: &mut Stats,
    mismatches: &mut MismatchBucket,
) {
    let label = &case.label;
    let source = &case.source;
    if report["truncated"].as_bool() == Some(true) {
        stats.truncated_sources += 1;
        return;
    }
    if report["bailed"].as_bool() == Some(true) {
        stats.oracle_bailed_sources += 1;
        return;
    }
    let oracle_accepted = report["accepted"]
        .as_bool()
        .expect("recovered acceptance bool");
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        goquery::syntax::validate_file(source)
    })) {
        Ok(diagnostics) => {
            stats.diagnostic_sources += 1;
            if diagnostics.is_empty() != oracle_accepted {
                mismatches.record(
                    "recovered:acceptance",
                    format!(
                        "{label}: go/parser accepted={oracle_accepted}, native diagnostics={}",
                        diagnostics.len()
                    ),
                );
            }
        }
        Err(_) => {
            mismatches.record(
                "recovered:panic",
                format!("{label}: validate_file panicked"),
            );
            return;
        }
    }
    let expected: Vec<RecoveredDeclaration> = report["declarations"]
        .as_array()
        .expect("recovered declarations")
        .iter()
        .map(decode_declaration)
        .collect();
    for declaration in &expected {
        check_span(
            label,
            source,
            declaration.start,
            declaration.end,
            mismatches,
            "oracle",
            false,
        );
        check_span(
            label,
            source,
            declaration.name_start,
            declaration.name_end,
            mismatches,
            "oracle",
            false,
        );
        if let Some(body) = &declaration.body {
            check_span(
                label, source, body.start, body.end, mismatches, "oracle", false,
            );
        }
        for statement in &declaration.statements {
            check_span(
                label,
                source,
                statement.start,
                statement.end,
                mismatches,
                "oracle",
                false,
            );
            for expression in &statement.expressions {
                check_span(
                    label,
                    source,
                    expression.start,
                    expression.end,
                    mismatches,
                    "oracle",
                    false,
                );
            }
        }
    }
    let native = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        native_declarations(source)
    })) {
        Ok(Some(declarations)) => declarations,
        Ok(None) => {
            stats.truncated_sources += 1;
            return;
        }
        Err(_) => {
            mismatches.record(
                "recovered:panic",
                format!("{label}: native recovery panicked"),
            );
            return;
        }
    };
    stats.declarations_compared += expected.len();
    let bad_declarations: Vec<Range<usize>> = report["bad_declarations"]
        .as_array()
        .map(|spans| {
            spans
                .iter()
                .map(|span| {
                    span["start"].as_u64().expect("bad declaration start") as usize
                        ..span["end"].as_u64().expect("bad declaration end") as usize
                })
                .collect()
        })
        .unwrap_or_default();
    for span in &bad_declarations {
        check_span(
            label, source, span.start, span.end, mismatches, "oracle", false,
        );
    }
    let debug =
        std::env::var("GOQUERY_RECOVERED_DEBUG").is_ok_and(|needle| label.contains(&needle));
    if debug {
        eprintln!("RECOVERED_DEBUG {label}\n go/ast: {expected:#?}\n native: {native:#?}");
    }
    let comparison = RecoveredComparison {
        label,
        source,
        oracle_accepted,
        expected_bodies: expected.iter().filter_map(|d| d.body.clone()).collect(),
        actual_bodies: native.iter().filter_map(|d| d.body.clone()).collect(),
        expected: &expected,
        actual: &native,
        bad_declarations: &bad_declarations,
    };
    compare_declarations(&comparison, stats, mismatches);

    let expected_initializers: Vec<RecoveredInitializer> = report["initializers"]
        .as_array()
        .map(|initializers| initializers.iter().map(decode_initializer).collect())
        .unwrap_or_default();
    for initializer in &expected_initializers {
        check_span(
            label,
            source,
            initializer.start,
            initializer.end,
            mismatches,
            "oracle",
            false,
        );
        for expression in &initializer.expressions {
            check_span(
                label,
                source,
                expression.start,
                expression.end,
                mismatches,
                "oracle",
                false,
            );
        }
    }
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| native_initializers(source))) {
        Ok(Some(native_initializers)) => {
            if debug {
                eprintln!(
                    "RECOVERED_DEBUG {label}\n expected initializers: {expected_initializers:#?}\n native initializers: {native_initializers:#?}"
                );
            }
            compare_initializers(
                label,
                source,
                &expected_initializers,
                &native_initializers,
                stats,
                mismatches,
            );
        }
        Ok(None) => stats.truncated_sources += 1,
        Err(_) => mismatches.record(
            "recovered:panic",
            format!("{label}: initializer recovery panicked"),
        ),
    }
}

fn decode_declaration(value: &Value) -> RecoveredDeclaration {
    let statements = value["statements"]
        .as_array()
        .map(|statements| {
            statements
                .iter()
                .map(|statement| RecoveredStatement {
                    kind: statement["kind"]
                        .as_str()
                        .expect("statement kind")
                        .to_owned(),
                    start: statement["start"].as_u64().expect("statement start") as usize,
                    end: statement["end"].as_u64().expect("statement end") as usize,
                    expressions: decode_expressions(&statement["expressions"]),
                })
                .collect()
        })
        .unwrap_or_default();
    let body = (value["body"].as_bool() == Some(true)).then(|| {
        value["body_start"].as_u64().expect("body start") as usize
            ..value["body_end"].as_u64().expect("body end") as usize
    });
    RecoveredDeclaration {
        kind: value["kind"].as_str().expect("declaration kind").to_owned(),
        name: value["name"].as_str().expect("declaration name").to_owned(),
        start: value["start"].as_u64().expect("declaration start") as usize,
        end: value["end"].as_u64().expect("declaration end") as usize,
        name_start: value["name_start"].as_u64().expect("name start") as usize,
        name_end: value["name_end"].as_u64().expect("name end") as usize,
        body,
        statements,
    }
}

fn decode_expressions(value: &Value) -> Vec<RecoveredExpression> {
    value
        .as_array()
        .map(|expressions| {
            expressions
                .iter()
                .map(|expression| RecoveredExpression {
                    slot: expression["slot"].as_u64().expect("expression slot") as usize,
                    depth: expression["depth"].as_u64().expect("expression depth") as usize,
                    kind: expression["kind"]
                        .as_str()
                        .expect("expression kind")
                        .to_owned(),
                    name: expression["name"].as_str().unwrap_or("").to_owned(),
                    start: expression["start"].as_u64().expect("expression start") as usize,
                    end: expression["end"].as_u64().expect("expression end") as usize,
                })
                .collect()
        })
        .unwrap_or_default()
}

fn decode_initializer(value: &Value) -> RecoveredInitializer {
    RecoveredInitializer {
        start: value["start"].as_u64().expect("initializer start") as usize,
        end: value["end"].as_u64().expect("initializer end") as usize,
        expressions: decode_expressions(&value["expressions"]),
    }
}

fn native_declarations(source: &str) -> Option<Vec<RecoveredDeclaration>> {
    use goquery::syntax::SymbolKind;
    let stub = goquery::syntax::stub(source);
    let mut declarations = Vec::with_capacity(stub.symbols.len());
    for symbol in &stub.symbols {
        let kind = match symbol.kind {
            SymbolKind::Function => "func",
            SymbolKind::Method => "method",
            SymbolKind::Type => "type",
            SymbolKind::Constant => "const",
            SymbolKind::Variable => "var",
        };
        let mut statements = Vec::new();
        let mut truncated = false;
        if let Some(body) = &symbol.body
            && let Some(text) = source.get(body.clone())
        {
            let tree = goquery::syntax::parse_block(text);
            collect_native_statements(&tree, text, body.start, &mut statements, &mut truncated);
        }
        if truncated {
            return None;
        }
        declarations.push(RecoveredDeclaration {
            kind: kind.to_owned(),
            name: truncate_name(&symbol.name),
            start: symbol.range.start,
            end: symbol.range.end,
            name_start: symbol.selection.start,
            name_end: symbol.selection.end,
            body: symbol.body.clone(),
            statements,
        });
    }
    Some(declarations)
}

fn truncate_name(name: &str) -> String {
    if name.len() <= MAX_NAME_BYTES {
        return name.to_owned();
    }
    let mut cut = MAX_NAME_BYTES;
    while cut > 0 && !name.is_char_boundary(cut) {
        cut -= 1;
    }
    name[..cut].to_owned()
}

fn collect_native_statements(
    tree: &goquery::syntax::BlockTree,
    source: &str,
    base: usize,
    statements: &mut Vec<RecoveredStatement>,
    truncated: &mut bool,
) {
    let Some(root) = tree.statement(tree.root()) else {
        return;
    };
    let goquery::syntax::StmtKind::Block { statements: body } = &root.kind else {
        return;
    };
    for id in body {
        collect_native_statement(tree, source, base, *id, statements, truncated);
        if *truncated {
            return;
        }
    }
}

fn collect_native_statement(
    tree: &goquery::syntax::BlockTree,
    source: &str,
    base: usize,
    id: goquery::syntax::StmtId,
    statements: &mut Vec<RecoveredStatement>,
    truncated: &mut bool,
) {
    if statements.len() >= MAX_STATEMENTS {
        *truncated = true;
        return;
    }
    let Some(statement) = tree.statement(id) else {
        return;
    };
    if let goquery::syntax::StmtKind::DeclarationGroup { declarations } = &statement.kind {
        for id in declarations {
            collect_native_statement(tree, source, base, *id, statements, truncated);
            if *truncated {
                return;
            }
        }
        return;
    }
    if let Some(kind) = native_statement_kind(tree, statement, source) {
        let mut collector = ExprCollector::new(tree, source, base, MAX_EXPRESSIONS_PER_STATEMENT);
        collector.collect_statement(statement);
        if collector.truncated {
            *truncated = true;
            return;
        }
        statements.push(RecoveredStatement {
            kind,
            start: base + statement.span.start,
            end: base + statement.span.end,
            expressions: collector.expressions,
        });
    }
}

fn native_statement_kind(
    tree: &goquery::syntax::BlockTree,
    statement: &goquery::syntax::Statement,
    source: &str,
) -> Option<String> {
    use goquery::syntax::StmtKind;
    Some(match &statement.kind {
        StmtKind::Block { .. } => "block".to_owned(),
        StmtKind::Expression { .. } => "expr".to_owned(),
        StmtKind::Assignment { .. } => "assign".to_owned(),
        StmtKind::Return { .. } => "return".to_owned(),
        StmtKind::Increment { .. } => "incdec".to_owned(),
        StmtKind::Send { .. } => "send".to_owned(),
        StmtKind::Go { call } => return is_call_expression(tree, *call).then(|| "go".to_owned()),
        StmtKind::Defer { call } => {
            return is_call_expression(tree, *call).then(|| "defer".to_owned());
        }
        StmtKind::Declaration { keyword, .. } => format!("decl:{}", &source[keyword.clone()]),
        StmtKind::Select { .. } => "select".to_owned(),
        StmtKind::CommunicationCase { .. } => "commcase".to_owned(),
        StmtKind::RangeClause { .. } => "rangeclause".to_owned(),
        StmtKind::RangeFor { .. } => "range".to_owned(),
        StmtKind::Branch { .. } => "branch".to_owned(),
        StmtKind::Label { .. } => "label".to_owned(),
        StmtKind::If { .. } => "if".to_owned(),
        StmtKind::For { .. } => "for".to_owned(),
        StmtKind::Switch { .. } => "switch".to_owned(),
        StmtKind::TypeSwitch { .. } => "typeswitch".to_owned(),
        StmtKind::Case { .. } => "case".to_owned(),
        _ => "unknown".to_owned(),
    })
}

fn is_call_expression(
    tree: &goquery::syntax::BlockTree,
    expression: goquery::syntax::ExprId,
) -> bool {
    match tree
        .expression(expression)
        .map(|expression| &expression.kind)
    {
        Some(goquery::syntax::ExprKind::Call { .. }) => true,
        Some(goquery::syntax::ExprKind::Parenthesized { expression }) => {
            is_call_expression(tree, *expression)
        }
        _ => false,
    }
}

struct ExprCollector<'a> {
    tree: &'a goquery::syntax::BlockTree,
    source: &'a str,
    base: usize,
    slot: usize,
    budget: usize,
    truncated: bool,
    expressions: Vec<RecoveredExpression>,
}

impl<'a> ExprCollector<'a> {
    fn new(
        tree: &'a goquery::syntax::BlockTree,
        source: &'a str,
        base: usize,
        budget: usize,
    ) -> Self {
        Self {
            tree,
            source,
            base,
            slot: 0,
            budget,
            truncated: false,
            expressions: Vec::new(),
        }
    }

    fn collect_statement(&mut self, statement: &goquery::syntax::Statement) {
        use goquery::syntax::StmtKind;
        match &statement.kind {
            StmtKind::Expression { expression }
            | StmtKind::Increment { expression, .. }
            | StmtKind::Go { call: expression }
            | StmtKind::Defer { call: expression } => self.add_slot(*expression),
            StmtKind::Assignment { left, right, .. } => {
                for expr in left.iter().chain(right) {
                    self.add_slot(*expr);
                }
            }
            StmtKind::Return { values } | StmtKind::Declaration { values, .. } => {
                for expr in values {
                    self.add_slot(*expr);
                }
            }
            StmtKind::Send { channel, value } => {
                self.add_slot(*channel);
                self.add_slot(*value);
            }
            StmtKind::If {
                init, condition, ..
            } => {
                self.add_simple(*init);
                self.add_slot(*condition);
            }
            StmtKind::For {
                init,
                condition,
                post,
                ..
            } => {
                self.add_simple(*init);
                if let Some(condition) = condition {
                    self.add_slot(*condition);
                }
                self.add_simple(*post);
            }
            StmtKind::RangeFor { clause, .. } => {
                if let Some(StmtKind::RangeClause { left, value, .. }) =
                    self.tree.statement(*clause).map(|s| &s.kind)
                {
                    // Oracle order: range expression first, then key/value operands.
                    self.add_slot(*value);
                    for expr in left {
                        self.add_slot(*expr);
                    }
                }
            }
            StmtKind::Switch { init, tag, .. } => {
                self.add_simple(*init);
                if let Some(tag) = tag {
                    self.add_slot(*tag);
                }
            }
            StmtKind::TypeSwitch { init, guard, .. } => {
                self.add_simple(*init);
                self.add_simple(Some(*guard));
            }
            _ => {}
        }
    }

    fn add_simple(&mut self, statement: Option<goquery::syntax::StmtId>) {
        use goquery::syntax::StmtKind;
        let Some(statement) = statement.and_then(|id| self.tree.statement(id)) else {
            return;
        };
        match &statement.kind {
            StmtKind::Expression { expression } | StmtKind::Increment { expression, .. } => {
                self.add_slot(*expression);
            }
            StmtKind::Assignment { left, right, .. } => {
                for expr in left.iter().chain(right) {
                    self.add_slot(*expr);
                }
            }
            StmtKind::Send { channel, value } => {
                self.add_slot(*channel);
                self.add_slot(*value);
            }
            _ => {}
        }
    }

    fn add_slot(&mut self, expression: goquery::syntax::ExprId) {
        let slot = self.slot;
        self.add_forest(expression, slot, 0);
        self.slot += 1;
    }

    fn add_forest(&mut self, id: goquery::syntax::ExprId, slot: usize, depth: usize) {
        if self.truncated {
            return;
        }
        if self.budget == 0 {
            self.truncated = true;
            return;
        }
        let Some(expression) = self.tree.expression(id) else {
            return;
        };
        let Some((kind, name, children)) = native_expression_shape(self.source, expression) else {
            return;
        };
        self.expressions.push(RecoveredExpression {
            slot,
            depth,
            kind,
            name,
            start: self.base + expression.span.start,
            end: self.base + expression.span.end,
        });
        self.budget -= 1;
        for child in children {
            self.add_forest(child, slot, depth + 1);
        }
    }
}

fn native_expression_shape(
    source: &str,
    expression: &goquery::syntax::Expr,
) -> Option<(String, String, Vec<goquery::syntax::ExprId>)> {
    use goquery::syntax::ExprKind;
    let node = |kind: &str| Some((kind.to_owned(), String::new(), Vec::new()));
    match &expression.kind {
        ExprKind::Name => Some((
            "name".to_owned(),
            truncate_name(&source[expression.span.clone()]),
            Vec::new(),
        )),
        ExprKind::Literal(_) => node("lit"),
        ExprKind::Missing => node("bad"),
        ExprKind::Type => node("type"),
        ExprKind::Composite {
            literal_type,
            elements,
        } => {
            let mut children = Vec::new();
            if let Some(literal_type) = literal_type {
                children.push(*literal_type);
            }
            children.extend(elements);
            Some(("composite".to_owned(), String::new(), children))
        }
        ExprKind::KeyValue { key, value } => {
            Some(("keyvalue".to_owned(), String::new(), vec![*key, *value]))
        }
        ExprKind::FunctionLiteral { .. } => node("funclit"),
        ExprKind::Unary { operator, operand } => {
            let operator = &source[operator.clone()];
            let kind = match operator {
                "<-" => "receive".to_owned(),
                "*" => "star".to_owned(),
                _ => format!("unary:{operator}"),
            };
            Some((kind, String::new(), vec![*operand]))
        }
        ExprKind::Binary {
            left,
            operator,
            right,
        } => Some((
            format!("binary:{}", &source[operator.clone()]),
            String::new(),
            vec![*left, *right],
        )),
        ExprKind::Parenthesized { expression } => {
            Some(("paren".to_owned(), String::new(), vec![*expression]))
        }
        ExprKind::Selector { operand, field } => Some((
            "selector".to_owned(),
            truncate_name(&source[field.clone()]),
            vec![*operand],
        )),
        ExprKind::Call {
            function,
            arguments,
            variadic,
            ..
        } => {
            let mut children = vec![*function];
            children.extend(arguments);
            let kind = if *variadic { "call:..." } else { "call" };
            Some((kind.to_owned(), String::new(), children))
        }
        ExprKind::Index { operand, arguments } => {
            let mut children = vec![*operand];
            children.extend(arguments);
            Some(("index".to_owned(), String::new(), children))
        }
        ExprKind::Slice {
            operand,
            low,
            high,
            max,
            ..
        } => {
            let mut children = vec![*operand];
            children.extend([*low, *high, *max].into_iter().flatten());
            Some(("slice".to_owned(), String::new(), children))
        }
        ExprKind::TypeAssertion { operand, .. } | ExprKind::TypeSwitchGuard { operand } => {
            Some(("typeassert".to_owned(), String::new(), vec![*operand]))
        }
        _ => None,
    }
}

fn native_initializers(source: &str) -> Option<Vec<RecoveredInitializer>> {
    use goquery::syntax::{SymbolKind, TokenKind};
    let stub = goquery::syntax::stub(source);
    let mut seen = std::collections::BTreeSet::new();
    let mut initializers = Vec::new();
    let mut source_budget = MAX_SOURCE_EXPRESSIONS;
    for symbol in &stub.symbols {
        if !matches!(symbol.kind, SymbolKind::Variable | SymbolKind::Constant) {
            continue;
        }
        let key = (symbol.range.start, symbol.range.end);
        if !seen.insert(key) {
            continue;
        }
        let Some(spec) = source.get(key.0..key.1) else {
            continue;
        };
        let mut depth = 0usize;
        let mut assign = None;
        for token in goquery::syntax::Lexer::new(spec) {
            match token.kind {
                TokenKind::LParen | TokenKind::LBrack | TokenKind::LBrace => depth += 1,
                TokenKind::RParen | TokenKind::RBrack | TokenKind::RBrace => {
                    depth = depth.saturating_sub(1);
                }
                TokenKind::Assign if depth == 0 => {
                    assign = Some(token.span);
                    break;
                }
                _ => {}
            }
        }
        let Some(assign) = assign else {
            continue;
        };
        let values = &spec[assign.end..];
        if values.trim().is_empty() {
            continue;
        }
        let prefix = "{ _ = ";
        let wrapper = format!("{prefix}{values}\n}}");
        let tree = goquery::syntax::parse_block(&wrapper);
        let Some(goquery::syntax::StmtKind::Block { statements }) =
            tree.statement(tree.root()).map(|s| &s.kind)
        else {
            continue;
        };
        let Some(goquery::syntax::StmtKind::Assignment { right, .. }) = statements
            .first()
            .and_then(|id| tree.statement(*id))
            .map(|s| &s.kind)
        else {
            continue;
        };
        if right.is_empty() {
            continue;
        }
        let mut collector = ExprCollector::new(&tree, &wrapper, 0, MAX_EXPRESSIONS_PER_STATEMENT);
        for (slot, expression) in right.iter().enumerate() {
            collector.budget = MAX_EXPRESSIONS_PER_STATEMENT;
            collector.add_forest(*expression, slot, 0);
            if collector.truncated {
                return None;
            }
        }
        let mut expressions = collector.expressions;
        let source_base = key.0 + assign.end;
        for expression in &mut expressions {
            expression.start = source_base + expression.start - prefix.len();
            expression.end = source_base + expression.end - prefix.len();
        }
        expressions.retain(|expression| expression.start <= key.1);
        for expression in &mut expressions {
            expression.start = expression.start.min(key.1);
            expression.end = expression.end.min(key.1);
        }
        if expressions.is_empty() {
            continue;
        }
        if expressions.len() > source_budget {
            return None;
        }
        source_budget -= expressions.len();
        let start = expressions
            .iter()
            .filter(|e| e.depth == 0)
            .map(|e| e.start)
            .min()
            .unwrap_or(source_base);
        let end = expressions
            .iter()
            .filter(|e| e.depth == 0)
            .map(|e| e.end)
            .max()
            .unwrap_or(start);
        initializers.push(RecoveredInitializer {
            start,
            end,
            expressions,
        });
    }
    Some(initializers)
}

fn compare_declarations(
    comparison: &RecoveredComparison<'_>,
    stats: &mut Stats,
    mismatches: &mut MismatchBucket,
) {
    let label = comparison.label;
    let expected = comparison.expected;
    let actual = comparison.actual;
    let actual_bodies = &comparison.actual_bodies;
    let mut actual_index = 0usize;
    for expected_declaration in expected {
        let window_end = (actual_index + ALIGNMENT_WINDOW).min(actual.len());
        let found = (actual_index..window_end)
            .filter(|&index| same_declaration_key(expected_declaration, &actual[index]))
            .max_by_key(|&index| {
                let actual = &actual[index];
                let exact = actual.name_start == expected_declaration.name_start
                    && actual.name_end == expected_declaration.name_end;
                let distance = actual.name_start.abs_diff(expected_declaration.name_start);
                (exact, usize::MAX - distance)
            });
        match found {
            Some(found_index) => {
                for extra in &actual[actual_index..found_index] {
                    record_extra_declaration(comparison, extra, stats, mismatches);
                }
                actual_index = found_index + 1;
                check_declaration(
                    comparison,
                    expected_declaration,
                    &actual[found_index],
                    stats,
                    mismatches,
                );
            }
            None => {
                if contains_offset(actual_bodies, expected_declaration.start) {
                    stats.count_info("recovered:go_nested_declaration", 1);
                } else if actual.iter().any(|declaration| {
                    declaration.start <= expected_declaration.start
                        && expected_declaration.start < declaration.end
                }) {
                    stats.count_info("recovered:native_swallowed_declaration", 1);
                } else {
                    mismatches.record(
                        "recovered:declaration_missing",
                        format!(
                            "{label}: go/ast {} {} at {} missing in native",
                            expected_declaration.kind,
                            expected_declaration.name,
                            expected_declaration.start
                        ),
                    );
                }
            }
        }
    }
    for extra in &actual[actual_index..] {
        record_extra_declaration(comparison, extra, stats, mismatches);
    }
}

fn record_extra_declaration(
    comparison: &RecoveredComparison<'_>,
    extra: &RecoveredDeclaration,
    stats: &mut Stats,
    mismatches: &mut MismatchBucket,
) {
    let label = comparison.label;
    if comparison.expected.iter().any(|declaration| {
        same_declaration_key(declaration, extra)
            && declaration.name_start == extra.name_start
            && declaration.name_end == extra.name_end
    }) {
        mismatches.record(
            "recovered:declaration_duplicate",
            format!(
                "{label}: native {} {} at {} duplicates a recovered declaration",
                extra.kind, extra.name, extra.start
            ),
        );
    } else if contains_offset(&comparison.expected_bodies, extra.start) {
        stats.count_info("recovered:native_split_declaration", 1);
    } else if contains_offset(comparison.bad_declarations, extra.start) {
        stats.count_info("recovered:go_bad_declaration", 1);
    } else if !comparison.oracle_accepted {
        stats.count_info("recovered:go_dropped_declaration", 1);
    } else {
        mismatches.record(
            "recovered:declaration_extra",
            format!(
                "{label}: native {} {} at {}",
                extra.kind, extra.name, extra.start
            ),
        );
    }
}

fn contains_offset(ranges: &[Range<usize>], offset: usize) -> bool {
    ranges
        .iter()
        .any(|range| range.start <= offset && offset < range.end)
}

fn same_declaration_key(expected: &RecoveredDeclaration, actual: &RecoveredDeclaration) -> bool {
    expected.kind == actual.kind && expected.name == actual.name
}

fn check_declaration(
    comparison: &RecoveredComparison<'_>,
    expected: &RecoveredDeclaration,
    actual: &RecoveredDeclaration,
    stats: &mut Stats,
    mismatches: &mut MismatchBucket,
) {
    let label = comparison.label;
    let source = comparison.source;
    check_span(
        label,
        source,
        actual.start,
        actual.end,
        mismatches,
        "native",
        true,
    );
    check_span(
        label,
        source,
        actual.name_start,
        actual.name_end,
        mismatches,
        "native",
        true,
    );
    if !(actual.start <= actual.name_start && actual.name_end <= actual.end) {
        mismatches.record(
            "recovered:native_span_envelope",
            format!(
                "{label} {}: declaration {}..{} does not contain name {}..{}",
                actual.name, actual.start, actual.end, actual.name_start, actual.name_end
            ),
        );
    }
    if expected.name_start != actual.name_start || expected.name_end != actual.name_end {
        mismatches.record(
            "recovered:declaration_name_span",
            format!(
                "{label} {} {}: go/ast name {}..{} native name {}..{}",
                actual.kind,
                actual.name,
                expected.name_start,
                expected.name_end,
                actual.name_start,
                actual.name_end
            ),
        );
    }
    match (&expected.body, &actual.body) {
        (None, None) => {}
        (Some(expected_body), Some(actual_body)) => {
            check_span(
                label,
                source,
                actual_body.start,
                actual_body.end,
                mismatches,
                "native",
                true,
            );
            for statement in &actual.statements {
                check_span(
                    label,
                    source,
                    statement.start,
                    statement.end,
                    mismatches,
                    "native",
                    true,
                );
                if statement.start < actual_body.start || statement.end > actual_body.end {
                    mismatches.record(
                        "recovered:native_statement_span",
                        format!(
                            "{label} {}: statement {} {}..{} outside body {}..{}",
                            expected.name,
                            statement.kind,
                            statement.start,
                            statement.end,
                            actual_body.start,
                            actual_body.end
                        ),
                    );
                }
            }
            stats.bodies_compared += 1;
            stats.statements_compared += expected.statements.len().max(actual.statements.len());
            let bad_statements: Vec<&RecoveredStatement> = expected
                .statements
                .iter()
                .filter(|statement| statement.kind == "bad")
                .collect();
            let expected_statements: Vec<&RecoveredStatement> = expected
                .statements
                .iter()
                .filter(|statement| statement.kind != "bad")
                .collect();
            let expected_kinds: Vec<&str> = expected_statements
                .iter()
                .map(|statement| statement.kind.as_str())
                .collect();
            let mut filtered_bad = 0usize;
            let actual_statements: Vec<&RecoveredStatement> = actual
                .statements
                .iter()
                .filter(|statement| {
                    let inside_bad = bad_statements
                        .iter()
                        .any(|bad| bad.start <= statement.start && statement.start < bad.end);
                    if inside_bad {
                        filtered_bad += 1;
                    }
                    !inside_bad
                })
                .collect();
            let actual_kinds: Vec<&str> = actual_statements
                .iter()
                .map(|statement| statement.kind.as_str())
                .collect();
            if filtered_bad > 0 {
                stats.count_info("recovered:oracle_bad_statement", 1);
            }
            let comparable = if expected_kinds == actual_kinds
                || is_prefix(&expected_kinds, &actual_kinds)
                || is_prefix(&actual_kinds, &expected_kinds)
            {
                expected_statements.len().min(actual_statements.len())
            } else {
                0
            };
            for index in 0..comparable {
                compare_slot_list(
                    label,
                    source,
                    &expected_statements[index].kind,
                    expected_statements[index].start..expected_statements[index].end,
                    actual_statements[index].start..actual_statements[index].end,
                    &expected_statements[index].expressions,
                    &actual_statements[index].expressions,
                    "recovered:expression_extent",
                    stats,
                    mismatches,
                );
            }
            if expected_kinds != actual_kinds {
                if is_prefix(&expected_kinds, &actual_kinds)
                    || is_prefix(&actual_kinds, &expected_kinds)
                {
                    stats.count_info("recovered:body_prefix", 1);
                } else if expected_body != actual_body {
                    stats.count_info("recovered:body_extent", 1);
                } else {
                    mismatches.record(
                        "recovered:body_statements",
                        format!(
                            "{label} {}: go/ast {:?} native {:?}",
                            expected.name,
                            bounded_kinds(&expected_kinds),
                            bounded_kinds(&actual_kinds)
                        ),
                    );
                }
            }
        }
        _ => stats.count_info("recovered:body_presence", 1),
    }
}

struct ExpressionContext<'a> {
    label: &'a str,
    source: &'a str,
    actual_extent: Range<usize>,
    extents_equal: bool,
}

#[allow(clippy::too_many_arguments)]
fn compare_slot_list(
    label: &str,
    source: &str,
    item_label: &str,
    expected_extent: Range<usize>,
    actual_extent: Range<usize>,
    expected_exprs: &[RecoveredExpression],
    actual_exprs: &[RecoveredExpression],
    extent_info_class: &str,
    stats: &mut Stats,
    mismatches: &mut MismatchBucket,
) {
    let extents_equal = expected_extent == actual_extent;
    let expected_slots = group_expression_slots(expected_exprs);
    let actual_slots = group_expression_slots(actual_exprs);
    let expected_keys: Vec<usize> = expected_slots.keys().copied().collect();
    let actual_keys: Vec<usize> = actual_slots.keys().copied().collect();
    if expected_keys != actual_keys {
        if extents_equal {
            mismatches.record(
                "recovered:expression_slots",
                format!(
                    "{label} {item_label} {}..{}: expression slots {expected_keys:?} vs {actual_keys:?}",
                    expected_extent.start, expected_extent.end
                ),
            );
        } else {
            stats.count_info(extent_info_class, 1);
        }
    }
    let context = ExpressionContext {
        label,
        source,
        actual_extent,
        extents_equal,
    };
    for (expected_slot, actual_slot) in expected_keys.iter().zip(actual_keys.iter()) {
        compare_expression_groups(
            &context,
            &expected_slots[expected_slot],
            &actual_slots[actual_slot],
            stats,
            mismatches,
        );
    }
}

fn compare_initializers(
    label: &str,
    source: &str,
    expected: &[RecoveredInitializer],
    actual: &[RecoveredInitializer],
    stats: &mut Stats,
    mismatches: &mut MismatchBucket,
) {
    let paired = expected.len().min(actual.len());
    stats.initializers_compared += paired;
    for index in 0..paired {
        let exp = &expected[index];
        let act = &actual[index];
        if exp.start != act.start || exp.end != act.end {
            stats.count_info("recovered:initializer_extent", 1);
        }
        compare_slot_list(
            label,
            source,
            &format!("initializer {index}"),
            exp.start..exp.end,
            act.start..act.end,
            &exp.expressions,
            &act.expressions,
            "recovered:initializer_extent",
            stats,
            mismatches,
        );
    }
    if expected.len() != actual.len() {
        stats.count_info(
            "recovered:initializer_extent",
            expected.len().abs_diff(actual.len()).max(1),
        );
    }
}

fn group_expression_slots(
    expressions: &[RecoveredExpression],
) -> BTreeMap<usize, Vec<RecoveredExpression>> {
    let mut slots: BTreeMap<usize, Vec<RecoveredExpression>> = BTreeMap::new();
    for expression in expressions {
        slots
            .entry(expression.slot)
            .or_default()
            .push(expression.clone());
    }
    slots
}

fn expression_is_synthetic(source: &str, expression: &RecoveredExpression) -> bool {
    let Some(text) = source.get(expression.start..expression.end) else {
        return false;
    };
    match expression.kind.as_str() {
        "name" => text != expression.name,
        "selector" => expression.name.is_empty() || !text.ends_with(&expression.name),
        _ => false,
    }
}

fn compare_expression_groups(
    context: &ExpressionContext<'_>,
    expected: &[RecoveredExpression],
    actual: &[RecoveredExpression],
    stats: &mut Stats,
    mismatches: &mut MismatchBucket,
) {
    stats.expression_nodes += expected.len() + actual.len();
    for node in actual {
        check_span(
            context.label,
            context.source,
            node.start,
            node.end,
            mismatches,
            "native",
            true,
        );
        if node.start < context.actual_extent.start || node.end > context.actual_extent.end {
            mismatches.record(
                "recovered:expression_span",
                format!(
                    "{}: {} {}..{} outside {}..{}",
                    context.label,
                    node.kind,
                    node.start,
                    node.end,
                    context.actual_extent.start,
                    context.actual_extent.end
                ),
            );
        }
    }
    let expected_nodes: Vec<&RecoveredExpression> =
        expected.iter().filter(|node| node.kind != "bad").collect();
    let actual_nodes: Vec<&RecoveredExpression> =
        actual.iter().filter(|node| node.kind != "bad").collect();
    let mut seen = std::collections::BTreeSet::new();
    for node in &actual_nodes {
        if !seen.insert((node.start, node.end, node.kind.clone(), node.name.clone())) {
            mismatches.record(
                "recovered:expression_duplicate",
                format!(
                    "{}: native {} at {}..{} repeated",
                    context.label, node.kind, node.start, node.end
                ),
            );
        }
    }
    let expected_by_extent: BTreeMap<(usize, usize), &RecoveredExpression> = expected_nodes
        .iter()
        .map(|node| ((node.start, node.end), *node))
        .collect();
    let synthetic_expected = expected_nodes
        .iter()
        .filter(|node| expression_is_synthetic(context.source, node))
        .count();
    let mut conflicts = 0usize;
    for node in &actual_nodes {
        if let Some(expected_node) = expected_by_extent.get(&(node.start, node.end))
            && (expected_node.kind != node.kind || expected_node.name != node.name)
            && !expression_is_synthetic(context.source, expected_node)
        {
            mismatches.record(
                "recovered:expression_kind",
                format!(
                    "{}: go/ast {} {:?} native {} {:?} at {}..{}",
                    context.label,
                    expected_node.kind,
                    expected_node.name,
                    node.kind,
                    node.name,
                    node.start,
                    node.end
                ),
            );
            conflicts += 1;
        }
    }
    if expected_nodes.is_empty() && actual_nodes.is_empty() {
        return;
    }
    if expected_nodes.is_empty() || actual_nodes.is_empty() {
        if context.extents_equal {
            mismatches.record(
                "recovered:expression_structure",
                format!("{}: one side recovered no expression nodes", context.label),
            );
        } else {
            stats.informational.record(
                "recovered:expression_extent",
                format!(
                    "{}: one side empty (go/ast {} nodes, native {} nodes)",
                    context.label,
                    expected_nodes.len(),
                    actual_nodes.len()
                ),
            );
        }
        return;
    }
    let expected_sequence: Vec<(&str, &str)> = expected_nodes
        .iter()
        .map(|node| (node.kind.as_str(), node.name.as_str()))
        .collect();
    let actual_sequence: Vec<(&str, &str)> = actual_nodes
        .iter()
        .map(|node| (node.kind.as_str(), node.name.as_str()))
        .collect();
    let roots_equal = expected_nodes[0].start == actual_nodes[0].start
        && expected_nodes[0].end == actual_nodes[0].end;
    if expected_sequence == actual_sequence {
        let expected_depths: Vec<usize> = expected_nodes.iter().map(|node| node.depth).collect();
        let actual_depths: Vec<usize> = actual_nodes.iter().map(|node| node.depth).collect();
        if expected_depths != actual_depths {
            if roots_equal && context.extents_equal {
                stats.count_info("recovered:expression_nesting", 1);
            } else {
                stats.informational.record(
                    "recovered:expression_extent",
                    format!(
                        "{}: equal kinds at go/ast {}..{} native {}..{}",
                        context.label,
                        expected_nodes[0].start,
                        expected_nodes[0].end,
                        actual_nodes[0].start,
                        actual_nodes[0].end
                    ),
                );
            }
        }
        return;
    }
    if roots_equal && context.extents_equal {
        if conflicts == 0 {
            let summarize = |nodes: &[&RecoveredExpression]| {
                expression_summary(
                    &nodes
                        .iter()
                        .map(|n| (n.depth, n.kind.as_str(), n.name.as_str()))
                        .collect::<Vec<_>>(),
                )
            };
            let detail = format!(
                "{}: go/ast {:?} native {:?}",
                context.label,
                summarize(&expected_nodes),
                summarize(&actual_nodes)
            );
            if synthetic_expected > 0 {
                stats
                    .informational
                    .record("recovered:go_synthetic_expression", detail);
            } else {
                mismatches.record("recovered:expression_structure", detail);
            }
        }
    } else {
        stats.informational.record(
            "recovered:expression_extent",
            format!(
                "{}: go/ast {}..{} native {}..{}",
                context.label,
                expected_nodes[0].start,
                expected_nodes[0].end,
                actual_nodes[0].start,
                actual_nodes[0].end
            ),
        );
    }
}

fn expression_summary(sequence: &[(usize, &str, &str)]) -> Vec<String> {
    sequence
        .iter()
        .take(24)
        .map(|(depth, kind, name)| {
            if name.is_empty() {
                format!("{depth}:{kind}")
            } else {
                format!("{depth}:{kind}:{name}")
            }
        })
        .collect()
}

fn is_prefix(short: &[&str], long: &[&str]) -> bool {
    short.len() <= long.len()
        && short
            .iter()
            .zip(long.iter())
            .all(|(left, right)| left == right)
}

fn bounded_kinds<'a>(kinds: &[&'a str]) -> Vec<&'a str> {
    kinds.iter().take(24).copied().collect()
}

fn check_span(
    label: &str,
    source: &str,
    start: usize,
    end: usize,
    mismatches: &mut MismatchBucket,
    origin: &str,
    require_boundaries: bool,
) {
    let valid = start <= end
        && end <= source.len()
        && (!require_boundaries
            || (source.is_char_boundary(start) && source.is_char_boundary(end)));
    if !valid {
        mismatches.record(
            format!("recovered:{origin}_span_invalid"),
            format!("{label}: {start}..{end} outside {} bytes", source.len()),
        );
    }
}
