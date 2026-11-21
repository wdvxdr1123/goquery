use crate::harness::{decode_semantic_tokens, dir_uri, file_uri, jsonrpc2};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

pub const GOLDEN_CASES: &[&str] = &[
    "protocol",
    "navigation",
    "imports",
    "build",
    "experiments",
    "experiments-env",
    "experiments-override",
    "discovery",
    "discovery-explicit",
    "calls",
    "call-scopes",
    "methods",
    "types",
    "promoted",
    "completion",
    "completion-limit",
    "workspace-symbol",
    "member-symbols",
    "aggregate-symbols",
    "symbol-packages",
    "workspace-symbol-limit",
    "completion-snippets",
    "completion-fuzzy",
    "completion-members",
    "completion-index",
    "member-definitions",
    "member-references",
    "generic-members",
    "alias-members",
    "defined-members",
    "promoted-generics",
    "generic-methods",
    "callable-results",
    "completion-empty-snippets",
    "completion-plain",
    "completion-no-calls",
    "completion-options",
    "completion-signature-budget",
    "references-limit",
    "reference-index",
    "qualified-references",
    "unicode-identifiers",
    "escaped-imports",
    "package-import-names",
    "test-variants",
    "module-replace",
    "module-cache",
    "module-workspace",
    "module-sdk",
    "module-sdk-scope",
    "module-sdk-vendor",
    "go-checkout",
    "module-reference-scope",
    "handwritten-queries",
    "highlighting",
    "highlighting-signatures",
    "elided-literals",
    "interface-methods",
    "generic-results",
    "scoped-results",
    "scoped-generic-unimported",
    "indexed-scopes",
    "sliced-scopes",
    "range-scopes",
    "bound-scopes",
    "core-scopes",
    "implication-scopes",
    "receiver-scopes",
    "field-scopes",
    "parameter-scopes",
    "builtin-scopes",
    "package-values",
    "implementation",
    "implementation-embedding",
    "implementation-limits",
    "generic-hierarchy",
    "struct-hierarchy",
    "interface-hierarchy",
    "hierarchy-index",
    "highlighting-lexical",
    "highlighting-negotiated",
    "highlighting-identities",
    "highlighting-identity-negotiated",
    "highlighting-type-forms",
    "highlighting-shadowing",
    "highlighting-operations",
    "range-inference",
    "named-operands",
    "generic-calls",
    "constraint-calls",
    "typeset-calls",
    "native-method-calls",
    "struct-calls",
    "conversion-calls",
    "signature-calls",
    "argument-calls",
    "interface-values",
    "concrete-interface-calls",
    "native-method-private",
    "scoped-generic-private",
    "completion-edits-modern",
    "completion-edits-legacy",
    "generic-completion-plain",
    "generic-completion-snippets",
    "generic-completion-empty",
    "completion-types",
    "completion-documentation-markdown",
    "completion-documentation-plaintext",
    "completion-documentation-disabled",
    "signature-help",
    "inferred-receivers",
    "universe-forms",
    "type-definition",
    "numeric-builtins",
    "method-values",
    "assertions",
    "type-switch",
    "clause-scope",
    "receive-select",
    "local-bindings",
    "dot-imports",
    "definitions",
    "type-navigation",
    "gopls-features",
    "gopls-document-symbols",
    "gopls-formatting",
];

pub fn check_tower_transport() {
    fn exchange(bytes: &[u8], expected_status: i32) -> Vec<Value> {
        let mut child = Command::new(env!("CARGO_BIN_EXE_goquery"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut input = child.stdin.take().unwrap();
        // Deliberately split headers and UTF-8 bodies; leave stdin open through
        // exit so a stuck Tokio blocking read cannot hide behind client EOF.
        for chunk in bytes.chunks(3) {
            if let Err(error) = input.write_all(chunk) {
                assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
                break;
            }
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            if std::time::Instant::now() >= deadline {
                child.kill().unwrap();
                drop(input);
                let output = child.wait_with_output().unwrap();
                panic!(
                    "tower-lsp did not exit with stdin open: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        drop(input);
        let output = child.wait_with_output().unwrap();
        assert_eq!(output.status.code(), Some(expected_status));
        let mut bytes = output.stdout.as_slice();
        let mut replies = Vec::new();
        while let Some(reply) = jsonrpc2::read_message(&mut bytes).unwrap() {
            replies.push(reply);
        }
        // Keep only request responses; diagnostics are covered by their own case.
        replies.retain(|reply| reply.get("id").is_some());
        replies
    }

    let exit = jsonrpc2::frame(json!({"jsonrpc": "2.0", "method": "exit"}));
    assert!(exchange(&exit, 1).is_empty());
    let mut lifecycle: Vec<u8> = [
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"capabilities": {}, "initializationOptions": {"goroot": ""}}}),
        json!({"jsonrpc": "2.0", "method": "textDocument/didOpen", "params": {"textDocument": {"uri": "file:///tower-transport.go", "languageId": "go", "version": 1, "text": "package p\nvar _ = \"馃榾\"\n"}}}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "shutdown"}),
    ]
    .into_iter()
    .flat_map(jsonrpc2::frame)
    .collect();
    lifecycle.extend(&exit);
    let replies = exchange(&lifecycle, 0);
    assert_eq!(replies.len(), 2);
    assert_eq!(replies[0]["id"], 1);
    let caps = &replies[0]["result"]["capabilities"];
    assert_eq!(caps["typeHierarchyProvider"], true);
    assert_eq!(caps["documentHighlightProvider"], true);
    assert_eq!(
        caps["textDocumentSync"],
        json!({"openClose": true, "change": 1, "save": true})
    );
    assert_eq!(replies[1]["id"], 2);
    assert!(replies[1]["result"].is_null());

    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/methods");
    let root_uri = dir_uri(&root);
    let file_uri = file_uri(root.join("main.go"));
    let disabled_highlights: Vec<u8> = [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "rootUri": root_uri,
                "capabilities": {"textDocument": {"semanticTokens": {
                    "requests": {"full": true, "range": true}, "tokenTypes": ["function"],
                    "tokenModifiers": [], "formats": ["relative"]
                }}},
                "initializationOptions": {
                    "goroot": "", "fileWatch": false, "semanticTokens": false, "documentHighlight": false
                }
            }
        }),
        json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}),
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/semanticTokens/full",
            "params": {"textDocument": {"uri": file_uri}}
        }),
        json!({
            "jsonrpc": "2.0", "id": 3, "method": "textDocument/semanticTokens/range",
            "params": {"textDocument": {"uri": file_uri}, "range": {
                "start": {"line": 0, "character": 0}, "end": {"line": 1, "character": 0}
            }}
        }),
        json!({
            "jsonrpc": "2.0", "id": 4, "method": "textDocument/documentHighlight",
            "params": {"textDocument": {"uri": file_uri}, "position": {"line": 0, "character": 0}}
        }),
        json!({"jsonrpc": "2.0", "id": 5, "method": "shutdown"}),
        json!({"jsonrpc": "2.0", "method": "exit"}),
    ]
    .into_iter()
    .flat_map(jsonrpc2::frame)
    .collect();
    let replies = exchange(&disabled_highlights, 0);
    assert_eq!(replies.len(), 5);
    let capabilities = &replies[0]["result"]["capabilities"];
    assert_eq!(capabilities["definitionProvider"], true);
    assert!(capabilities.get("semanticTokensProvider").is_none());
    assert!(capabilities.get("documentHighlightProvider").is_none());
    for id in 2..=4 {
        assert_eq!(
            replies[id - 1],
            json!({"jsonrpc": "2.0", "id": id, "result": null}),
        );
    }

    let mut malformed = b"Content-Length: 1\r\n\r\n{".to_vec();
    malformed.extend(lifecycle);
    let replies = exchange(&malformed, 0);
    // tower-lsp's codec terminates this stream after a parse error; later
    // pipelined operations must not run on a malformed connection.
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0]["error"]["code"], -32700);
    for invalid in [
        b"x".repeat(8193),
        b"Content-Length: 33554433\r\n\r\n".to_vec(),
        b"Content-Length: 1\r\nContent-Length: 1\r\n\r\n".to_vec(),
    ] {
        let replies = exchange(&invalid, 1);
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0]["error"]["code"], -32700);
    }
}

fn fixture_rel(case: &str) -> &'static str {
    match case {
        "type-definition" | "numeric-builtins" | "method-values" | "assertions" | "type-switch"
        | "clause-scope" | "receive-select" | "local-bindings" => "tests/fixtures/type-definition",
        "implementation" | "implementation-embedding" | "implementation-limits" => {
            "tests/fixtures/implementation"
        }
        "generic-hierarchy" | "hierarchy-index" => "tests/fixtures/generic-hierarchy",
        "struct-hierarchy" => "tests/fixtures/struct-hierarchy",
        "interface-hierarchy" => "tests/fixtures/interface-hierarchy",
        "generic-results" => "tests/fixtures/generic-results",
        "named-operands"
        | "generic-calls"
        | "constraint-calls"
        | "typeset-calls"
        | "native-method-calls"
        | "struct-calls"
        | "conversion-calls"
        | "signature-calls"
        | "argument-calls"
        | "interface-values" => "tests/fixtures/type-oracle",
        "test-variants" => "tests/fixtures/test-variants",
        "scoped-results"
        | "scoped-generic-unimported"
        | "indexed-scopes"
        | "sliced-scopes"
        | "range-scopes" => "tests/fixtures/scoped-results",
        "interface-methods" => "tests/fixtures/interface-methods",
        "elided-literals" => "tests/fixtures/elided-literals",
        "highlighting-signatures" => "tests/fixtures/highlighting-signatures",
        "module-cache" => "tests/fixtures/modules/cache/app",
        "completion-index" => "tests/fixtures/completion-index",
        "go-checkout" => "tests/fixtures/modules/sdk/toolchain",
        "module-reference-scope" => "tests/fixtures/modules/scope/app",
        "module-sdk" | "module-sdk-scope" | "module-sdk-vendor" => "tests/fixtures/modules/sdk/app",
        "module-replace" => "tests/fixtures/modules/replace/app",
        "module-workspace" => "tests/fixtures/modules/work/app",
        "discovery" | "discovery-explicit" => "tests/fixtures/discovery",
        "symbol-packages" => "tests/fixtures/symbols",
        "promoted" => "tests/fixtures/promoted",
        "types" => "tests/fixtures/types",
        "methods"
        | "completion"
        | "completion-limit"
        | "definitions"
        | "type-navigation"
        | "handwritten-queries" => "tests/fixtures/methods",
        "calls" | "call-scopes" => "tests/fixtures/calls",
        "gopls-features" | "gopls-document-symbols" => "tests/fixtures/gopls-features",
        "gopls-formatting" => "tests/fixtures/format",
        _ => "tests/fixtures/imports",
    }
}

fn uses_temp_main_file(case: &str) -> bool {
    matches!(
        case,
        "protocol"
            | "navigation"
            | "build"
            | "experiments"
            | "experiments-env"
            | "experiments-override"
            | "symbol-packages"
            | "workspace-symbol-limit"
            | "completion-options"
            | "references-limit"
            | "go-checkout"
            | "highlighting-lexical"
            | "highlighting-negotiated"
            | "highlighting-identities"
            | "highlighting-identity-negotiated"
            | "highlighting-type-forms"
            | "highlighting-shadowing"
            | "highlighting-operations"
            | "range-inference"
            | "completion-edits-modern"
            | "completion-edits-legacy"
            | "generic-completion-plain"
            | "generic-completion-snippets"
            | "generic-completion-empty"
            | "completion-types"
            | "gopls-features"
            | "gopls-document-symbols"
            | "gopls-formatting"
    )
}

pub fn run_case(case: &str) {
    let base = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = base.join(fixture_rel(case));
    if case == "discovery" {
        let output = Command::new("go")
            .args(["list", "./..."])
            .env("GOWORK", "off")
            .current_dir(&root)
            .output()
            .expect("Go package discovery oracle");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let packages = String::from_utf8(output.stdout).expect("Go package paths");
        assert_eq!(
            packages.lines().collect::<Vec<_>>(),
            ["example.test/discovery", "example.test/discovery/visible"]
        );
    }
    let parent = root.parent().expect("fixture parent");
    let file = if uses_temp_main_file(case) {
        std::env::temp_dir().join("goquery-stdio-test.go")
    } else {
        root.join("main.go")
    };
    let dependency = parent.join(if case == "module-replace" {
        "dep source/dep.go"
    } else {
        "dep/dep.go"
    });
    let work_env = if matches!(
        case,
        "module-cache"
            | "module-replace"
            | "module-sdk"
            | "module-sdk-scope"
            | "module-sdk-vendor"
            | "go-checkout"
            | "generic-results"
            | "scoped-results"
            | "scoped-generic-unimported"
            | "indexed-scopes"
            | "sliced-scopes"
            | "range-scopes"
            | "bound-scopes"
            | "core-scopes"
            | "implication-scopes"
            | "receiver-scopes"
            | "field-scopes"
            | "parameter-scopes"
            | "builtin-scopes"
            | "package-values"
            | "test-variants"
            | "implementation"
            | "implementation-embedding"
            | "implementation-limits"
            | "generic-hierarchy"
            | "struct-hierarchy"
            | "interface-hierarchy"
            | "hierarchy-index"
    ) {
        std::ffi::OsString::from("off")
    } else if matches!(case, "module-workspace" | "module-reference-scope") {
        parent.join("go.work").into_os_string()
    } else {
        std::env::var_os("GOWORK").unwrap_or_default()
    };
    let checkout_root = if case == "go-checkout" {
        root.clone()
    } else {
        parent.join("toolchain")
    };
    let temp = std::env::temp_dir();
    let mut replacements: Vec<(&str, String)> = vec![
        (
            "$SDK_PATH",
            parent.join("toolchain").to_string_lossy().into_owned(),
        ),
        ("$DEP_URI", file_uri(&dependency)),
        ("$FILE_URI", file_uri(&file)),
        ("$ROOT_URI", dir_uri(&root)),
        ("$GEN_ROOT_URI", dir_uri(root.join("_gen"))),
    ];
    for (base_dir, entries) in [
        (
            parent,
            &[
                (
                    "$CACHE_UPPER_URI",
                    "downloaded/example.test/!upper@v1.0.0/dep.go",
                ),
                (
                    "$CACHE_SHARED_URI",
                    "downloaded/example.test/shared@v1.10.0/dep.go",
                ),
                (
                    "$CACHE_REPLACED_URI",
                    "downloaded/example.test/!new@v2.0.0+incompatible/dep.go",
                ),
                (
                    "$SDK_EXTERNAL_HTTPGUTS_URI",
                    "external/http/httpguts/httplex.go",
                ),
                ("$WRAPPER_URI", "wrapper/wrapper.go"),
                ("$SDK_UTF8_URI", "toolchain/src/unicode/utf8/utf8.go"),
                ("$SDK_URL_URI", "toolchain/src/net/url/url.go"),
                ("$SDK_TRIM_URI", "toolchain/src/internal/trim/trim.go"),
                ("$SDK_STRINGS_URI", "toolchain/src/strings/strings.go"),
                ("$SDK_BYTES_URI", "toolchain/src/bytes/bytes.go"),
                ("$ALT_DEP_URI", "alternate/dep.go"),
                ("$WORK_URI", "go.work"),
            ][..],
        ),
        (
            checkout_root.as_path(),
            &[
                ("$SDK_HTTP_TEST_URI", "src/net/http/header_test.go"),
                ("$SDK_DECODE_URI", "src/strings/decode.go"),
                ("$SDK_HTTP_OTHER_URI", "src/net/http/http.go"),
                ("$CMD_MAIN_URI", "src/cmd/go/main.go"),
                ("$CMD_NODER_URI", "src/cmd/compile/internal/noder/noder.go"),
                (
                    "$CMD_CONTEXT_URI",
                    "src/cmd/compile/internal/types2/context.go",
                ),
                (
                    "$CMD_MODFILE_URI",
                    "src/cmd/vendor/golang.org/x/mod/modfile/rule.go",
                ),
                (
                    "$CMD_SEMVER_URI",
                    "src/cmd/vendor/golang.org/x/mod/semver/semver.go",
                ),
                (
                    "$CMD_IDNA_URI",
                    "src/cmd/vendor/golang.org/x/net/idna/idna.go",
                ),
                ("$SDK_HTTP_HEADER_URI", "src/net/http/header.go"),
                (
                    "$SDK_HTTPGUTS_URI",
                    "src/vendor/golang.org/x/net/http/httpguts/httplex.go",
                ),
                ("$SDK_IDNA_URI", "src/vendor/golang.org/x/net/idna/idna.go"),
            ],
        ),
        (
            temp.as_path(),
            &[
                ("$WINDOWS_URI", "goquery-build_windows_amd64.go"),
                ("$LINUX_URI", "goquery-build_linux_arm64.go"),
                ("$EXTRA_URI", "goquery-reference-extra.go"),
            ],
        ),
        (
            root.as_path(),
            &[
                ("$MOD_URI", "go.mod"),
                ("$MID_URI", "mid/mid.go"),
                ("$MID_USE_URI", "mid/use.go"),
                ("$OTHER_URI", "other/other.go"),
                ("$SHADOW_URI", "shadow/shadow.go"),
                ("$SIBLING_URI", "sibling.go"),
                ("$HIDDEN_URI", ".hidden/hidden.go"),
                ("$GEN_URI", "_gen/gen.go"),
                ("$DATA_URI", "testdata/data.go"),
                ("$VENDOR_URI", "vendor/example.test/dep/dep.go"),
                ("$VISIBLE_URI", "visible/use.go"),
                ("$EXTERNAL_TEST_URI", "lib/lib_test.go"),
                ("$SUFFIX_URI", "suffix/suffix.go"),
                ("$SUFFIX_TEST_URI", "suffix/suffix_test.go"),
                ("$ONLY_TEST_URI", "testonly/only_test.go"),
                ("$INTERNAL_TEST_URI", "lib/internal_test.go"),
                ("$UNRELATED_TEST_URI", "other/other_test.go"),
                ("$LIB_URI", "lib/lib.go"),
            ],
        ),
    ] {
        for &(placeholder, rel) in entries {
            replacements.push((placeholder, file_uri(base_dir.join(rel))));
        }
    }
    let mut messages: Value = serde_json::from_str(
        &std::fs::read_to_string(base.join(format!("tests/golden/{case}.requests.json"))).unwrap(),
    )
    .unwrap();
    if let Ok(source) = std::fs::read_to_string(root.join("main.go")) {
        replace_string(&mut messages, "$SOURCE_FROM_URI", &source);
    }
    inject_synthetic_sources(case, &mut messages);
    for (placeholder, uri) in &replacements {
        replace_string(&mut messages, placeholder, uri);
    }
    for message in messages.as_array_mut().unwrap() {
        if message["method"] == "initialize" {
            // The existing goldens record the live-diagnostics session.
            let options = &mut message["params"]["initializationOptions"];
            options["diagnostics"]["onSave"] = false.into();
            if !matches!(
                case,
                "module-sdk" | "module-sdk-scope" | "module-sdk-vendor"
            ) {
                options["goroot"] = Value::String(String::new());
            }
        }
    }
    let mut child = Command::new(env!("CARGO_BIN_EXE_goquery"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("GOQUERY_TRACE_QUERIES", "1")
        .env("GOWORK", work_env)
        .env(
            "GOMODCACHE",
            if case == "module-cache" {
                parent.join("downloaded").into_os_string()
            } else {
                std::env::var_os("GOMODCACHE").unwrap_or_default()
            },
        )
        .env(
            "GOEXPERIMENT",
            if matches!(case, "experiments-env" | "experiments-override") {
                "none"
            } else {
                ""
            },
        )
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    for message in messages.as_array().unwrap() {
        jsonrpc2::send(&mut input, message.clone()).unwrap();
    }
    drop(input);
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "case {case}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    check_case_traces(case, &String::from_utf8_lossy(&output.stderr));

    let mut bytes = output.stdout.as_slice();
    let mut replies = Vec::new();
    while let Some(reply) = jsonrpc2::read_message(&mut bytes).unwrap() {
        replies.push(reply);
    }
    // Diagnostics are asynchronous notifications; the dedicated diagnostics case
    // checks them, while response goldens stay focused on request results.
    replies.retain(|reply| reply.get("id").is_some());
    verify_bounded_replies(case, &mut replies, &replacements);
    check_semantic_streams(&messages, &replies);

    let mut actual = Value::Array(replies);
    for (placeholder, uri) in &replacements {
        replace_string(&mut actual, uri, placeholder);
        if matches!(case, "module-replace" | "module-workspace" | "protocol") {
            replace_string(
                &mut actual,
                &format!("no package metadata for file {uri}"),
                &format!("no package metadata for file {placeholder}"),
            );
        }
    }
    assert_golden(case, &actual);
}

fn inject_synthetic_sources(case: &str, messages: &mut Value) {
    match case {
        "implementation-limits" => {
            let mut source = "package main\n".to_owned();
            for number in 0..16_385 {
                source.push_str(&format!("type Empty{number} struct{{}}\n"));
            }
            replace_string(messages, "$HIERARCHY_LIMIT_SOURCE", &source);
        }
        "workspace-symbol-limit" => {
            let mut source = "package main\n".to_owned();
            for number in (0..101).rev() {
                source.push_str(&format!("func BoundedCandidate{number:03}() {{}}\n"));
            }
            replace_string(messages, "$SYMBOL_LIMIT_SOURCE", &source);
            let mut source = "package main\ntype BoundedBox struct {\n".to_owned();
            for number in (0..101).rev() {
                source.push_str(&format!(" BoundedField{number:03} int\n"));
            }
            source.push_str("}\n");
            replace_string(messages, "$MEMBER_LIMIT_SOURCE", &source);
        }
        "completion-limit" => {
            for count in [199, 200] {
                replace_string(
                    messages,
                    &format!("$LIMIT_SOURCE_{count}"),
                    &completion_limit_source(count),
                );
            }
            let mut fuzzy = "package main\nfunc Probe() { _ = Foo }\n".to_owned();
            for number in (0..200).rev() {
                fuzzy.push_str(&format!("func AxxxFoo{number:03}() {{}}\n"));
            }
            fuzzy.push_str("func FooWinner() {}\n");
            replace_string(messages, "$FUZZY_LIMIT_SOURCE", &fuzzy);
            let mut library = "package lib\n".to_owned();
            for number in (0..200).rev() {
                library.push_str(&format!("func Candidate{number:08}() {{}}\n"));
            }
            library.push_str("func hidden() {}\n");
            replace_string(messages, "$IMPORT_LIMIT_SOURCE", &library);
        }
        "references-limit" => {
            let source = format!(
                "package main\nvar Target int\nfunc Probe() {{\nlocal := 0\n{}}}\n",
                "_ = local; _ = Target\n".repeat(4096)
            );
            replace_string(messages, "$REFERENCE_LIMIT_SOURCE", &source);
        }
        _ => {}
    }
}

fn check_case_traces(case: &str, trace: &str) {
    if case == "promoted" {
        assert!(trace.contains("references: 4/5 candidate files"), "{trace}");
        assert!(trace.contains("references: 1/5 candidate files"), "{trace}");
        assert!(
            trace.contains("incomingCalls: 4/5 candidate files"),
            "{trace}"
        );
    }
    if matches!(case, "definitions" | "type-navigation") {
        assert_eq!(
            trace
                .lines()
                .filter(|line| line.starts_with("textDocument/definition:"))
                .count(),
            if case == "definitions" { 22 } else { 25 },
            "{trace}"
        );
        if case == "definitions" {
            assert!(
                trace.contains("textDocument/prepareCallHierarchy: 1/2 candidate files"),
                "{trace}"
            );
            assert!(
                trace.contains("textDocument/prepareTypeHierarchy: 1/2 candidate files"),
                "{trace}"
            );
        }
    }
}

fn verify_bounded_replies(case: &str, replies: &mut [Value], replacements: &[(&str, String)]) {
    match case {
        "references-limit" => {
            let file_uri = &replacements
                .iter()
                .find(|(name, _)| *name == "$FILE_URI")
                .expect("file URI replacement")
                .1;
            for (id, character) in [(1, 4), (2, 15), (6, 4)] {
                let reply = replies
                    .iter_mut()
                    .find(|reply| reply["id"] == id)
                    .expect("reference reply");
                let result = reply["result"]
                    .as_array()
                    .expect("boundary reference result");
                assert_eq!(result.len(), 4096);
                let width = if id == 2 { 6 } else { 5 };
                for (index, location) in result.iter().enumerate() {
                    assert_eq!(location["uri"], *file_uri);
                    assert_eq!(
                        location["range"]["start"],
                        json!({"line": index + 4, "character": character})
                    );
                    assert_eq!(
                        location["range"]["end"],
                        json!({"line": index + 4, "character": character + width})
                    );
                }
                reply["result"] = json!({"verifiedLocations": result.len()});
            }
        }
        "workspace-symbol-limit" => {
            let first = replies.iter().find(|reply| reply["id"] == 1).unwrap()["result"].clone();
            for id in [1, 2] {
                let reply = replies.iter_mut().find(|reply| reply["id"] == id).unwrap();
                let result = reply["result"].as_array().expect("bounded symbols");
                assert_eq!(result.len(), 100);
                assert_eq!(reply["result"], first, "stable tied-result ranking");
                for (index, symbol) in result.iter().enumerate() {
                    assert_eq!(symbol["name"], format!("BoundedCandidate{index:03}"));
                    assert_eq!(symbol["location"]["range"]["start"]["line"], 101 - index);
                }
                reply["result"] = json!({"verifiedSymbols": 100});
            }
            let first = replies.iter().find(|reply| reply["id"] == 6).unwrap()["result"].clone();
            for id in [6, 7] {
                let reply = replies.iter_mut().find(|reply| reply["id"] == id).unwrap();
                let result = reply["result"].as_array().expect("bounded member symbols");
                assert_eq!(result.len(), 100);
                assert_eq!(reply["result"], first, "stable member ranking");
                for (index, symbol) in result.iter().enumerate() {
                    assert_eq!(symbol["name"], format!("BoundedBox.BoundedField{index:03}"));
                    assert_eq!(symbol["kind"], 8);
                    assert_eq!(
                        symbol["location"]["range"]["start"],
                        json!({"line": 102 - index, "character": 1})
                    );
                    assert_eq!(
                        symbol["location"]["range"]["end"],
                        json!({"line": 102 - index, "character": 16})
                    );
                }
                reply["result"] = json!({"verifiedMembers": 100});
            }
        }
        "completion-limit" => {
            let reply = replies.iter_mut().find(|reply| reply["id"] == 4).unwrap();
            let items = reply["result"]["items"]
                .as_array()
                .expect("ranked completion results");
            assert_eq!(items.len(), 200);
            assert_eq!(reply["result"]["isIncomplete"], true);
            assert_eq!(items[0]["label"], "FooWinner");
            for (index, item) in items.iter().enumerate() {
                assert_eq!(item["sortText"], format!("{index:05}"));
                if index > 0 {
                    assert_eq!(item["label"], format!("AxxxFoo{:03}", index - 1));
                }
            }
            reply["result"] = json!({"verifiedRankedCandidates": 200, "isIncomplete": true});
        }
        _ => {}
    }
}

fn check_semantic_streams(messages: &Value, replies: &[Value]) {
    let Some(legend) = replies
        .iter()
        .map(|reply| &reply["result"]["capabilities"]["semanticTokensProvider"]["legend"])
        .find(|legend| legend.is_object())
    else {
        return;
    };
    let kinds = legend["tokenTypes"].as_array().unwrap().len() as u32;
    let modifiers = legend["tokenModifiers"].as_array().unwrap().len() as u32;
    let mut sources = HashMap::new();
    for message in messages.as_array().unwrap() {
        let params = &message["params"];
        let uri = params["textDocument"]["uri"].as_str().unwrap_or("");
        match message["method"].as_str().unwrap() {
            "textDocument/didOpen" => {
                sources.insert(uri, params["textDocument"]["text"].as_str().unwrap());
            }
            "textDocument/didChange" => {
                sources.insert(uri, params["contentChanges"][0]["text"].as_str().unwrap());
            }
            "textDocument/semanticTokens/full" => {
                let reply = replies
                    .iter()
                    .find(|reply| reply["id"] == message["id"])
                    .unwrap();
                let Some(data) = reply["result"]["data"].as_array() else {
                    continue;
                };
                let Some(source) = sources.get(uri) else {
                    continue;
                };
                let lines: Vec<_> = source
                    .split('\n')
                    .map(|line| line.trim_end_matches('\r').encode_utf16().count() as u32)
                    .collect();
                let mut previous_end = (0u32, 0u32);
                for (line, column, length, kind, mods) in decode_semantic_tokens(data) {
                    assert!(length > 0 && kind < kinds && mods < (1 << modifiers));
                    assert!(
                        (line, column) >= previous_end,
                        "overlapping semantic tokens"
                    );
                    assert!(
                        column + length <= lines[line as usize],
                        "multiline semantic token"
                    );
                    previous_end = (line, column + length);
                }
            }
            _ => {}
        }
    }
}

fn completion_limit_source(count: usize) -> String {
    let mut source = "package main\nfunc Probe() { Candidate00000001 := 0; Candidate00000002 := 0; Candidate99999999 := 0; { type Candidate00000001 struct{}; Candidate } }\n".to_owned();
    for number in (0..count).rev() {
        source.push_str(&format!("func Candidate{number:08}() {{}}\n"));
    }
    source.push_str("func Candidate99999999() {}\n");
    source
}

fn assert_golden(case: &str, actual: &Value) {
    let golden =
        Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/golden/{case}.responses.json"));
    if std::env::var_os("UPDATE_GOLDEN").as_deref() == Some(std::ffi::OsStr::new("1")) {
        let encoded = format!("{}\n", serde_json::to_string_pretty(&actual).unwrap());
        if std::fs::read_to_string(&golden).ok().as_deref() != Some(encoded.as_str()) {
            std::fs::write(&golden, encoded)
                .unwrap_or_else(|error| panic!("write golden {}: {error}", golden.display()));
        }
    }
    let expected: Value =
        serde_json::from_str(&std::fs::read_to_string(&golden).expect("missing golden file"))
            .unwrap();
    assert_eq!(
        serde_json::to_string_pretty(&actual).unwrap(),
        serde_json::to_string_pretty(&expected).unwrap(),
        "golden response mismatch in {case}: review changes before UPDATE_GOLDEN=1 cargo test --test stdio"
    );
}

fn replace_string(value: &mut Value, from: &str, to: &str) {
    match value {
        Value::String(text) if text == from => *text = to.to_owned(),
        Value::Array(values) => {
            for value in values {
                replace_string(value, from, to);
            }
        }
        Value::Object(values) => {
            for value in values.values_mut() {
                replace_string(value, from, to);
            }
        }
        _ => {}
    }
}
