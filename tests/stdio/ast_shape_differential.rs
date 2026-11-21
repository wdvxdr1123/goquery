use crate::go_corpus::{GoCorpus, MismatchBucket, OracleBinary};
use serde_json::Value;
use std::collections::BTreeMap;

type ShapeNode = (String, usize, usize);

/// Mirror of the oracle's detail limit and chunk size.
const SHAPE_NODE_DETAIL_LIMIT: usize = 8192;
const SHAPE_CHUNK_NODES: usize = 2048;

/// Explicit token budget for the view index in this differential: the pinned
/// corpus's largest generated file has 616,298 tokens (measured), just above
/// the server's 600,000-token budget. The server keeps its default; only this
/// bounded corpus comparison passes a larger budget.
const VIEW_GATE_TOKEN_BUDGET: usize = 1_000_000;

#[derive(Default)]
struct Stats {
    packages: usize,
    files: usize,
    bodies: usize,
    initializers: usize,
    types: usize,
    declarations: usize,
    digest_components: usize,
    digest_bodies: usize,
    digest_initializers: usize,
    digest_types: usize,
    digest_nodes: usize,
    max_component_nodes: usize,
    hard_truncated: usize,
    hard_truncated_examples: Vec<String>,
    view_files: usize,
    view_skipped: usize,
    view_budget_overrides: usize,
    view_imports: usize,
    view_comment_groups: usize,
    view_comment_spans: usize,
    view_raw_strings: usize,
    view_groups: usize,
    view_nodes: usize,
    view_skipped_examples: Vec<String>,
}

fn note_digest(stats: &mut Stats, section: &str, node_count: usize) {
    stats.digest_components += 1;
    stats.digest_nodes += node_count;
    match section {
        "body" => stats.digest_bodies += 1,
        "init" => stats.digest_initializers += 1,
        _ => stats.digest_types += 1,
    }
}

fn note_hard_truncated(stats: &mut Stats, section: &str, display: &str, start: usize) {
    stats.hard_truncated += 1;
    if stats.hard_truncated_examples.len() < 10 {
        stats
            .hard_truncated_examples
            .push(format!("{section}:{display}:{start}"));
    }
}

/// FNV-1a over `kind`, a NUL separator and little-endian start/end. This is
/// byte-identical to the Go oracle's `hash/fnv` digest.
fn shape_digest(nodes: &[ShapeNode]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for (kind, start, end) in nodes {
        for byte in kind.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash ^= 0;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        for byte in start.to_le_bytes().into_iter().chain(end.to_le_bytes()) {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    hash
}

fn compare_component_digest(
    section: &str,
    display: &str,
    base: usize,
    component: &Value,
    mut actual: Vec<ShapeNode>,
    stats: &mut Stats,
    mismatches: &mut MismatchBucket,
) {
    if section != "type" {
        for node in &mut actual {
            node.1 += base;
            node.2 += base;
        }
    }
    let node_count = component_node_count(component);
    note_digest(stats, section, node_count);
    digest_compare(
        section,
        display,
        &format!("byte {base}"),
        node_count,
        component["digests"].as_array().expect("digest chunks"),
        actual,
        mismatches,
    );
}

fn digest_compare(
    section: &str,
    display: &str,
    label: &str,
    node_count: usize,
    expected_chunks: &[Value],
    mut actual: Vec<ShapeNode>,
    mismatches: &mut MismatchBucket,
) {
    actual.sort();
    let expected: Vec<(usize, u64)> = expected_chunks
        .iter()
        .map(|chunk| {
            (
                chunk["count"].as_u64().expect("chunk count") as usize,
                chunk["hash"].as_u64().expect("chunk hash"),
            )
        })
        .collect();
    let actual_chunks: Vec<(usize, u64)> = actual
        .chunks(SHAPE_CHUNK_NODES)
        .map(|chunk| (chunk.len(), shape_digest(chunk)))
        .collect();
    if expected == actual_chunks && node_count == actual.len() {
        return;
    }
    let mut detail = format!(
        "{display} {label}: {node_count} expected nodes, {} actual",
        actual.len()
    );
    for (index, (expected_chunk, actual_chunk)) in expected.iter().zip(&actual_chunks).enumerate() {
        if expected_chunk == actual_chunk {
            continue;
        }
        let first = &expected_chunks[index]["first"];
        let last = &expected_chunks[index]["last"];
        detail.push_str(&format!(
            "; chunk {index}: expected {expected_chunk:?} ({first:?} .. {last:?}), actual {actual_chunk:?}"
        ));
        if let Some(node) = actual.get(index * SHAPE_CHUNK_NODES) {
            detail.push_str(&format!(", actual first {node:?}"));
        }
        break;
    }
    mismatches.record(format!("{section}:digest"), detail);
}

#[derive(Clone, Debug)]
struct Declaration {
    kind: String,
    name: String,
    start: usize,
    end: usize,
    name_start: usize,
    name_end: usize,
    generic: bool,
    alias: bool,
    method: bool,
    body: bool,
}

#[test]
#[ignore = "prepare the pinned Go source corpus and set GOQUERY_GO_SOURCE"]
fn go_source_ast_shape_corpus() {
    let corpus = GoCorpus::load();
    let limit = std::env::var("GOQUERY_SHAPE_LIMIT")
        .ok()
        .map(|value| value.parse::<usize>().expect("GOQUERY_SHAPE_LIMIT"));
    let oracle = OracleBinary::build("shape");
    let mut stats = Stats::default();
    let mut mismatches = MismatchBucket::default();
    let mut checked_packages = 0usize;
    for (package, mut files) in corpus.packages.clone() {
        if !corpus.package_enabled(&package, false, None) {
            continue;
        }
        if let Some(limit) = limit
            && checked_packages >= limit
        {
            break;
        }
        checked_packages += 1;
        files.sort();
        let reports = oracle.run("--shapes", &files);
        assert_eq!(reports.len(), files.len(), "oracle must inspect every file");
        let before = mismatches.total();
        for (report, file) in reports.iter().zip(&files) {
            assert_eq!(
                report["file"].as_str(),
                file.to_str(),
                "oracle report order"
            );
            if report["error"].as_str() != Some("") {
                // Acceptance is gated by go_source_package_corpus.
                continue;
            }
            let source = std::fs::read_to_string(file).expect("read corpus file");
            let display = file.display().to_string();
            stats.files += 1;
            check_declarations(&display, &source, report, &mut stats, &mut mismatches);
            check_bodies(&display, &source, report, &mut stats, &mut mismatches);
            check_initializers(&display, &source, report, &mut stats, &mut mismatches);
            check_types(&display, &source, report, &mut stats, &mut mismatches);
            check_view(&display, &source, report, &mut stats, &mut mismatches);
        }
        stats.packages += 1;
        let package_mismatches = mismatches.total() - before;
        eprintln!(
            "{package}: {} files, {package_mismatches} shape mismatches",
            files.len()
        );
    }
    assert!(
        stats.bodies > 0 && stats.types > 0 && stats.declarations > 0,
        "shape differential must inspect real syntax"
    );
    if let Some(path) = std::env::var_os("GOQUERY_SHAPE_REPORT") {
        let report = serde_json::json!({
            "version": "go1.27.3",
            "scope": corpus.scope,
            "packages": stats.packages,
            "files": stats.files,
            "bodies": stats.bodies,
            "initializers": stats.initializers,
            "types": stats.types,
            "declarations": stats.declarations,
            "digest_components": stats.digest_components,
            "digest_bodies": stats.digest_bodies,
            "digest_initializers": stats.digest_initializers,
            "digest_types": stats.digest_types,
            "digest_nodes": stats.digest_nodes,
            "max_component_nodes": stats.max_component_nodes,
            "hard_truncated_components": stats.hard_truncated,
            "hard_truncated_examples": stats.hard_truncated_examples,
            "view_files": stats.view_files,
            "view_budget_overrides": stats.view_budget_overrides,
            "view_skipped": stats.view_skipped,
            "view_imports": stats.view_imports,
            "view_comment_groups": stats.view_comment_groups,
            "view_comment_spans": stats.view_comment_spans,
            "view_raw_strings": stats.view_raw_strings,
            "view_groups": stats.view_groups,
            "view_nodes": stats.view_nodes,
            "view_skipped_examples": stats.view_skipped_examples,
            "mismatch_classes": mismatches.classes,
            "examples": mismatches.examples,
        });
        std::fs::write(
            path,
            serde_json::to_vec_pretty(&report).expect("encode shape report"),
        )
        .expect("write shape report");
    }
    eprintln!(
        "AST shape differential: {} packages, {} files, {} bodies, {} initializers, {} types, {} declarations; {} oversized components compared by chunk digest ({} bodies, {} initializers, {} types; {} nodes; max component {} nodes); {} hard-truncated components",
        stats.packages,
        stats.files,
        stats.bodies,
        stats.initializers,
        stats.types,
        stats.declarations,
        stats.digest_components,
        stats.digest_bodies,
        stats.digest_initializers,
        stats.digest_types,
        stats.digest_nodes,
        stats.max_component_nodes,
        stats.hard_truncated
    );
    eprintln!(
        "View differential: {} files ({} with an explicit larger token budget), {} imports, {} comment groups, {} comment spans, {} raw strings, {} folding groups, {} folding nodes; {} files over the view limits",
        stats.view_files,
        stats.view_budget_overrides,
        stats.view_imports,
        stats.view_comment_groups,
        stats.view_comment_spans,
        stats.view_raw_strings,
        stats.view_groups,
        stats.view_nodes,
        stats.view_skipped
    );
    mismatches.assert_empty("AST shape divergences");
}

fn check_declarations(
    display: &str,
    source: &str,
    report: &Value,
    stats: &mut Stats,
    mismatches: &mut MismatchBucket,
) {
    let stub = goquery::syntax::stub(source);
    let expected: Vec<Declaration> = report["declarations"]
        .as_array()
        .expect("oracle declarations")
        .iter()
        .map(|value| Declaration {
            kind: value["kind"].as_str().expect("declaration kind").to_owned(),
            name: value["name"].as_str().expect("declaration name").to_owned(),
            start: value["start"].as_u64().expect("declaration start") as usize,
            end: value["end"].as_u64().expect("declaration end") as usize,
            name_start: value["name_start"].as_u64().expect("name start") as usize,
            name_end: value["name_end"].as_u64().expect("name end") as usize,
            generic: value["generic"].as_bool().expect("declaration generic"),
            alias: value["alias"].as_bool().expect("declaration alias"),
            method: value["method"].as_bool().expect("declaration method"),
            body: value["body"].as_bool().expect("declaration body"),
        })
        .collect();
    let actual: Vec<Declaration> = stub
        .symbols
        .iter()
        .map(|symbol| {
            use goquery::syntax::SymbolKind;
            let kind = match symbol.kind {
                SymbolKind::Type => "type",
                SymbolKind::Constant => "const",
                SymbolKind::Variable => "var",
                SymbolKind::Function => "func",
                SymbolKind::Method => "method",
            };
            Declaration {
                kind: kind.to_owned(),
                name: symbol.name.clone(),
                start: symbol.range.start,
                end: symbol.range.end,
                name_start: symbol.selection.start,
                name_end: symbol.selection.end,
                generic: symbol.generic,
                alias: symbol.alias,
                method: symbol.kind == SymbolKind::Method,
                body: symbol.body.is_some(),
            }
        })
        .collect();
    stats.declarations += expected.len();
    compare_declarations(display, &expected, &actual, mismatches);
}

fn compare_declarations(
    display: &str,
    expected: &[Declaration],
    actual: &[Declaration],
    mismatches: &mut MismatchBucket,
) {
    let key = |declaration: &Declaration| {
        (
            declaration.start,
            declaration.name_start,
            declaration.name.clone(),
        )
    };
    let expected_by_key: BTreeMap<_, _> = expected.iter().map(|d| (key(d), d)).collect();
    let actual_by_key: BTreeMap<_, _> = actual.iter().map(|d| (key(d), d)).collect();
    for (key, expected) in &expected_by_key {
        let Some(actual) = actual_by_key.get(key) else {
            mismatches.record(
                "decl:missing",
                format!(
                    "{display} {} {} at byte {}",
                    expected.kind, expected.name, key.0
                ),
            );
            continue;
        };
        if expected.kind != actual.kind {
            mismatches.record(
                "decl:kind",
                format!(
                    "{display} {}: {} vs {}",
                    expected.name, expected.kind, actual.kind
                ),
            );
        }
        if expected.end != actual.end {
            mismatches.record(
                "decl:end",
                format!(
                    "{display} {}: end {} vs {}",
                    expected.name, expected.end, actual.end
                ),
            );
        }
        if expected.name_end != actual.name_end {
            mismatches.record(
                "decl:name_end",
                format!(
                    "{display} {}: name end {} vs {}",
                    expected.name, expected.name_end, actual.name_end
                ),
            );
        }
        for (class, expected_flag, actual_flag) in [
            ("decl:generic", expected.generic, actual.generic),
            ("decl:alias", expected.alias, actual.alias),
            ("decl:method", expected.method, actual.method),
            ("decl:body", expected.body, actual.body),
        ] {
            if expected_flag != actual_flag {
                mismatches.record(
                    class,
                    format!(
                        "{display} {}: {expected_flag} vs {actual_flag}",
                        expected.name
                    ),
                );
            }
        }
    }
    for (key, actual) in &actual_by_key {
        if !expected_by_key.contains_key(key) {
            mismatches.record(
                "decl:extra",
                format!(
                    "{display} {} {} at byte {}",
                    actual.kind, actual.name, key.0
                ),
            );
        }
    }
}

fn component_node_count(component: &Value) -> usize {
    component["node_count"].as_u64().expect("node count") as usize
}

fn component_has_digests(component: &Value) -> bool {
    let digests = component["digests"]
        .as_array()
        .is_some_and(|digests| !digests.is_empty());
    debug_assert!(
        !digests || component_node_count(component) > SHAPE_NODE_DETAIL_LIMIT,
        "oracle only digests components above the detail limit"
    );
    digests
}

fn compare_component(
    section: &str,
    display: &str,
    start: usize,
    component: &Value,
    actual: Vec<ShapeNode>,
    stats: &mut Stats,
    mismatches: &mut MismatchBucket,
) {
    if component_has_digests(component) {
        compare_component_digest(
            section, display, start, component, actual, stats, mismatches,
        );
    } else {
        let expected = if section == "type" {
            component_nodes(component)
        } else {
            relative_nodes(component, start)
        };
        compare_nodes(section, display, start, &expected, &actual, mismatches);
    }
}

fn check_bodies(
    display: &str,
    source: &str,
    report: &Value,
    stats: &mut Stats,
    mismatches: &mut MismatchBucket,
) {
    for component in report["bodies"].as_array().expect("oracle body components") {
        let start = component["start"].as_u64().expect("body start") as usize;
        stats.max_component_nodes = stats
            .max_component_nodes
            .max(component_node_count(component));
        if component["truncated"].as_bool() == Some(true) {
            note_hard_truncated(stats, "body", display, start);
            continue;
        }
        let end = component["end"].as_u64().expect("body end") as usize;
        let text = source.get(start..end).expect("body source");
        let tree = goquery::syntax::parse_block(text);
        if !tree.diagnostics().is_empty() {
            mismatches.record(
                "body:diagnostics",
                format!("{display} byte {start}: {:?}", tree.diagnostics()),
            );
            continue;
        }
        compare_component(
            "body",
            display,
            start,
            component,
            block_shape(&tree, text),
            stats,
            mismatches,
        );
        stats.bodies += 1;
    }
}

fn check_initializers(
    display: &str,
    source: &str,
    report: &Value,
    stats: &mut Stats,
    mismatches: &mut MismatchBucket,
) {
    for component in report["initializers"]
        .as_array()
        .expect("oracle initializer components")
    {
        let start = component["start"].as_u64().expect("initializer start") as usize;
        stats.max_component_nodes = stats
            .max_component_nodes
            .max(component_node_count(component));
        if component["truncated"].as_bool() == Some(true) {
            note_hard_truncated(stats, "init", display, start);
            continue;
        }
        let end = component["end"].as_u64().expect("initializer end") as usize;
        let text = source.get(start..end).expect("initializer source");
        let tree = goquery::syntax::parse_expression(text);
        if !tree.diagnostics().is_empty() {
            mismatches.record(
                "init:diagnostics",
                format!("{display} byte {start}: {:?}", tree.diagnostics()),
            );
            continue;
        }
        let mut nodes = Vec::new();
        expression_shape(tree.nodes(), tree.root().index(), text, &mut nodes);
        compare_component("init", display, start, component, nodes, stats, mismatches);
        stats.initializers += 1;
    }
}

fn check_types(
    display: &str,
    source: &str,
    report: &Value,
    stats: &mut Stats,
    mismatches: &mut MismatchBucket,
) {
    for component in report["types"].as_array().expect("oracle type components") {
        let start = component["start"].as_u64().expect("type start") as usize;
        stats.max_component_nodes = stats
            .max_component_nodes
            .max(component_node_count(component));
        if component["truncated"].as_bool() == Some(true) {
            note_hard_truncated(stats, "type", display, start);
            continue;
        }
        let end = component["end"].as_u64().expect("type end") as usize;
        let method = component["method"].as_bool() == Some(true);
        let text = source.get(start..end).expect("type source");
        let synthetic;
        let parsed_text = if method {
            synthetic = format!("func{text}");
            synthetic.as_str()
        } else {
            text
        };
        let parsed = goquery::syntax::parse_type(parsed_text);
        if !parsed.diagnostics().is_empty() {
            mismatches.record(
                "type:diagnostics",
                format!("{display} byte {start}: {:?}", parsed.diagnostics()),
            );
            continue;
        }
        if parsed.span != (0..parsed_text.len()) {
            mismatches.record(
                "type:span",
                format!(
                    "{display} byte {start} method {method}: {:?} != {:?}",
                    parsed.span,
                    0..parsed_text.len()
                ),
            );
        }
        compare_component(
            "type",
            display,
            start,
            component,
            type_shape(parsed.type_nodes()),
            stats,
            mismatches,
        );
        stats.types += 1;
    }
}

fn component_nodes(component: &Value) -> Vec<ShapeNode> {
    component["nodes"]
        .as_array()
        .map(|nodes| {
            nodes
                .iter()
                .map(|node| {
                    (
                        node["kind"].as_str().expect("shape kind").to_owned(),
                        node["start"].as_u64().expect("shape start") as usize,
                        node["end"].as_u64().expect("shape end") as usize,
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

fn relative_nodes(component: &Value, base: usize) -> Vec<ShapeNode> {
    component_nodes(component)
        .into_iter()
        .map(|(kind, start, end)| (kind, start.saturating_sub(base), end.saturating_sub(base)))
        .collect()
}

fn compare_nodes(
    section: &str,
    display: &str,
    base: usize,
    expected: &[ShapeNode],
    actual: &[ShapeNode],
    mismatches: &mut MismatchBucket,
) {
    let mut expected_sorted = expected.to_vec();
    expected_sorted.sort();
    let mut actual_sorted = actual.to_vec();
    actual_sorted.sort();
    if expected_sorted == actual_sorted {
        return;
    }
    let mut expected_counts: BTreeMap<ShapeNode, usize> = BTreeMap::new();
    for node in &expected_sorted {
        *expected_counts.entry(node.clone()).or_default() += 1;
    }
    let mut actual_counts: BTreeMap<ShapeNode, usize> = BTreeMap::new();
    for node in &actual_sorted {
        *actual_counts.entry(node.clone()).or_default() += 1;
    }
    let mut missing: Vec<ShapeNode> = Vec::new();
    let mut extra: Vec<ShapeNode> = Vec::new();
    for (node, count) in &expected_counts {
        let present = actual_counts.get(node).copied().unwrap_or(0);
        for _ in present..*count {
            missing.push(node.clone());
        }
    }
    for (node, count) in &actual_counts {
        let present = expected_counts.get(node).copied().unwrap_or(0);
        for _ in present..*count {
            extra.push(node.clone());
        }
    }
    missing.sort();
    extra.sort();
    let mut extra_used = vec![false; extra.len()];
    let mut missing_used = vec![false; missing.len()];
    if missing.len().saturating_mul(extra.len()) <= 1_000_000 {
        for (missing_index, missing_node) in missing.iter().enumerate() {
            for (extra_index, extra_node) in extra.iter().enumerate() {
                if !extra_used[extra_index]
                    && extra_node.0 == missing_node.0
                    && extra_node.1 == missing_node.1
                {
                    extra_used[extra_index] = true;
                    missing_used[missing_index] = true;
                    mismatches.record(
                        format!("{section}:span:{}", missing_node.0),
                        format!(
                            "{display} byte {base}: expected {missing_node:?} actual {extra_node:?}"
                        ),
                    );
                    break;
                }
            }
        }
    }
    for (index, node) in missing.iter().enumerate() {
        if !missing_used[index] {
            mismatches.record(
                format!("{section}:missing:{}", node.0),
                format!("{display} byte {base}: expected {node:?}"),
            );
        }
    }
    for (index, node) in extra.iter().enumerate() {
        if !extra_used[index] {
            mismatches.record(
                format!("{section}:extra:{}", node.0),
                format!("{display} byte {base}: unexpected {node:?}"),
            );
        }
    }
}

fn block_shape(tree: &goquery::syntax::BlockTree, source: &str) -> Vec<ShapeNode> {
    let mut nodes = Vec::new();
    if let Some(root) = tree.statement(tree.root()) {
        statement_shape(tree, root, source, &mut nodes);
    }
    nodes
}

fn statement_shape(
    tree: &goquery::syntax::BlockTree,
    statement: &goquery::syntax::Statement,
    source: &str,
    nodes: &mut Vec<ShapeNode>,
) {
    use goquery::syntax::StmtKind;
    let span = &statement.span;
    let push = |nodes: &mut Vec<ShapeNode>, kind: String| {
        nodes.push((kind, span.start, span.end));
    };
    match &statement.kind {
        StmtKind::Block { statements } => {
            push(nodes, "block".to_owned());
            for id in statements {
                child_statement(tree, *id, source, nodes);
            }
        }
        StmtKind::Expression { expression } => {
            push(nodes, "expr".to_owned());
            child_expression(tree, *expression, source, nodes);
        }
        StmtKind::Assignment {
            left,
            operator,
            right,
        } => {
            push(nodes, format!("assign:{}", &source[operator.clone()]));
            for id in left.iter().chain(right) {
                child_expression(tree, *id, source, nodes);
            }
        }
        StmtKind::Return { values } => {
            push(nodes, "return".to_owned());
            for id in values {
                child_expression(tree, *id, source, nodes);
            }
        }
        StmtKind::Increment {
            expression,
            operator,
        } => {
            push(nodes, format!("incdec:{}", &source[operator.clone()]));
            child_expression(tree, *expression, source, nodes);
        }
        StmtKind::Send { channel, value } => {
            push(nodes, "send".to_owned());
            child_expression(tree, *channel, source, nodes);
            child_expression(tree, *value, source, nodes);
        }
        StmtKind::Go { call } => {
            push(nodes, "go".to_owned());
            child_expression(tree, *call, source, nodes);
        }
        StmtKind::Defer { call } => {
            push(nodes, "defer".to_owned());
            child_expression(tree, *call, source, nodes);
        }
        StmtKind::Declaration {
            keyword,
            annotation,
            values,
            ..
        } => {
            push(nodes, format!("decl:{}", &source[keyword.clone()]));
            if let Some(annotation) = annotation {
                nodes.extend(inline_type_shape(source, annotation));
            }
            for id in values {
                child_expression(tree, *id, source, nodes);
            }
        }
        StmtKind::Missing => {}
        StmtKind::DeclarationGroup { declarations } => {
            for id in declarations {
                child_statement(tree, *id, source, nodes);
            }
        }
        StmtKind::Select { cases } => {
            push(nodes, "select".to_owned());
            for id in cases {
                child_statement(tree, *id, source, nodes);
            }
        }
        StmtKind::CommunicationCase {
            communication,
            statements,
        } => {
            push(nodes, "commcase".to_owned());
            if let Some(id) = communication {
                child_statement(tree, *id, source, nodes);
            }
            for id in statements {
                child_statement(tree, *id, source, nodes);
            }
        }
        StmtKind::RangeClause { left, value, .. } => {
            for id in left {
                child_expression(tree, *id, source, nodes);
            }
            child_expression(tree, *value, source, nodes);
        }
        StmtKind::RangeFor { clause, body } => {
            let token = match tree.statement(*clause).map(|clause| &clause.kind) {
                Some(StmtKind::RangeClause { operator, .. }) => operator
                    .as_ref()
                    .map_or("illegal", |operator| &source[operator.clone()]),
                _ => "illegal",
            };
            push(nodes, format!("rangefor:{token}"));
            child_statement(tree, *clause, source, nodes);
            child_statement(tree, *body, source, nodes);
        }
        StmtKind::Branch { keyword, label } => {
            let mut kind = format!("branch:{}", keyword.fixed_text().unwrap_or("unknown"));
            if let Some(label) = label {
                kind.push(':');
                kind.push_str(&source[label.clone()]);
            }
            push(nodes, kind);
        }
        StmtKind::Label { name, statement } => {
            push(nodes, format!("label:{}", &source[name.clone()]));
            if let Some(id) = statement {
                child_statement(tree, *id, source, nodes);
            }
        }
        StmtKind::If { .. } => {
            let mut current = statement;
            loop {
                let span = &current.span;
                nodes.push(("if".to_owned(), span.start, span.end));
                let StmtKind::If {
                    init,
                    condition,
                    then_block,
                    otherwise,
                } = &current.kind
                else {
                    break;
                };
                if let Some(id) = init {
                    child_statement(tree, *id, source, nodes);
                }
                child_expression(tree, *condition, source, nodes);
                child_statement(tree, *then_block, source, nodes);
                let Some(id) = otherwise else {
                    break;
                };
                match tree.statement(*id) {
                    Some(next) if matches!(next.kind, StmtKind::If { .. }) => current = next,
                    _ => {
                        child_statement(tree, *id, source, nodes);
                        break;
                    }
                }
            }
        }
        StmtKind::For {
            init,
            condition,
            post,
            body,
        } => {
            push(nodes, "for".to_owned());
            if let Some(id) = init {
                child_statement(tree, *id, source, nodes);
            }
            if let Some(id) = condition {
                child_expression(tree, *id, source, nodes);
            }
            if let Some(id) = post {
                child_statement(tree, *id, source, nodes);
            }
            child_statement(tree, *body, source, nodes);
        }
        StmtKind::Switch { init, tag, cases } => {
            push(nodes, "switch".to_owned());
            if let Some(id) = init {
                child_statement(tree, *id, source, nodes);
            }
            if let Some(id) = tag {
                child_expression(tree, *id, source, nodes);
            }
            for id in cases {
                child_statement(tree, *id, source, nodes);
            }
        }
        StmtKind::TypeSwitch { init, guard, cases } => {
            push(nodes, "typeswitch".to_owned());
            if let Some(id) = init {
                child_statement(tree, *id, source, nodes);
            }
            child_statement(tree, *guard, source, nodes);
            for id in cases {
                child_statement(tree, *id, source, nodes);
            }
        }
        StmtKind::Case {
            values, statements, ..
        } => {
            push(nodes, "case".to_owned());
            for id in values {
                child_expression(tree, *id, source, nodes);
            }
            for id in statements {
                child_statement(tree, *id, source, nodes);
            }
        }
        _ => push(nodes, "unknown".to_owned()),
    }
}

fn child_statement(
    tree: &goquery::syntax::BlockTree,
    id: goquery::syntax::StmtId,
    source: &str,
    nodes: &mut Vec<ShapeNode>,
) {
    if let Some(statement) = tree.statement(id) {
        statement_shape(tree, statement, source, nodes);
    }
}

fn child_expression(
    tree: &goquery::syntax::BlockTree,
    id: goquery::syntax::ExprId,
    source: &str,
    nodes: &mut Vec<ShapeNode>,
) {
    expression_shape(tree.expressions(), id.index(), source, nodes);
}

fn expression_shape(
    expressions: &[goquery::syntax::Expr],
    root: usize,
    source: &str,
    nodes: &mut Vec<ShapeNode>,
) {
    use goquery::syntax::ExprKind;
    let mut pending = vec![root];
    while let Some(index) = pending.pop() {
        let Some(expression) = expressions.get(index) else {
            nodes.push(("unknown".to_owned(), 0, 0));
            continue;
        };
        let span = &expression.span;
        let push = |nodes: &mut Vec<ShapeNode>, kind: String| {
            nodes.push((kind, span.start, span.end));
        };
        match &expression.kind {
            ExprKind::Name => push(nodes, "name".to_owned()),
            ExprKind::Literal(_) => push(nodes, "literal".to_owned()),
            ExprKind::Missing => {}
            ExprKind::Type => {
                let within = inline_type_shape(source, span);
                if within.is_empty() {
                    push(nodes, "type".to_owned());
                } else {
                    nodes.extend(within);
                }
            }
            ExprKind::Composite {
                literal_type,
                elements,
            } => {
                push(nodes, "composite".to_owned());
                if let Some(id) = literal_type {
                    pending.push(id.index());
                }
                for id in elements {
                    pending.push(id.index());
                }
            }
            ExprKind::KeyValue { key, value } => {
                push(nodes, "keyvalue".to_owned());
                pending.push(key.index());
                pending.push(value.index());
            }
            ExprKind::FunctionLiteral { signature, .. } => {
                push(nodes, "funclit".to_owned());
                nodes.extend(inline_type_shape(source, signature));
            }
            ExprKind::Unary { operator, operand } => {
                push(nodes, format!("unary:{}", &source[operator.clone()]));
                pending.push(operand.index());
            }
            ExprKind::Binary {
                left,
                operator,
                right,
            } => {
                push(nodes, format!("binary:{}", &source[operator.clone()]));
                pending.push(left.index());
                pending.push(right.index());
            }
            ExprKind::Parenthesized { expression } => {
                push(nodes, "paren".to_owned());
                pending.push(expression.index());
            }
            ExprKind::Selector { operand, .. } => {
                push(nodes, "selector".to_owned());
                pending.push(operand.index());
            }
            ExprKind::Call {
                function,
                arguments,
                variadic,
                ..
            } => {
                push(
                    nodes,
                    (if *variadic { "call..." } else { "call" }).to_owned(),
                );
                pending.push(function.index());
                for id in arguments {
                    pending.push(id.index());
                }
            }
            ExprKind::Index { operand, arguments } => {
                push(nodes, "index".to_owned());
                pending.push(operand.index());
                for id in arguments {
                    pending.push(id.index());
                }
            }
            ExprKind::Slice {
                operand,
                low,
                high,
                max,
                ..
            } => {
                push(nodes, "slice".to_owned());
                pending.push(operand.index());
                for id in [low, high, max].into_iter().flatten() {
                    pending.push(id.index());
                }
            }
            ExprKind::TypeAssertion {
                operand,
                asserted_type,
            } => {
                push(nodes, "typeassert".to_owned());
                pending.push(operand.index());
                nodes.extend(inline_type_shape(source, asserted_type));
            }
            ExprKind::TypeSwitchGuard { operand } => {
                push(nodes, "typeassert".to_owned());
                pending.push(operand.index());
            }
            _ => push(nodes, "unknown".to_owned()),
        }
    }
}

fn inline_type_shape(source: &str, span: &std::ops::Range<usize>) -> Vec<ShapeNode> {
    let Some(text) = source.get(span.clone()) else {
        return Vec::new();
    };
    if text.is_empty() {
        return Vec::new();
    }
    let parsed = goquery::syntax::parse_type(text);
    if !parsed.diagnostics().is_empty() {
        return Vec::new();
    }
    parsed
        .type_nodes()
        .iter()
        .map(|node| {
            (
                type_node_kind(node.kind).to_owned(),
                span.start + node.span.start,
                span.start + node.span.end,
            )
        })
        .collect()
}

fn type_shape(nodes: &[goquery::syntax::TypeNode]) -> Vec<ShapeNode> {
    nodes
        .iter()
        .map(|node| {
            (
                type_node_kind(node.kind).to_owned(),
                node.span.start,
                node.span.end,
            )
        })
        .collect()
}

fn type_node_kind(kind: goquery::syntax::TypeNodeKind) -> &'static str {
    use goquery::syntax::TypeNodeKind;
    match kind {
        TypeNodeKind::Name => "name",
        TypeNodeKind::Qualified => "selector",
        TypeNodeKind::Instantiation => "instantiation",
        TypeNodeKind::Pointer => "pointer",
        TypeNodeKind::Array => "array",
        TypeNodeKind::Slice => "slice",
        TypeNodeKind::Map => "map",
        TypeNodeKind::Chan => "chan",
        TypeNodeKind::SendChan => "sendchan",
        TypeNodeKind::RecvChan => "recvchan",
        TypeNodeKind::Function => "function",
        TypeNodeKind::Struct => "struct",
        TypeNodeKind::Interface => "interface",
        TypeNodeKind::Parenthesized => "paren",
        _ => "unknown",
    }
}

fn check_view(
    display: &str,
    source: &str,
    report: &Value,
    stats: &mut Stats,
    mismatches: &mut MismatchBucket,
) {
    let index = match goquery::syntax::view::SyntaxIndex::parse(source) {
        Some(index) => index,
        None => match goquery::syntax::view::SyntaxIndex::parse_with_token_budget(
            source,
            VIEW_GATE_TOKEN_BUDGET,
        ) {
            Some(index) => {
                stats.view_budget_overrides += 1;
                index
            }
            None => {
                stats.view_skipped += 1;
                if stats.view_skipped_examples.len() < 10 {
                    stats.view_skipped_examples.push(display.to_owned());
                }
                return;
            }
        },
    };
    stats.view_files += 1;
    let view = &report["view"];
    let expected_imports: Vec<String> = view["imports"]
        .as_array()
        .expect("view imports")
        .iter()
        .map(|entry| {
            format!(
                "{}..{}:{}:{}",
                entry["start"].as_u64().expect("import start"),
                entry["end"].as_u64().expect("import end"),
                entry["name"].as_str().expect("import name"),
                entry["path"].as_str().expect("import path"),
            )
        })
        .collect();
    let actual_imports: Vec<String> = index
        .imports()
        .iter()
        .map(|entry| {
            use goquery::syntax::ImportName;
            let name = match &entry.name {
                ImportName::Package => "",
                ImportName::Dot => ".",
                ImportName::Blank => "_",
                ImportName::Alias(alias) => alias.as_str(),
            };
            format!(
                "{}..{}:{}:{}",
                entry.span.start, entry.span.end, name, entry.path
            )
        })
        .collect();
    stats.view_imports += expected_imports.len();
    compare_multiset(
        "view:import",
        display,
        &expected_imports,
        &actual_imports,
        mismatches,
    );

    let expected_groups: Vec<String> = view["comment_groups"]
        .as_array()
        .expect("view comment groups")
        .iter()
        .map(|group| {
            format!(
                "{}..{}:{}..{}",
                group["group"]["start"]
                    .as_u64()
                    .expect("comment group start"),
                group["group"]["end"].as_u64().expect("comment group end"),
                group["first"]["start"]
                    .as_u64()
                    .expect("comment first start"),
                group["first"]["end"].as_u64().expect("comment first end"),
            )
        })
        .collect();
    let actual_groups: Vec<String> = index
        .comments()
        .iter()
        .map(|comment| {
            format!(
                "{}..{}:{}..{}",
                comment.group.start, comment.group.end, comment.first.start, comment.first.end
            )
        })
        .collect();
    stats.view_comment_groups += expected_groups.len();
    compare_multiset(
        "view:comment_group",
        display,
        &expected_groups,
        &actual_groups,
        mismatches,
    );

    let expected_spans = span_nodes(&view["comment_spans"], "comment_spans", "comment");
    let actual_spans: Vec<ShapeNode> = index
        .comment_spans()
        .iter()
        .map(|span| ("comment".to_owned(), span.start, span.end))
        .collect();
    stats.view_comment_spans += expected_spans.len();
    compare_nodes(
        "view:comment_span",
        display,
        0,
        &expected_spans,
        &actual_spans,
        mismatches,
    );

    let expected_raw = span_nodes(&view["raw_strings"], "raw_strings", "raw");
    let actual_raw: Vec<ShapeNode> = index
        .raw_strings()
        .iter()
        .map(|span| ("raw".to_owned(), span.start, span.end))
        .collect();
    stats.view_raw_strings += expected_raw.len();
    compare_nodes(
        "view:raw_string",
        display,
        0,
        &expected_raw,
        &actual_raw,
        mismatches,
    );

    let actual_group_nodes: Vec<ShapeNode> = index
        .groups()
        .iter()
        .map(|group| {
            use goquery::syntax::view::FoldGroupKind;
            let kind = match group.kind {
                FoldGroupKind::FieldList => "fieldlist",
                FoldGroupKind::Decl => "decl",
                FoldGroupKind::Imports => "imports",
                _ => "unknown",
            };
            (kind.to_owned(), group.open.start, group.close.end)
        })
        .collect();
    stats.view_groups += compare_shape_list(
        "view:group",
        display,
        &view["groups"],
        actual_group_nodes,
        mismatches,
    );

    let actual_fold_nodes: Vec<ShapeNode> = index
        .nodes()
        .iter()
        .map(|node| {
            use goquery::syntax::view::FoldNodeKind;
            let kind = match node.kind {
                FoldNodeKind::Block => "block",
                FoldNodeKind::Composite => "composite",
                FoldNodeKind::Call => "call",
                FoldNodeKind::Case => "case",
                FoldNodeKind::Comm => "comm",
                _ => "unknown",
            };
            (kind.to_owned(), node.open, node.close)
        })
        .collect();
    stats.view_nodes += compare_shape_list(
        "view:node",
        display,
        &view["nodes"],
        actual_fold_nodes,
        mismatches,
    );
}

fn span_nodes(value: &Value, label: &str, kind: &str) -> Vec<ShapeNode> {
    value
        .as_array()
        .unwrap_or_else(|| panic!("view {label}"))
        .iter()
        .map(|span| {
            (
                kind.to_owned(),
                span["start"].as_u64().expect("span start") as usize,
                span["end"].as_u64().expect("span end") as usize,
            )
        })
        .collect()
}

fn compare_multiset(
    section: &str,
    display: &str,
    expected: &[String],
    actual: &[String],
    mismatches: &mut MismatchBucket,
) {
    let mut expected_sorted = expected.to_vec();
    expected_sorted.sort();
    let mut actual_sorted = actual.to_vec();
    actual_sorted.sort();
    if expected_sorted == actual_sorted {
        return;
    }
    let mut expected_counts: BTreeMap<&str, usize> = BTreeMap::new();
    for key in &expected_sorted {
        *expected_counts.entry(key.as_str()).or_default() += 1;
    }
    let mut actual_counts: BTreeMap<&str, usize> = BTreeMap::new();
    for key in &actual_sorted {
        *actual_counts.entry(key.as_str()).or_default() += 1;
    }
    let mut missing: Vec<&str> = Vec::new();
    let mut extra: Vec<&str> = Vec::new();
    for (key, count) in &expected_counts {
        let present = actual_counts.get(key).copied().unwrap_or(0);
        for _ in present..*count {
            missing.push(key);
        }
    }
    for (key, count) in &actual_counts {
        let present = expected_counts.get(key).copied().unwrap_or(0);
        for _ in present..*count {
            extra.push(key);
        }
    }
    missing.sort();
    extra.sort();
    mismatches.record(
        section,
        format!(
            "{display}: missing {:?}, extra {:?}",
            &missing[..missing.len().min(3)],
            &extra[..extra.len().min(3)]
        ),
    );
}

fn compare_shape_list(
    section: &str,
    display: &str,
    list: &Value,
    mut actual: Vec<ShapeNode>,
    mismatches: &mut MismatchBucket,
) -> usize {
    let node_count = list["node_count"].as_u64().expect("shape list node count") as usize;
    if list["truncated"].as_bool() == Some(true) {
        mismatches.record(format!("{section}:hard_truncated"), display.to_owned());
        return node_count;
    }
    if list["nodes"].as_array().is_none() && node_count > 0 {
        digest_compare(
            section,
            display,
            "",
            node_count,
            list["digests"].as_array().expect("shape list digests"),
            actual,
            mismatches,
        );
    } else {
        actual.sort();
        let expected = component_nodes(list);
        compare_nodes(section, display, 0, &expected, &actual, mismatches);
    }
    node_count
}
