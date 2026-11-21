use crate::frontend::{check_token_stream, check_type_spans};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use walkdir::WalkDir;

pub fn official_acceptance(cases: &[String], mode: &str) -> Vec<bool> {
    serde_json::from_value(official_output(cases, mode)).expect("oracle acceptance results")
}

pub fn official_output(cases: &[String], mode: &str) -> Value {
    let base = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut command = Command::new("go");
    command.arg("run").arg(base.join("tests/oracle.go"));
    if !mode.is_empty() {
        command.arg(mode);
    }
    let mut oracle = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Go SDK required for syntax oracle");
    oracle
        .stdin
        .take()
        .expect("oracle stdin")
        .write_all(&serde_json::to_vec(&cases).expect("encode corpus"))
        .expect("send corpus");
    let output = oracle.wait_with_output().expect("wait for Go oracle");
    assert!(
        output.status.success(),
        "Go oracle failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("oracle output")
}

pub struct GoCorpus {
    pub source_root: PathBuf,
    pub packages: BTreeMap<String, Vec<PathBuf>>,
    pub scope: String,
    pub selected: Option<String>,
}

impl GoCorpus {
    pub fn load() -> Self {
        let root = std::env::var_os("GOQUERY_GO_SOURCE")
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("goquery-go1.27.3/go-go1.27.3"));
        let version = std::fs::read_to_string(root.join("VERSION"))
            .expect("prepare the pinned Go source corpus (GOQUERY_GO_SOURCE)");
        assert_eq!(
            version.lines().next(),
            Some("go1.27.3"),
            "external source version must be pinned"
        );
        let source_root = root.join("src");
        let mut packages: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
        for entry in WalkDir::new(&source_root).follow_links(false) {
            let entry = entry.expect("source traversal must not silently drop errors");
            if entry.file_type().is_file()
                && entry.path().extension().is_some_and(|ext| ext == "go")
            {
                let parent = entry.path().parent().expect("source directory");
                let package = parent
                    .strip_prefix(&source_root)
                    .expect("source is inside corpus")
                    .to_string_lossy()
                    .replace('\\', "/");
                packages.entry(package).or_default().push(entry.into_path());
            }
        }
        let scope = std::env::var("GOQUERY_GO_SCOPE").unwrap_or_default();
        assert!(
            matches!(scope.as_str(), "" | "stdlib" | "toolchain" | "all"),
            "unknown corpus scope: {scope}"
        );
        let selected = std::env::var("GOQUERY_GO_PACKAGE").ok();
        Self {
            source_root,
            packages,
            scope,
            selected,
        }
    }

    pub fn package_enabled(
        &self,
        package: &str,
        default_includes_testdata: bool,
        skipped: Option<&serde_json::Map<String, Value>>,
    ) -> bool {
        if let Some(selected) = &self.selected {
            selected == package
        } else if self.scope == "all" {
            true
        } else if self.scope == "stdlib" {
            !package
                .split('/')
                .any(|part| matches!(part, "cmd" | "testdata"))
        } else if self.scope == "toolchain" {
            !package.split('/').any(|part| part == "testdata")
        } else if let Some(skipped) = skipped {
            !skipped.contains_key(package)
        } else if default_includes_testdata {
            true
        } else {
            !package.split('/').any(|part| part == "testdata")
        }
    }
}

pub struct OracleBinary {
    path: PathBuf,
}

impl OracleBinary {
    pub fn build(tag: &str) -> Self {
        let base = Path::new(env!("CARGO_MANIFEST_DIR"));
        let path = std::env::temp_dir().join(format!(
            "goquery-{tag}-oracle-{}{}",
            std::process::id(),
            std::env::consts::EXE_SUFFIX
        ));
        let toolchain = Command::new("go")
            .arg("version")
            .output()
            .expect("Go SDK for pinned oracle");
        assert!(
            toolchain.status.success()
                && String::from_utf8_lossy(&toolchain.stdout)
                    .split_whitespace()
                    .any(|word| word == "go1.27.3"),
            "{tag} oracle requires Go 1.27.3"
        );
        let build = Command::new("go")
            .arg("build")
            .arg("-o")
            .arg(&path)
            .arg(base.join("tests/oracle.go"))
            .output()
            .expect("build official oracle");
        assert!(
            build.status.success(),
            "oracle build failed: {}",
            String::from_utf8_lossy(&build.stderr)
        );
        Self { path }
    }

    pub fn run<T: Serialize + ?Sized>(&self, flag: &str, payload: &T) -> Vec<Value> {
        let mut child = Command::new(&self.path)
            .arg(flag)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap_or_else(|error| panic!("launch oracle {flag}: {error}"));
        child
            .stdin
            .take()
            .expect("oracle stdin")
            .write_all(&serde_json::to_vec(payload).expect("encode oracle payload"))
            .expect("send oracle payload");
        let output = child.wait_with_output().expect("wait for oracle");
        assert!(
            output.status.success(),
            "oracle {flag} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).expect("oracle reports")
    }
}

#[derive(Default)]
pub struct MismatchBucket {
    pub classes: BTreeMap<String, usize>,
    pub examples: BTreeMap<String, Vec<String>>,
}

impl MismatchBucket {
    pub fn record(&mut self, class: impl Into<String>, detail: String) {
        let class = class.into();
        *self.classes.entry(class.clone()).or_default() += 1;
        let examples = self.examples.entry(class).or_default();
        if examples.len() < 5 {
            let truncated = detail.chars().count() > 240;
            let mut detail: String = detail.chars().take(240).collect();
            if truncated {
                detail.push_str("...");
            }
            examples.push(detail);
        }
    }

    pub fn total(&self) -> usize {
        self.classes.values().sum()
    }

    pub fn is_empty(&self) -> bool {
        self.classes.is_empty()
    }

    pub fn assert_empty(&self, label: &str) {
        if self.is_empty() {
            return;
        }
        let mut lines = Vec::new();
        for (class, count) in &self.classes {
            lines.push(format!("{class}: {count}"));
            for example in self.examples.get(class).into_iter().flatten() {
                lines.push(format!("  {example}"));
            }
        }
        panic!(
            "{label} ({} classes):\n{}",
            self.classes.len(),
            lines.join("\n")
        );
    }
}

pub fn run_package_corpus() {
    let corpus = GoCorpus::load();
    let manifest: Value = serde_json::from_str(include_str!("../go-corpus/skip-packages.json"))
        .expect("skip manifest");
    assert_eq!(manifest["version"], "go1.27.3");
    let skipped = manifest["packages"]
        .as_object()
        .expect("exact package names with reasons");
    for (package, reason) in skipped {
        assert!(
            corpus.packages.contains_key(package),
            "stale skip entry: {package}"
        );
        assert!(
            reason.as_str().is_some_and(|r| !r.trim().is_empty()),
            "missing skip reason for {package}"
        );
    }
    if let Some(package) = &corpus.selected {
        assert!(
            corpus.packages.contains_key(package),
            "unknown package {package}"
        );
    }
    let oracle = OracleBinary::build("corpus");
    let mut checked_packages = 0;
    let mut checked_files = 0;
    let mut checked_bodies = 0;
    let mut checked_headers = 0;
    let mut checked_types = 0;
    let mut checked_initializers = 0;
    let mut failures = Vec::new();
    let mut classifications = Vec::new();
    for (package, mut files) in corpus.packages.clone() {
        if !corpus.package_enabled(&package, false, Some(skipped)) {
            continue;
        }
        files.sort();
        let reports = oracle.run("--file-bodies", &files);
        assert_eq!(reports.len(), files.len(), "oracle must inspect every file");
        let (mut package_bodies, mut package_headers, mut package_types, mut package_initializers) =
            (0, 0, 0, 0);
        for (report, file) in reports.iter().zip(&files) {
            assert_eq!(
                report["file"].as_str(),
                file.to_str(),
                "oracle report order"
            );
            let source = std::fs::read_to_string(file).expect("read corpus file");
            let file_diagnostics = goquery::syntax::validate_file(&source);
            assert!(file_diagnostics.len() <= 4096);
            assert!(
                file_diagnostics
                    .iter()
                    .all(|diagnostic| diagnostic.span.start <= diagnostic.span.end
                        && source.get(diagnostic.span.clone()).is_some())
            );
            let accepted = report["error"].as_str() == Some("");
            let stub = goquery::syntax::stub(&source);
            for symbol in &stub.symbols {
                if symbol.range.start > symbol.selection.start
                    || symbol.selection.end > symbol.range.end
                    || source.get(symbol.range.clone()).is_none()
                    || source.get(symbol.selection.clone()).is_none()
                {
                    failures.push(format!("{} invalid stub declaration span", file.display()));
                }
            }
            let mut previous = 0;
            for group in &stub.constant_groups {
                let valid =
                    group.start >= previous
                        && source.get(group.clone()).is_some_and(|text| {
                            let mut tokens = goquery::syntax::Lexer::new(text);
                            tokens.next().is_some_and(|token| {
                                token.kind == goquery::syntax::TokenKind::Const
                            }) && tokens.next().is_some_and(|token| {
                                token.kind == goquery::syntax::TokenKind::LParen
                            }) && tokens
                                .filter(|token| token.kind != goquery::syntax::TokenKind::Semicolon)
                                .last()
                                .is_some_and(|token| {
                                    token.kind == goquery::syntax::TokenKind::RParen
                                })
                        });
                if !valid {
                    failures.push(format!("{} invalid constant group span", file.display()));
                }
                previous = group.end;
            }
            classifications.push(serde_json::json!({
                "file": file.strip_prefix(&corpus.source_root).unwrap(),
                "parser_accepted": accepted,
                "parser_errors": report["error_count"],
                "native_diagnostics": file_diagnostics.len(),
                "stub_symbols": stub.symbols.len(),
                "constant_groups": stub.constant_groups.len(),
            }));
            if !accepted {
                if file_diagnostics.is_empty() {
                    failures.push(format!("{package}: {} false accept", file.display()));
                }
                continue;
            }
            if !file_diagnostics.is_empty() {
                failures.push(format!(
                    "{package}: {} complete file: {:?}",
                    file.display(),
                    file_diagnostics
                ));
            }
            check_token_stream(&source, report["tokens"].as_array().expect("oracle tokens"));
            for header in report["headers"]
                .as_array()
                .expect("oracle signature spans")
            {
                let start = header["start"].as_u64().expect("header start") as usize;
                let end = header["end"].as_u64().expect("header end") as usize;
                let header_source = source.get(start..end).expect("signature source");
                let generic_header;
                let header_source = if header["type_parameters"].as_bool() == Some(true) {
                    generic_header = format!("func Generic{header_source}()");
                    generic_header.as_str()
                } else {
                    header_source
                };
                let signature = if header["literal"].as_bool() == Some(true) {
                    goquery::syntax::parse_literal_signature(header_source)
                } else {
                    goquery::syntax::parse_signature(header_source)
                };
                if !signature.diagnostics().is_empty() {
                    failures.push(format!(
                        "{package}: {} signature byte {start}: {:?}",
                        file.display(),
                        signature.diagnostics()
                    ));
                }
                package_headers += 1;
            }
            for span in report["types"].as_array().expect("oracle type spans") {
                let start = span["start"].as_u64().expect("type start") as usize;
                let end = span["end"].as_u64().expect("type end") as usize;
                let type_source = source.get(start..end).expect("type source");
                let method_source;
                let type_source = if span["method"].as_bool() == Some(true) {
                    method_source = format!("func{type_source}");
                    method_source.as_str()
                } else {
                    type_source
                };
                let parsed = goquery::syntax::parse_type(type_source);
                check_type_spans(type_source, &parsed);
                if !parsed.diagnostics().is_empty() {
                    failures.push(format!(
                        "{package}: {} type byte {start}: {:?}",
                        file.display(),
                        parsed.diagnostics()
                    ));
                }
                package_types += 1;
            }
            let initializers = report
                .get("initializers")
                .expect("oracle initializer field");
            assert!(
                initializers.is_null() || initializers.is_array(),
                "oracle initializer spans"
            );
            for span in initializers.as_array().into_iter().flatten() {
                let start = span["start"].as_u64().expect("initializer start") as usize;
                let end = span["end"].as_u64().expect("initializer end") as usize;
                let text = source.get(start..end).expect("initializer source");
                let parsed = goquery::syntax::parse_expression(text);
                if !parsed.diagnostics().is_empty() {
                    failures.push(format!(
                        "{package}: {} initializer byte {start}: {:?}",
                        file.display(),
                        parsed.diagnostics()
                    ));
                }
                package_initializers += 1;
            }
            // Official body ranges prevent unsupported declarations from silently
            // hiding functions from this gate. We do not compare AST shapes.
            for body in report["bodies"].as_array().expect("oracle body spans") {
                let start = body["start"].as_u64().expect("body start") as usize;
                let end = body["end"].as_u64().expect("body end") as usize;
                let text = source
                    .get(start..end)
                    .expect("oracle uses UTF-8 byte spans");
                let parsed = goquery::syntax::parse_block(text);
                if !parsed.diagnostics().is_empty() {
                    failures.push(format!(
                        "{package}: {} byte {start}: {:?}",
                        file.display(),
                        parsed.diagnostics()
                    ));
                }
                package_bodies += 1;
            }
        }
        eprintln!(
            "{package}: {} files, {package_headers} headers, {package_types} types, {package_initializers} initializers, {package_bodies} function bodies",
            files.len()
        );
        checked_packages += 1;
        checked_files += files.len();
        checked_bodies += package_bodies;
        checked_headers += package_headers;
        checked_types += package_types;
        checked_initializers += package_initializers;
    }
    assert!(
        checked_packages > 0 && checked_files > 0,
        "corpus gate must inspect real source files"
    );
    if let Some(path) = std::env::var_os("GOQUERY_GO_REPORT") {
        let report = serde_json::json!({
            "version": "go1.27.3",
            "scope": corpus.scope,
            "packages": checked_packages,
            "files": classifications,
            "failures": failures,
        });
        std::fs::write(path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    }
    eprintln!(
        "checked {checked_packages} packages, {checked_files} files, {checked_headers} headers, {checked_types} types, {checked_initializers} initializers, {checked_bodies} bodies; {} inventory skips (ignored by explicit scopes)",
        skipped.len()
    );
    assert!(
        failures.is_empty(),
        "source corpus failures:\n{}",
        failures.join("\n")
    );
}
