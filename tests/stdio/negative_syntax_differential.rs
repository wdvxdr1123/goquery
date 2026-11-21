use crate::go_corpus::{GoCorpus, MismatchBucket, OracleBinary};
use serde_json::Value;
use std::collections::BTreeMap;
use std::ops::Range;
use std::path::PathBuf;

/// Deterministic caps, mirrored in the gate documentation.
const MAX_SEED_FILES: usize = 160;
const MAX_SEED_BYTES: usize = 32 * 1024;
const CHAR_MUTATIONS_PER_FILE: usize = 6;
const TOKEN_MUTATIONS_PER_FILE: usize = 4;
const MAX_MUTATION_SOURCES: usize = 3072;
const MAX_MUTATION_BYTES: usize = 32 * 1024 * 1024;
const MAX_BROAD_MUTATIONS: usize = 8192;
const MUTATION_CHUNK: usize = 128;
const MAX_REJECTED_FILE_BYTES: usize = 4 * 1024 * 1024;
const MAX_DIAGNOSTICS: usize = 4096;

#[derive(Default)]
struct Stats {
    packages: usize,
    files: usize,
    accepted: usize,
    rejected: usize,
    rejected_compared: usize,
    rejected_skipped: usize,
    mutations: usize,
    mutations_accepted: usize,
    mutations_rejected: usize,
    broad_mutations: usize,
    broad_accepted: usize,
    diagnostic_count: usize,
    snippets: usize,
    snippets_accepted: usize,
    stress: usize,
    stress_nesting_bound: usize,
    native_first_before: usize,
    native_first_equal: usize,
    native_first_after: usize,
    native_first_unknown: usize,
}

/// Deterministic grammar-boundary samples, aimed at acceptance parity for
/// recovery-sensitive constructs (semicolon insertion, generics ambiguity,
/// clause forms, variadics and delimiters). Both valid and invalid sources.
const CURATED_SNIPPETS: &[&str] = &[
    "package p\nfunc F() { L: }\n",
    "package p\nfunc F() { for range 10 {} }\n",
    "package p\nfunc F() { for i := range 10 { _ = i } }\n",
    "package p\nfunc F[P *S]() {}\ntype S struct{}\n",
    "package p\ntype T[P (any)] struct{}\n",
    "package p\ntype T[P interface{ ~int | ~string }] struct{}\n",
    "package p\nfunc F(...int) {}\n",
    "package p\nfunc F(int, ...string) {}\n",
    "package p\nfunc F() (x, y int) { return }\n",
    "package p\nfunc F() { if x := 1; x > 0 {} }\n",
    "package p\nfunc F() { switch x := 1; x { case 1: } }\n",
    "package p\nfunc F() { switch x := any(1).(type) { case int: _ = x } }\n",
    "package p\nfunc F() { select { case <-ch: } }\n",
    "package p\nfunc F() { go func() {}() }\n",
    "package p\nvar _ = map[string]int{\"a\": 1}\n",
    "package p\nvar _ = [...]int{1, 2}\n",
    "package p\ntype T = func(int) (string)\n",
    "package p\nfunc F() { _ = func() (int, error) { return 0, nil } }\n",
    "package p\ntype I interface{ M(a, b int) (c, d string); ~[]byte | string }\n",
    "package p\nvar _ = (<-chan int)(nil)\n",
    "package p\nfunc F() { x, ok := y.(T); _ = x; _ = ok }\n",
    "package p\nfunc F() { switch { case true: fallthrough; default: } }\n",
    "package p\nfunc F() { a := b\n\t; c := d }\n",
    "package p\nfunc F() { ; ; }\n",
    "package p\ntype S struct { A, B int; *T }\ntype T struct{}\n",
    "\u{feff}package p\n",
    "package p\r\nfunc F() {\r\n}\r\n",
    "package p\nfunc F() { if }\n",
    "package p\nfunc F() { for range }\n",
    "package p\nfunc F() { go f }\n",
    "package p\nfunc F() { defer 1 }\n",
    "package p\nfunc F() { x := }\n",
    "package p\nfunc F() { switch { case: } }\n",
    "package p\nfunc F() { case 1: }\n",
    "package p\nfunc F() { select { case: } }\n",
    "package p\nfunc F[P]() {}\n",
    "package p\ntype T struct { A, B }\n",
    "package p\ntype T func(...)\n",
    "package p\nfunc F(a, ...int) {}\n",
    "package p\nfunc F() { goto }\n",
    "package p\nfunc F() { for ; ; ; {} }\n",
    "package p\nfunc F() { if x { } else if { } }\n",
    "package p\nfunc F() { _ = []int{1,,2} }\n",
    "package p\nvar _ = T{1: }\n",
    "package p\nfunc F() { _ = x[1:2:3:4] }\n",
    "package p\nfunc F() { _ = func( { } }\n",
    "package p\nfunc F() { var x = }",
    "package p\rfunc F() {}\n",
    "package p\nvar x = \"unterminated\n",
    "package p\nvar x = 'ab'\n",
    "package p\nvar x = 0x\n",
    "package p\nfunc F() { break label }\n",
    "package p\nfunc F() { continue Outer }\n",
    "package p\nfunc F() { 1 = x }\n",
    "package p\nfunc F() { x.(int) }\n",
    "package p\ntype T[P []E] struct{}\ntype E int\n",
    "package p\ntype T[P [3]E] struct{}\ntype E int\n",
    "package p\ntype T[P <-chan E] struct{}\ntype E int\n",
    "package p\ntype T[P *[]E] struct{}\ntype E int\n",
    "package p\ntype ( T[P []E] struct{} )\ntype E int\n",
    "package p\nvar x = \"\\q\"\n",
    "package p\nvar x = 0779\n",
    "package p\nvar x = 1e\n",
    "package p\nvar x = 0x1p\n",
    "package p\nvar x = .e1\n",
    "package p\nvar 日本語 = 1\n",
    "package p\nvar \u{00b7} = 1\n",
    "package p\nvar 🙂 = 1\n",
    "package p\n\u{feff}var x = 1\n",
    "package p\nvar x = 1\u{2028}\n",
    "package p\nvar x = 1\u{0}\n",
    "",
    " ",
    "package",
    "package p",
    "package p\n",
    "package p\nimport \"x\"\n",
    "package p\nimport \"x\" \"y\"\n",
    "package p\nfunc F() {\n\tif c : CmpInt(a, b); c != 0 {\n\t\treturn c\n\t}\n}\n",
    "package p\nfunc F() {\n\tif var x int; x > 0 {\n\t}\n}\n",
    "package p\nfunc F() {\n\tfor go f(); x; y++ {\n\t}\n}\n",
];

fn duplicate_or_delete(source: &str, span: &Range<usize>, duplicate: bool) -> String {
    if duplicate {
        let mut mutated = String::with_capacity(source.len() + (span.end - span.start));
        mutated.push_str(&source[..span.end]);
        mutated.push_str(&source[span.clone()]);
        mutated.push_str(&source[span.end..]);
        mutated
    } else {
        format!("{}{}", &source[..span.start], &source[span.end..])
    }
}

#[test]
#[ignore = "prepare the pinned Go source corpus and set GOQUERY_GO_SOURCE"]
fn go_source_negative_syntax_corpus() {
    let corpus = GoCorpus::load();
    let oracle = OracleBinary::build("negative");
    let mut stats = Stats::default();
    let mut mismatches = MismatchBucket::default();
    let mut acceptance: BTreeMap<PathBuf, bool> = BTreeMap::new();
    let mut rejected: Vec<(PathBuf, Value)> = Vec::new();
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
            stats.files += 1;
            if accepted {
                stats.accepted += 1;
            } else {
                stats.rejected += 1;
                rejected.push((file.clone(), report.clone()));
            }
            acceptance.insert(file.clone(), accepted);
        }
        stats.packages += 1;
    }
    assert!(stats.files > 0, "negative corpus must inspect source files");
    if corpus.selected.is_none() && (corpus.scope.is_empty() || corpus.scope == "all") {
        assert!(
            stats.rejected > 0,
            "full negative corpus must contain go/parser-rejected files"
        );
    }
    eprintln!(
        "negative corpus: {} packages, {} files, {} accepted by go/parser, {} rejected",
        stats.packages, stats.files, stats.accepted, stats.rejected
    );

    // (a) Every rejected corpus file: native must reject it with in-source diagnostics.
    for (file, report) in &rejected {
        let bytes = std::fs::read(file).expect("read rejected corpus file");
        if bytes.len() > MAX_REJECTED_FILE_BYTES {
            stats.rejected_skipped += 1;
            continue;
        }
        let Ok(source) = String::from_utf8(bytes) else {
            stats.rejected_skipped += 1;
            continue;
        };
        let display = file.display().to_string();
        compare_rejected(&display, &source, report, &mut stats, &mut mismatches);
        stats.rejected_compared += 1;
    }

    // (b) Deterministic strided mutations of accepted corpus files.
    let (mutation_sources, mutation_labels) = collect_strided_mutations(&acceptance);
    for (chunk_index, chunk) in mutation_sources.chunks(MUTATION_CHUNK).enumerate() {
        let reports = oracle.run("--negative-sources", chunk);
        assert_eq!(
            reports.len(),
            chunk.len(),
            "oracle must inspect every mutation"
        );
        for ((report, source), label) in reports
            .iter()
            .zip(chunk)
            .zip(&mutation_labels[chunk_index * MUTATION_CHUNK..])
        {
            compare_mutation(label, source, report, &mut stats, &mut mismatches);
        }
    }

    // (c) Curated grammar-boundary samples (valid and invalid).
    for (chunk_index, chunk) in CURATED_SNIPPETS.chunks(MUTATION_CHUNK).enumerate() {
        let reports = oracle.run("--negative-sources", chunk);
        assert_eq!(
            reports.len(),
            chunk.len(),
            "oracle must inspect every sample"
        );
        for (offset, (report, source)) in reports.iter().zip(chunk).enumerate() {
            compare_curated(
                chunk_index * MUTATION_CHUNK + offset,
                source,
                report,
                &mut stats,
                &mut mismatches,
            );
        }
    }

    // (d) Bounded pathological sources: panic safety and diagnostic caps.
    let stress = stress_sources();
    for (chunk_index, chunk) in stress.chunks(MUTATION_CHUNK).enumerate() {
        let reports = oracle.run("--negative-sources", chunk);
        assert_eq!(
            reports.len(),
            chunk.len(),
            "oracle must inspect every stress sample"
        );
        for (offset, (report, source)) in reports.iter().zip(chunk).enumerate() {
            compare_stress(
                chunk_index * MUTATION_CHUNK + offset,
                source,
                report,
                &mut stats,
                &mut mismatches,
            );
        }
    }

    // (e) Broad sweep: one deterministic mutation of every eligible accepted file.
    let mut broad_sources = Vec::new();
    let mut broad_labels = Vec::new();
    let mut broad_generated = 0usize;
    for (file, &accepted) in &acceptance {
        if !accepted {
            continue;
        }
        if broad_generated >= MAX_BROAD_MUTATIONS {
            break;
        }
        let Ok(metadata) = std::fs::metadata(file) else {
            continue;
        };
        if metadata.len() as usize > MAX_SEED_BYTES {
            continue;
        }
        let Ok(source) = std::fs::read_to_string(file) else {
            continue;
        };
        if source.len() < 32 {
            continue;
        }
        let anchor = source.len() * (broad_generated % 5 + 1) / 6;
        let Some(offset) = char_boundary_at_or_after(&source, anchor) else {
            continue;
        };
        let kind = broad_generated % 3;
        let Some(mutated) = mutation_for(&source, offset, kind) else {
            continue;
        };
        broad_sources.push(mutated);
        broad_labels.push(format!(
            "{} broad {broad_generated} kind {kind}",
            file.display()
        ));
        broad_generated += 1;
        if broad_sources.len() == MUTATION_CHUNK {
            flush_broad_chunk(
                &oracle,
                &mut broad_sources,
                &mut broad_labels,
                &mut stats,
                &mut mismatches,
            );
        }
    }
    flush_broad_chunk(
        &oracle,
        &mut broad_sources,
        &mut broad_labels,
        &mut stats,
        &mut mismatches,
    );

    eprintln!(
        "negative differential: {} rejected files compared ({} excluded), {} strided byte/char/token/structural/keyword/newline mutations ({} parser-accepted; {} broad sweep mutations, {} accepted; {} curated samples, {} accepted; {} stress sources, {} intentional nesting-bound rejections; {} diagnostics); native first diagnostic before/equal/after go/parser first offset: {}/{}/{} ({} without position)",
        stats.rejected_compared,
        stats.rejected_skipped,
        stats.mutations,
        stats.mutations_accepted,
        stats.broad_mutations,
        stats.broad_accepted,
        stats.snippets,
        stats.snippets_accepted,
        stats.stress,
        stats.stress_nesting_bound,
        stats.diagnostic_count,
        stats.native_first_before,
        stats.native_first_equal,
        stats.native_first_after,
        stats.native_first_unknown
    );
    if let Some(path) = std::env::var_os("GOQUERY_NEGATIVE_REPORT") {
        let report = serde_json::json!({
            "version": "go1.27.3",
            "scope": corpus.scope,
            "packages": stats.packages,
            "files": stats.files,
            "parser_accepted": stats.accepted,
            "parser_rejected": stats.rejected,
            "rejected_compared": stats.rejected_compared,
            "rejected_skipped": stats.rejected_skipped,
            "mutations": stats.mutations,
            "mutations_accepted": stats.mutations_accepted,
            "mutations_rejected": stats.mutations_rejected,
            "broad_mutations": stats.broad_mutations,
            "broad_accepted": stats.broad_accepted,
            "curated_samples": stats.snippets,
            "curated_accepted": stats.snippets_accepted,
            "stress_sources": stats.stress,
            "stress_bounded_rejections": stats.stress_nesting_bound,
            "mismatches": mismatches.total(),
            "diagnostic_count": stats.diagnostic_count,
            "native_first_before": stats.native_first_before,
            "native_first_equal": stats.native_first_equal,
            "native_first_after": stats.native_first_after,
            "native_first_unknown": stats.native_first_unknown,
            "mismatch_classes": mismatches.classes,
            "examples": mismatches.examples,
        });
        std::fs::write(
            path,
            serde_json::to_vec_pretty(&report).expect("encode negative report"),
        )
        .expect("write negative report");
    }
    mismatches.assert_empty("negative syntax divergences");
}

fn collect_strided_mutations(acceptance: &BTreeMap<PathBuf, bool>) -> (Vec<String>, Vec<String>) {
    use goquery::syntax::TokenKind;
    let eligible: Vec<PathBuf> = acceptance
        .iter()
        .filter(|(file, accepted)| {
            **accepted
                && std::fs::metadata(file).is_ok_and(|meta| meta.len() as usize <= MAX_SEED_BYTES)
        })
        .map(|(file, _)| file.clone())
        .collect();
    let stride = (eligible.len() / MAX_SEED_FILES).max(1);
    let mut sources = Vec::new();
    let mut labels = Vec::new();
    let mut bytes = 0usize;
    'seeds: for file in eligible.iter().step_by(stride).take(MAX_SEED_FILES) {
        let Ok(source) = std::fs::read_to_string(file) else {
            continue;
        };
        if source.len() < 32 || source.len() > MAX_SEED_BYTES {
            continue;
        }
        let display = file.display().to_string();
        for index in 0..CHAR_MUTATIONS_PER_FILE {
            let anchor = source.len() * (index + 1) / (CHAR_MUTATIONS_PER_FILE + 1);
            let Some(offset) = char_boundary_at_or_after(&source, anchor) else {
                continue;
            };
            let Some(mutated) = mutation_for(&source, offset, index) else {
                continue;
            };
            if !push_mutation(
                &source,
                mutated,
                format!("{display} char anchor {offset} kind {index}"),
                &mut sources,
                &mut labels,
                &mut bytes,
            ) {
                break 'seeds;
            }
        }
        let tokens: Vec<Range<usize>> = goquery::syntax::Lexer::new(&source)
            .filter(|token| !token.inserted && token.span.start < token.span.end)
            .map(|token| token.span)
            .collect();
        if tokens.len() < TOKEN_MUTATIONS_PER_FILE + 1 {
            continue;
        }
        for index in 0..TOKEN_MUTATIONS_PER_FILE {
            let token_index = tokens.len() * (index + 1) / (TOKEN_MUTATIONS_PER_FILE + 1);
            let span = &tokens[token_index];
            let mutated = match index {
                0 => duplicate_or_delete(&source, span, false),
                1 => duplicate_or_delete(&source, span, true),
                2 => {
                    let end = tokens
                        .get(token_index + 1)
                        .map_or(span.end, |next| next.end);
                    format!("{}{}", &source[..span.start], &source[end..])
                }
                _ => source[..span.start].to_owned(),
            };
            if !push_mutation(
                &source,
                mutated,
                format!("{display} token {token_index} kind {index}"),
                &mut sources,
                &mut labels,
                &mut bytes,
            ) {
                break 'seeds;
            }
        }
        let structural: Vec<Range<usize>> = goquery::syntax::Lexer::new(&source)
            .filter(|token| {
                !token.inserted
                    && matches!(
                        token.kind,
                        TokenKind::LParen
                            | TokenKind::RParen
                            | TokenKind::LBrack
                            | TokenKind::RBrack
                            | TokenKind::LBrace
                            | TokenKind::RBrace
                            | TokenKind::Semicolon
                            | TokenKind::Comma
                            | TokenKind::Colon
                    )
            })
            .map(|token| token.span)
            .collect();
        let keywords: Vec<Range<usize>> = goquery::syntax::Lexer::new(&source)
            .filter(|token| {
                !token.inserted
                    && matches!(
                        token.kind,
                        TokenKind::Break
                            | TokenKind::Case
                            | TokenKind::Chan
                            | TokenKind::Const
                            | TokenKind::Continue
                            | TokenKind::Default
                            | TokenKind::Defer
                            | TokenKind::Else
                            | TokenKind::Fallthrough
                            | TokenKind::For
                            | TokenKind::Func
                            | TokenKind::Go
                            | TokenKind::Goto
                            | TokenKind::If
                            | TokenKind::Import
                            | TokenKind::Interface
                            | TokenKind::Map
                            | TokenKind::Package
                            | TokenKind::Range
                            | TokenKind::Return
                            | TokenKind::Select
                            | TokenKind::Struct
                            | TokenKind::Switch
                            | TokenKind::Type
                            | TokenKind::Var
                    )
            })
            .map(|token| token.span)
            .collect();
        for (spans, kind_label, count) in [
            (&structural, "structural", 3usize),
            (&keywords, "keyword", 2),
        ] {
            if spans.len() <= count {
                continue;
            }
            for index in 0..count {
                let target = spans.len() * (index + 1) / (count + 1);
                let span = &spans[target];
                for duplicate in [false, true] {
                    let mutated = duplicate_or_delete(&source, span, duplicate);
                    let suffix = if duplicate { " duplicate" } else { "" };
                    if !push_mutation(
                        &source,
                        mutated,
                        format!(
                            "{display} {kind_label} {target} byte {}{suffix}",
                            span.start
                        ),
                        &mut sources,
                        &mut labels,
                        &mut bytes,
                    ) {
                        break 'seeds;
                    }
                }
            }
        }
        let newlines: Vec<usize> = source
            .match_indices('\n')
            .map(|(offset, _)| offset)
            .collect();
        if newlines.len() >= 3 {
            for index in 0..2 {
                let offset = newlines[newlines.len() * (index + 1) / 3];
                let mutated = format!("{}{}", &source[..offset], &source[offset + 1..]);
                if !push_mutation(
                    &source,
                    mutated,
                    format!("{display} delete-newline {index} byte {offset}"),
                    &mut sources,
                    &mut labels,
                    &mut bytes,
                ) {
                    break 'seeds;
                }
            }
        }
        for index in 0..3 {
            let anchor = source.len() * (index + 1) / 4;
            let Some(offset) = char_boundary_at_or_after(&source, anchor) else {
                continue;
            };
            let mutated = format!("{}\n{}", &source[..offset], &source[offset..]);
            if !push_mutation(
                &source,
                mutated,
                format!("{display} insert-newline {index} byte {offset}"),
                &mut sources,
                &mut labels,
                &mut bytes,
            ) {
                break 'seeds;
            }
        }
    }
    (sources, labels)
}

fn compare_curated(
    index: usize,
    source: &str,
    report: &Value,
    stats: &mut Stats,
    mismatches: &mut MismatchBucket,
) {
    let accepted = report["accepted"]
        .as_bool()
        .expect("curated acceptance bool");
    stats.snippets += 1;
    let Ok(diagnostics) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        goquery::syntax::validate_file(source)
    })) else {
        mismatches.record(
            "negative:panic",
            format!("snippet {index}: validate_file panicked"),
        );
        return;
    };
    let native_accepted = diagnostics.is_empty();
    if accepted != native_accepted {
        let class = if accepted {
            "negative:curated_false_reject"
        } else {
            "negative:curated_false_accept"
        };
        mismatches.record(class, format!("snippet {index} {source:?}"));
        return;
    }
    if accepted {
        stats.snippets_accepted += 1;
        return;
    }
    stats.diagnostic_count += diagnostics.len();
    check_diagnostics(
        &format!("snippet {index}"),
        source,
        &diagnostics,
        mismatches,
    );
    record_first_position(report, &diagnostics, stats);
}

fn stress_sources() -> Vec<String> {
    vec![
        format!("package p\n{}", "@;\n".repeat(6000)),
        format!(
            "package p\nfunc F() {{ {}return{} }}",
            "func(){".repeat(160),
            "}();".repeat(160)
        ),
        format!("package p\nvar x = {}1", "(".repeat(400)),
        format!("package p\nvar {} = 1\n", "a".repeat(100_000)),
        format!("package p\n{}", "type T struct{ F int }\n".repeat(2000)),
        format!("package p\nvar _ = [{}1]int{{}}", "1*".repeat(400)),
    ]
}

fn compare_stress(
    index: usize,
    source: &str,
    report: &Value,
    stats: &mut Stats,
    mismatches: &mut MismatchBucket,
) {
    stats.stress += 1;
    let Ok(diagnostics) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        goquery::syntax::validate_file(source)
    })) else {
        mismatches.record(
            "negative:panic",
            format!("stress {index}: validate_file panicked"),
        );
        return;
    };
    let accepted = report["accepted"]
        .as_bool()
        .expect("stress acceptance bool");
    let native_accepted = diagnostics.is_empty();
    if !accepted && native_accepted {
        mismatches.record(
            "negative:stress_false_accept",
            format!(
                "stress {index}: {} bytes fp {:016x}",
                source.len(),
                fingerprint(source)
            ),
        );
    } else if accepted && !native_accepted {
        stats.stress_nesting_bound += 1;
    }
    check_diagnostics(
        &format!("stress {index} ({} bytes)", source.len()),
        source,
        &diagnostics,
        mismatches,
    );
    stats.diagnostic_count += diagnostics.len();
}

fn push_mutation(
    original: &str,
    mutated: String,
    label: String,
    sources: &mut Vec<String>,
    labels: &mut Vec<String>,
    bytes: &mut usize,
) -> bool {
    if mutated == original {
        return true;
    }
    if sources.len() >= MAX_MUTATION_SOURCES || *bytes + mutated.len() > MAX_MUTATION_BYTES {
        return false;
    }
    *bytes += mutated.len();
    sources.push(mutated);
    labels.push(label);
    true
}

fn fingerprint(source: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in source.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn compare_mutation(
    label: &str,
    source: &str,
    report: &Value,
    stats: &mut Stats,
    mismatches: &mut MismatchBucket,
) {
    stats.mutations += 1;
    if let Some(accepted) = compare_generated("negative", label, source, report, stats, mismatches)
    {
        if accepted {
            stats.mutations_accepted += 1;
        } else {
            stats.mutations_rejected += 1;
        }
    }
}

fn compare_generated(
    prefix: &str,
    label: &str,
    source: &str,
    report: &Value,
    stats: &mut Stats,
    mismatches: &mut MismatchBucket,
) -> Option<bool> {
    let accepted = report["accepted"]
        .as_bool()
        .expect("generated acceptance bool");
    let Ok(diagnostics) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        goquery::syntax::validate_file(source)
    })) else {
        mismatches.record(
            format!("{prefix}:panic"),
            format!("{label}: validate_file panicked"),
        );
        return None;
    };
    let native_accepted = diagnostics.is_empty();
    if accepted != native_accepted {
        let class = if accepted {
            "false_reject"
        } else {
            "false_accept"
        };
        dump_failing_source(source);
        mismatches.record(
            format!("{prefix}:{class}"),
            format!(
                "{label}: go/parser accepted={accepted}, validate_file accepted={native_accepted}, {} bytes, fp {:016x}",
                source.len(),
                fingerprint(source)
            ),
        );
        return None;
    }
    if !native_accepted {
        stats.diagnostic_count += diagnostics.len();
        check_diagnostics(label, source, &diagnostics, mismatches);
        record_first_position(report, &diagnostics, stats);
    }
    Some(accepted)
}

fn dump_failing_source(source: &str) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static DUMPED: AtomicUsize = AtomicUsize::new(0);
    let Some(directory) = std::env::var_os("GOQUERY_NEGATIVE_DUMP") else {
        return;
    };
    let index = DUMPED.fetch_add(1, Ordering::Relaxed);
    if index >= 50 {
        return;
    }
    let directory = PathBuf::from(directory);
    let _ = std::fs::create_dir_all(&directory);
    let _ = std::fs::write(directory.join(format!("negative-{index:03}.go")), source);
}

fn flush_broad_chunk(
    oracle: &OracleBinary,
    sources: &mut Vec<String>,
    labels: &mut Vec<String>,
    stats: &mut Stats,
    mismatches: &mut MismatchBucket,
) {
    if sources.is_empty() {
        return;
    }
    let reports = oracle.run("--negative-sources", sources.as_slice());
    assert_eq!(
        reports.len(),
        sources.len(),
        "oracle must inspect every broad mutation"
    );
    for ((report, source), label) in reports.iter().zip(sources.iter()).zip(labels.iter()) {
        stats.broad_mutations += 1;
        if let Some(accepted) =
            compare_generated("negative:broad", label, source, report, stats, mismatches)
            && accepted
        {
            stats.broad_accepted += 1;
        }
    }
    sources.clear();
    labels.clear();
}

fn compare_rejected(
    display: &str,
    source: &str,
    report: &Value,
    stats: &mut Stats,
    mismatches: &mut MismatchBucket,
) {
    assert_eq!(report["accepted"].as_bool(), Some(false));
    let Ok(diagnostics) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        goquery::syntax::validate_file(source)
    })) else {
        mismatches.record(
            "negative:panic",
            format!("{display}: validate_file panicked"),
        );
        return;
    };
    if diagnostics.is_empty() {
        mismatches.record(
            "negative:false_accept",
            format!("{display}: go/parser rejects but validate_file accepts"),
        );
        return;
    }
    check_diagnostics(display, source, &diagnostics, mismatches);
    stats.diagnostic_count += diagnostics.len();
    record_first_position(report, &diagnostics, stats);
}

fn record_first_position(
    report: &Value,
    diagnostics: &[goquery::syntax::Diagnostic],
    stats: &mut Stats,
) {
    let Some(go_first) = report["first_offset"].as_i64() else {
        stats.native_first_unknown += 1;
        return;
    };
    if go_first < 0 {
        stats.native_first_unknown += 1;
        return;
    }
    let native_first = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.span.start as i64)
        .min()
        .expect("rejected input has diagnostics");
    match native_first.cmp(&go_first) {
        std::cmp::Ordering::Less => stats.native_first_before += 1,
        std::cmp::Ordering::Equal => stats.native_first_equal += 1,
        std::cmp::Ordering::Greater => stats.native_first_after += 1,
    }
}

fn check_diagnostics(
    label: &str,
    source: &str,
    diagnostics: &[goquery::syntax::Diagnostic],
    mismatches: &mut MismatchBucket,
) {
    if diagnostics.len() > MAX_DIAGNOSTICS {
        mismatches.record(
            "negative:overlong_diagnostics",
            format!("{label}: {} diagnostics", diagnostics.len()),
        );
    }
    for diagnostic in diagnostics {
        let span = &diagnostic.span;
        let valid = span.start <= span.end
            && span.end <= source.len()
            && source.is_char_boundary(span.start)
            && source.is_char_boundary(span.end);
        if !valid {
            mismatches.record(
                "negative:invalid_span",
                format!(
                    "{label}: {:?} outside {} bytes / char boundary",
                    span,
                    source.len()
                ),
            );
            break;
        }
    }
}

fn char_boundary_at_or_after(source: &str, anchor: usize) -> Option<usize> {
    if source.is_empty() {
        return None;
    }
    let mut offset = anchor.min(source.len().saturating_sub(1));
    while offset < source.len() && !source.is_char_boundary(offset) {
        offset += 1;
    }
    (offset > 0 && offset < source.len()).then_some(offset)
}

fn next_boundary(source: &str, offset: usize) -> usize {
    source[offset..]
        .char_indices()
        .nth(1)
        .map_or(source.len(), |(index, _)| offset + index)
}

/// Six deterministic mutations per seed: delete, duplicate, truncate,
/// replace with an invalid token byte, insert a quote, delete an 8-char span.
fn mutation_for(source: &str, offset: usize, kind: usize) -> Option<String> {
    let end = next_boundary(source, offset);
    let mutated = match kind {
        0 => format!("{}{}", &source[..offset], &source[end..]),
        1 => format!(
            "{}{}{}",
            &source[..end],
            &source[offset..end],
            &source[end..]
        ),
        2 => source[..offset].to_owned(),
        3 => format!("{}@{}", &source[..offset], &source[end..]),
        4 => format!("{}`{}", &source[..offset], &source[offset..]),
        _ => {
            let mut far = offset;
            for _ in 0..8 {
                if far >= source.len() {
                    break;
                }
                far = next_boundary(source, far);
            }
            format!("{}{}", &source[..offset], &source[far..])
        }
    };
    (mutated != source).then_some(mutated)
}
