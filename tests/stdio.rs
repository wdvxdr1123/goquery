use serde_json::Value;
use std::io::Write;
use std::process::{Command, Stdio};

/// Bounded Content-Length framing for the test client (the server itself uses
/// tower-lsp's codec). Keeping it local removes a standalone workspace crate.
mod jsonrpc2 {
    use serde_json::Value;
    use std::io::{self, BufRead, Write};

    pub fn read_message(input: &mut impl BufRead) -> io::Result<Option<Value>> {
        let mut size = None;
        let mut header_bytes = 0;
        loop {
            let mut line = String::new();
            // Bound even a malformed header without a newline.
            let count =
                std::io::Read::take(&mut *input, 8193 - header_bytes).read_line(&mut line)?;
            if count == 0 {
                return if header_bytes == 0 {
                    Ok(None)
                } else {
                    Err(io::Error::other("truncated header"))
                };
            }
            header_bytes += count as u64;
            if header_bytes > 8192 {
                return Err(io::Error::other("header too large"));
            }
            if line == "\r\n" || line == "\n" {
                break;
            }
            if let Some((name, value)) = line.split_once(':')
                && name.eq_ignore_ascii_case("Content-Length")
            {
                if size.is_some() {
                    return Err(io::Error::other("duplicate Content-Length"));
                }
                size = Some(value.trim().parse::<usize>().map_err(io::Error::other)?);
            }
        }
        let size = size.ok_or_else(|| io::Error::other("missing Content-Length"))?;
        if size > 32 * 1024 * 1024 {
            return Err(io::Error::other("message too large"));
        }
        let mut body = vec![0; size];
        input.read_exact(&mut body)?;
        serde_json::from_slice(&body)
            .map(Some)
            .map_err(io::Error::other)
    }

    pub fn send(output: &mut impl Write, value: Value) -> io::Result<()> {
        let body = serde_json::to_vec(&value)?;
        write!(output, "Content-Length: {}\r\n\r\n", body.len())?;
        output.write_all(&body)?;
        output.flush()
    }
}

#[test]
fn stdio_lifecycle_and_unsaved_navigation() {
    check_tower_transport();
    run_case("protocol");
    run_case("navigation");
    run_case("imports");
    run_case("build");
    run_case("experiments");
    run_case("experiments-env");
    run_case("experiments-override");
    run_case("discovery");
    run_case("discovery-explicit");
    run_case("calls");
    run_case("call-scopes");
    run_case("methods");
    run_case("types");
    run_case("promoted");
    run_case("completion");
    run_case("completion-limit");
    run_case("workspace-symbol");
    run_case("member-symbols");
    run_case("aggregate-symbols");
    run_case("symbol-packages");
    run_case("workspace-symbol-limit");
    run_case("completion-snippets");
    run_case("completion-fuzzy");
    run_case("completion-members");
    run_case("completion-index");
    run_case("member-definitions");
    run_case("member-references");
    run_case("generic-members");
    run_case("alias-members");
    run_case("defined-members");
    run_case("promoted-generics");
    run_case("generic-methods");
    run_case("callable-results");
    run_case("completion-empty-snippets");
    run_case("completion-plain");
    run_case("completion-no-calls");
    run_case("completion-options");
    run_case("completion-signature-budget");
    run_case("references-limit");
    run_case("reference-index");
    run_case("qualified-references");
    run_case("unicode-identifiers");
    run_case("escaped-imports");
    run_case("package-import-names");
    run_case("test-variants");
    run_case("module-replace");
    run_case("module-cache");
    run_case("module-workspace");
    run_case("module-sdk");
    run_case("module-sdk-scope");
    run_case("module-sdk-vendor");
    run_case("go-checkout");
    run_case("module-reference-scope");
    run_case("handwritten-queries");
    run_case("highlighting");
    run_case("highlighting-signatures");
    run_case("elided-literals");
    run_case("interface-methods");
    run_case("generic-results");
    run_case("scoped-results");
    run_case("scoped-generic-unimported");
    run_case("indexed-scopes");
    run_case("sliced-scopes");
    run_case("range-scopes");
    run_case("bound-scopes");
    run_case("core-scopes");
    run_case("implication-scopes");
    run_case("receiver-scopes");
    run_case("field-scopes");
    run_case("parameter-scopes");
    run_case("builtin-scopes");
    run_case("package-values");
    run_case("implementation");
    run_case("implementation-embedding");
    run_case("implementation-limits");
    run_case("generic-hierarchy");
    run_case("struct-hierarchy");
    run_case("interface-hierarchy");
    run_case("hierarchy-index");
    run_case("highlighting-lexical");
    run_case("highlighting-negotiated");
    run_case("highlighting-identities");
    run_case("highlighting-identity-negotiated");
    run_case("highlighting-type-forms");
    run_case("highlighting-shadowing");
    run_case("highlighting-operations");
    run_case("range-inference");
    run_case("named-operands");
    run_case("generic-calls");
    run_case("constraint-calls");
    run_case("typeset-calls");
    run_case("native-method-calls");
    run_case("struct-calls");
    run_case("conversion-calls");
    run_case("signature-calls");
    run_case("argument-calls");
    run_case("interface-values");
    run_case("concrete-interface-calls");
    run_case("native-method-private");
    run_case("scoped-generic-private");
    run_case("completion-edits-modern");
    run_case("completion-edits-legacy");
    run_case("generic-completion-plain");
    run_case("generic-completion-snippets");
    run_case("generic-completion-empty");
    run_case("completion-types");
    run_case("completion-documentation-markdown");
    run_case("completion-documentation-plaintext");
    run_case("completion-documentation-disabled");
    run_case("signature-help");
    run_case("inferred-receivers");
    run_case("universe-forms");
    run_case("type-definition");
    run_case("numeric-builtins");
    run_case("method-values");
    run_case("assertions");
    run_case("type-switch");
    run_case("clause-scope");
    run_case("receive-select");
    run_case("local-bindings");
    run_case("dot-imports");
    run_case("definitions");
    run_case("type-navigation");
    run_case("gopls-features");
    run_case("gopls-document-symbols");
    run_case("gopls-formatting");
    check_symbol_matcher();
    check_completion_matcher();
    check_syntax_corpus();
    check_block_corpus();
    check_file_corpus();
    check_numeric_literals();
    check_identifier_categories();
    check_lexical_tokens();
    check_signature_corpus();
    check_type_corpus();
    check_aggregate_limits();
}

fn check_tower_transport() {
    fn frame(value: Value) -> Vec<u8> {
        let mut bytes = Vec::new();
        jsonrpc2::send(&mut bytes, value).unwrap();
        bytes
    }
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
    let exit = frame(serde_json::json!({"jsonrpc":"2.0","method":"exit"}));
    assert!(exchange(&exit, 1).is_empty());
    let mut lifecycle = frame(
        serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{},"initializationOptions":{"goroot":""}}}),
    );
    lifecycle.extend(frame(serde_json::json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///tower-transport.go","languageId":"go","version":1,"text":"package p\nvar _ = \"馃榾\"\n"}}})));
    lifecycle.extend(frame(
        serde_json::json!({"jsonrpc":"2.0","id":2,"method":"shutdown"}),
    ));
    lifecycle.extend(exit);
    let replies = exchange(&lifecycle, 0);
    assert_eq!(replies.len(), 2);
    assert_eq!(replies[0]["id"], 1);
    assert_eq!(
        replies[0]["result"]["capabilities"]["typeHierarchyProvider"],
        true
    );
    assert_eq!(replies[1]["id"], 2);
    assert!(replies[1]["result"].is_null());
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

fn run_case(case: &str) {
    let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let type_fixture = matches!(
        case,
        "type-definition"
            | "numeric-builtins"
            | "method-values"
            | "assertions"
            | "type-switch"
            | "clause-scope"
            | "receive-select"
            | "local-bindings"
    );
    let root = base.join(if type_fixture {
        "tests/fixtures/type-definition"
    } else if matches!(
        case,
        "implementation" | "implementation-embedding" | "implementation-limits"
    ) {
        "tests/fixtures/implementation"
    } else if matches!(case, "generic-hierarchy" | "hierarchy-index") {
        "tests/fixtures/generic-hierarchy"
    } else if case == "struct-hierarchy" {
        "tests/fixtures/struct-hierarchy"
    } else if case == "interface-hierarchy" {
        "tests/fixtures/interface-hierarchy"
    } else if case == "generic-results" {
        "tests/fixtures/generic-results"
    } else if matches!(
        case,
        "named-operands"
            | "generic-calls"
            | "constraint-calls"
            | "typeset-calls"
            | "native-method-calls"
            | "struct-calls"
            | "conversion-calls"
            | "signature-calls"
            | "argument-calls"
            | "interface-values"
    ) {
        "tests/fixtures/type-oracle"
    } else if case == "test-variants" {
        "tests/fixtures/test-variants"
    } else if matches!(
        case,
        "scoped-results"
            | "scoped-generic-unimported"
            | "indexed-scopes"
            | "sliced-scopes"
            | "range-scopes"
    ) {
        "tests/fixtures/scoped-results"
    } else if case == "interface-methods" {
        "tests/fixtures/interface-methods"
    } else if case == "elided-literals" {
        "tests/fixtures/elided-literals"
    } else if case == "highlighting-signatures" {
        "tests/fixtures/highlighting-signatures"
    } else if case == "module-cache" {
        "tests/fixtures/modules/cache/app"
    } else if case == "completion-index" {
        "tests/fixtures/completion-index"
    } else if case == "go-checkout" {
        "tests/fixtures/modules/sdk/toolchain"
    } else if case == "module-reference-scope" {
        "tests/fixtures/modules/scope/app"
    } else if matches!(
        case,
        "module-sdk" | "module-sdk-scope" | "module-sdk-vendor"
    ) {
        "tests/fixtures/modules/sdk/app"
    } else if case == "module-replace" {
        "tests/fixtures/modules/replace/app"
    } else if case == "module-workspace" {
        "tests/fixtures/modules/work/app"
    } else if matches!(case, "discovery" | "discovery-explicit") {
        "tests/fixtures/discovery"
    } else if case == "symbol-packages" {
        "tests/fixtures/symbols"
    } else if case == "promoted" {
        "tests/fixtures/promoted"
    } else if case == "types" {
        "tests/fixtures/types"
    } else if matches!(
        case,
        "methods"
            | "completion"
            | "completion-limit"
            | "definitions"
            | "type-navigation"
            | "handwritten-queries"
    ) {
        "tests/fixtures/methods"
    } else if matches!(case, "calls" | "call-scopes") {
        "tests/fixtures/calls"
    } else if matches!(case, "gopls-features" | "gopls-document-symbols") {
        "tests/fixtures/gopls-features"
    } else if case == "gopls-formatting" {
        "tests/fixtures/format"
    } else {
        "tests/fixtures/imports"
    });
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
    let file = if matches!(
        case,
        "completion-documentation-markdown"
            | "completion-documentation-plaintext"
            | "completion-documentation-disabled"
            | "imports"
            | "calls"
            | "call-scopes"
            | "methods"
            | "types"
            | "promoted"
            | "completion"
            | "completion-limit"
            | "definitions"
            | "type-navigation"
            | "handwritten-queries"
            | "dot-imports"
            | "reference-index"
            | "qualified-references"
            | "unicode-identifiers"
            | "escaped-imports"
            | "package-import-names"
            | "test-variants"
            | "module-cache"
            | "module-replace"
            | "module-workspace"
            | "module-sdk"
            | "module-sdk-scope"
            | "module-sdk-vendor"
            | "module-reference-scope"
            | "interface-methods"
            | "generic-results"
            | "named-operands"
            | "generic-calls"
            | "constraint-calls"
            | "typeset-calls"
            | "native-method-calls"
            | "struct-calls"
            | "conversion-calls"
            | "signature-calls"
            | "argument-calls"
            | "interface-values"
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
            | "implementation"
            | "implementation-embedding"
            | "implementation-limits"
            | "generic-hierarchy"
            | "struct-hierarchy"
            | "interface-hierarchy"
            | "hierarchy-index"
            | "elided-literals"
            | "highlighting-signatures"
            | "highlighting"
            | "signature-help"
            | "workspace-symbol"
            | "member-symbols"
            | "aggregate-symbols"
            | "completion-snippets"
            | "completion-fuzzy"
            | "completion-members"
            | "completion-index"
            | "member-definitions"
            | "member-references"
            | "generic-members"
            | "alias-members"
            | "defined-members"
            | "promoted-generics"
            | "generic-methods"
            | "callable-results"
            | "concrete-interface-calls"
            | "native-method-private"
            | "scoped-generic-private"
            | "completion-empty-snippets"
            | "completion-plain"
            | "completion-no-calls"
            | "completion-signature-budget"
            | "inferred-receivers"
            | "universe-forms"
            | "type-definition"
            | "numeric-builtins"
            | "method-values"
            | "assertions"
            | "type-switch"
            | "clause-scope"
            | "receive-select"
            | "local-bindings"
            | "discovery"
            | "discovery-explicit"
    ) {
        root.join("main.go")
    } else {
        std::env::temp_dir().join("goquery-stdio-test.go")
    };
    let dependency = root
        .parent()
        .expect("fixture parent")
        .join(if case == "module-replace" {
            "dep source/dep.go"
        } else {
            "dep/dep.go"
        });
    let work_env = if matches!(
        case,
        "module-replace"
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
        root.parent()
            .expect("fixture parent")
            .join("go.work")
            .into_os_string()
    } else {
        std::env::var_os("GOWORK").unwrap_or_default()
    };
    let checkout_root = if case == "go-checkout" {
        root.clone()
    } else {
        root.parent().unwrap().join("toolchain")
    };
    let replacements = [
        (
            "$CACHE_UPPER_URI",
            url::Url::from_file_path(
                root.parent()
                    .unwrap()
                    .join("downloaded/example.test/!upper@v1.0.0/dep.go"),
            )
            .unwrap()
            .to_string(),
        ),
        (
            "$CACHE_SHARED_URI",
            url::Url::from_file_path(
                root.parent()
                    .unwrap()
                    .join("downloaded/example.test/shared@v1.10.0/dep.go"),
            )
            .unwrap()
            .to_string(),
        ),
        (
            "$CACHE_REPLACED_URI",
            url::Url::from_file_path(
                root.parent()
                    .unwrap()
                    .join("downloaded/example.test/!new@v2.0.0+incompatible/dep.go"),
            )
            .unwrap()
            .to_string(),
        ),
        (
            "$SDK_HTTP_TEST_URI",
            url::Url::from_file_path(checkout_root.join("src/net/http/header_test.go"))
                .unwrap()
                .to_string(),
        ),
        (
            "$SDK_DECODE_URI",
            url::Url::from_file_path(checkout_root.join("src/strings/decode.go"))
                .unwrap()
                .to_string(),
        ),
        (
            "$SDK_HTTP_OTHER_URI",
            url::Url::from_file_path(checkout_root.join("src/net/http/http.go"))
                .unwrap()
                .to_string(),
        ),
        (
            "$CMD_MAIN_URI",
            url::Url::from_file_path(checkout_root.join("src/cmd/go/main.go"))
                .unwrap()
                .to_string(),
        ),
        (
            "$CMD_NODER_URI",
            url::Url::from_file_path(checkout_root.join("src/cmd/compile/internal/noder/noder.go"))
                .unwrap()
                .to_string(),
        ),
        (
            "$CMD_CONTEXT_URI",
            url::Url::from_file_path(
                checkout_root.join("src/cmd/compile/internal/types2/context.go"),
            )
            .unwrap()
            .to_string(),
        ),
        (
            "$CMD_MODFILE_URI",
            url::Url::from_file_path(
                checkout_root.join("src/cmd/vendor/golang.org/x/mod/modfile/rule.go"),
            )
            .unwrap()
            .to_string(),
        ),
        (
            "$CMD_SEMVER_URI",
            url::Url::from_file_path(
                checkout_root.join("src/cmd/vendor/golang.org/x/mod/semver/semver.go"),
            )
            .unwrap()
            .to_string(),
        ),
        (
            "$CMD_IDNA_URI",
            url::Url::from_file_path(
                checkout_root.join("src/cmd/vendor/golang.org/x/net/idna/idna.go"),
            )
            .unwrap()
            .to_string(),
        ),
        (
            "$SDK_HTTP_HEADER_URI",
            url::Url::from_file_path(checkout_root.join("src/net/http/header.go"))
                .unwrap()
                .to_string(),
        ),
        (
            "$SDK_HTTPGUTS_URI",
            url::Url::from_file_path(
                checkout_root.join("src/vendor/golang.org/x/net/http/httpguts/httplex.go"),
            )
            .unwrap()
            .to_string(),
        ),
        (
            "$SDK_IDNA_URI",
            url::Url::from_file_path(
                checkout_root.join("src/vendor/golang.org/x/net/idna/idna.go"),
            )
            .unwrap()
            .to_string(),
        ),
        (
            "$SDK_EXTERNAL_HTTPGUTS_URI",
            url::Url::from_file_path(
                root.parent()
                    .unwrap()
                    .join("external/http/httpguts/httplex.go"),
            )
            .unwrap()
            .to_string(),
        ),
        (
            "$WRAPPER_URI",
            url::Url::from_file_path(root.parent().unwrap().join("wrapper/wrapper.go"))
                .unwrap()
                .to_string(),
        ),
        (
            "$SDK_UTF8_URI",
            url::Url::from_file_path(
                root.parent()
                    .unwrap()
                    .join("toolchain/src/unicode/utf8/utf8.go"),
            )
            .unwrap()
            .to_string(),
        ),
        (
            "$SDK_URL_URI",
            url::Url::from_file_path(root.parent().unwrap().join("toolchain/src/net/url/url.go"))
                .unwrap()
                .to_string(),
        ),
        (
            "$SDK_TRIM_URI",
            url::Url::from_file_path(
                root.parent()
                    .unwrap()
                    .join("toolchain/src/internal/trim/trim.go"),
            )
            .unwrap()
            .to_string(),
        ),
        (
            "$SDK_PATH",
            root.parent()
                .unwrap()
                .join("toolchain")
                .to_string_lossy()
                .into_owned(),
        ),
        (
            "$SDK_STRINGS_URI",
            url::Url::from_file_path(
                root.parent()
                    .unwrap()
                    .join("toolchain/src/strings/strings.go"),
            )
            .unwrap()
            .to_string(),
        ),
        (
            "$SDK_BYTES_URI",
            url::Url::from_file_path(root.parent().unwrap().join("toolchain/src/bytes/bytes.go"))
                .unwrap()
                .to_string(),
        ),
        (
            "$DEP_URI",
            url::Url::from_file_path(&dependency).unwrap().to_string(),
        ),
        (
            "$ALT_DEP_URI",
            url::Url::from_file_path(root.parent().unwrap().join("alternate/dep.go"))
                .unwrap()
                .to_string(),
        ),
        (
            "$MOD_URI",
            url::Url::from_file_path(root.join("go.mod"))
                .unwrap()
                .to_string(),
        ),
        (
            "$WORK_URI",
            url::Url::from_file_path(root.parent().unwrap().join("go.work"))
                .unwrap()
                .to_string(),
        ),
        (
            "$MID_URI",
            url::Url::from_file_path(root.join("mid/mid.go"))
                .unwrap()
                .to_string(),
        ),
        (
            "$MID_USE_URI",
            url::Url::from_file_path(root.join("mid/use.go"))
                .unwrap()
                .to_string(),
        ),
        (
            "$WINDOWS_URI",
            url::Url::from_file_path(std::env::temp_dir().join("goquery-build_windows_amd64.go"))
                .expect("temporary file URI")
                .to_string(),
        ),
        (
            "$LINUX_URI",
            url::Url::from_file_path(std::env::temp_dir().join("goquery-build_linux_arm64.go"))
                .expect("temporary file URI")
                .to_string(),
        ),
        (
            "$FILE_URI",
            url::Url::from_file_path(file).unwrap().to_string(),
        ),
        (
            "$ROOT_URI",
            url::Url::from_directory_path(&root).unwrap().to_string(),
        ),
        (
            "$EXTRA_URI",
            url::Url::from_file_path(std::env::temp_dir().join("goquery-reference-extra.go"))
                .expect("temporary reference file URI")
                .to_string(),
        ),
        (
            "$OTHER_URI",
            url::Url::from_file_path(root.join("other/other.go"))
                .unwrap()
                .to_string(),
        ),
        (
            "$SHADOW_URI",
            url::Url::from_file_path(root.join("shadow/shadow.go"))
                .unwrap()
                .to_string(),
        ),
        (
            "$SIBLING_URI",
            url::Url::from_file_path(root.join("sibling.go"))
                .unwrap()
                .to_string(),
        ),
        (
            "$HIDDEN_URI",
            url::Url::from_file_path(root.join(".hidden/hidden.go"))
                .unwrap()
                .to_string(),
        ),
        (
            "$GEN_URI",
            url::Url::from_file_path(root.join("_gen/gen.go"))
                .unwrap()
                .to_string(),
        ),
        (
            "$GEN_ROOT_URI",
            url::Url::from_directory_path(root.join("_gen"))
                .unwrap()
                .to_string(),
        ),
        (
            "$DATA_URI",
            url::Url::from_file_path(root.join("testdata/data.go"))
                .unwrap()
                .to_string(),
        ),
        (
            "$VENDOR_URI",
            url::Url::from_file_path(root.join("vendor/example.test/dep/dep.go"))
                .unwrap()
                .to_string(),
        ),
        (
            "$VISIBLE_URI",
            url::Url::from_file_path(root.join("visible/use.go"))
                .unwrap()
                .to_string(),
        ),
        (
            "$EXTERNAL_TEST_URI",
            url::Url::from_file_path(root.join("lib/lib_test.go"))
                .unwrap()
                .to_string(),
        ),
        (
            "$SUFFIX_URI",
            url::Url::from_file_path(root.join("suffix/suffix.go"))
                .unwrap()
                .to_string(),
        ),
        (
            "$SUFFIX_TEST_URI",
            url::Url::from_file_path(root.join("suffix/suffix_test.go"))
                .unwrap()
                .to_string(),
        ),
        (
            "$ONLY_TEST_URI",
            url::Url::from_file_path(root.join("testonly/only_test.go"))
                .unwrap()
                .to_string(),
        ),
        (
            "$INTERNAL_TEST_URI",
            url::Url::from_file_path(root.join("lib/internal_test.go"))
                .unwrap()
                .to_string(),
        ),
        (
            "$UNRELATED_TEST_URI",
            url::Url::from_file_path(root.join("other/other_test.go"))
                .unwrap()
                .to_string(),
        ),
        (
            "$LIB_URI",
            url::Url::from_file_path(root.join("lib/lib.go"))
                .unwrap()
                .to_string(),
        ),
    ];
    let mut messages: Value = serde_json::from_str(
        &std::fs::read_to_string(base.join(format!("tests/golden/{case}.requests.json"))).unwrap(),
    )
    .unwrap();
    if let Ok(source) = std::fs::read_to_string(root.join("main.go")) {
        replace_string(&mut messages, "$SOURCE_FROM_URI", &source);
    }
    if case == "implementation-limits" {
        let mut source = "package main\n".to_owned();
        for number in 0..16_385 {
            source.push_str(&format!("type Empty{number} struct{{}}\n"));
        }
        replace_string(&mut messages, "$HIERARCHY_LIMIT_SOURCE", &source);
    }
    if case == "workspace-symbol-limit" {
        let mut source = "package main\n".to_owned();
        for number in (0..101).rev() {
            source.push_str(&format!("func BoundedCandidate{number:03}() {{}}\n"));
        }
        replace_string(&mut messages, "$SYMBOL_LIMIT_SOURCE", &source);
        let mut source = "package main\ntype BoundedBox struct {\n".to_owned();
        for number in (0..101).rev() {
            source.push_str(&format!(" BoundedField{number:03} int\n"));
        }
        source.push_str("}\n");
        replace_string(&mut messages, "$MEMBER_LIMIT_SOURCE", &source);
    }
    if case == "completion-limit" {
        for count in [199, 200] {
            replace_string(
                &mut messages,
                &format!("$LIMIT_SOURCE_{count}"),
                &completion_limit_source(count),
            );
        }
        let mut fuzzy = "package main\nfunc Probe() { _ = Foo }\n".to_owned();
        for number in (0..200).rev() {
            fuzzy.push_str(&format!("func AxxxFoo{number:03}() {{}}\n"));
        }
        fuzzy.push_str("func FooWinner() {}\n");
        replace_string(&mut messages, "$FUZZY_LIMIT_SOURCE", &fuzzy);
        let mut library = "package lib\n".to_owned();
        for number in (0..200).rev() {
            library.push_str(&format!("func Candidate{number:08}() {{}}\n"));
        }
        library.push_str("func hidden() {}\n");
        replace_string(&mut messages, "$IMPORT_LIMIT_SOURCE", &library);
    }
    if case == "references-limit" {
        let source = format!(
            "package main\nvar Target int\nfunc Probe() {{\nlocal := 0\n{}}}\n",
            "_ = local; _ = Target\n".repeat(4096)
        );
        replace_string(&mut messages, "$REFERENCE_LIMIT_SOURCE", &source);
    }
    for (placeholder, uri) in &replacements {
        replace_string(&mut messages, placeholder, uri);
    }
    for message in messages.as_array_mut().unwrap() {
        if message["method"] == "initialize"
            && !matches!(
                case,
                "module-sdk" | "module-sdk-scope" | "module-sdk-vendor"
            )
        {
            message["params"]["initializationOptions"]["goroot"] = Value::String(String::new());
        }
    }
    let mut child = Command::new(env!("CARGO_BIN_EXE_goquery"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("GOQUERY_TRACE_QUERIES", "1")
        .env(
            "GOWORK",
            if case == "module-cache" {
                std::ffi::OsString::from("off")
            } else {
                work_env
            },
        )
        .env(
            "GOMODCACHE",
            if case == "module-cache" {
                root.parent().unwrap().join("downloaded").into_os_string()
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
        let bytes = serde_json::to_vec(&message).unwrap();
        write!(input, "Content-Length: {}\r\n\r\n", bytes.len()).unwrap();
        input.write_all(&bytes).unwrap();
    }
    drop(input);
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "case {case}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    if case == "promoted" {
        let trace = String::from_utf8_lossy(&output.stderr);
        assert!(trace.contains("references: 4/5 candidate files"), "{trace}");
        assert!(trace.contains("references: 1/5 candidate files"), "{trace}");
        assert!(
            trace.contains("incomingCalls: 4/5 candidate files"),
            "{trace}"
        );
    }
    if matches!(case, "definitions" | "type-navigation") {
        let trace = String::from_utf8_lossy(&output.stderr);
        assert_eq!(
            trace
                .lines()
                .filter(|line| line.starts_with("textDocument/definition:"))
                .count(),
            if case == "definitions" { 22 } else { 24 },
            "{trace}"
        );
        assert!(
            case != "definitions"
                || trace.contains("textDocument/prepareCallHierarchy: 1/2 candidate files"),
            "{trace}"
        );
        assert!(
            case != "definitions"
                || trace.contains("textDocument/prepareTypeHierarchy: 1/2 candidate files"),
            "{trace}"
        );
    }
    let mut bytes = output.stdout.as_slice();
    let mut replies = Vec::new();
    while !bytes.is_empty() {
        let end = bytes
            .windows(4)
            .position(|s| s == b"\r\n\r\n")
            .expect("protocol header");
        let length: usize = std::str::from_utf8(&bytes[..end])
            .unwrap()
            .strip_prefix("Content-Length: ")
            .unwrap()
            .parse()
            .unwrap();
        replies.push(serde_json::from_slice::<Value>(&bytes[end + 4..end + 4 + length]).unwrap());
        bytes = &bytes[end + 4 + length..];
    }
    // Diagnostics are asynchronous notifications; the dedicated diagnostics case
    // checks them, while response goldens stay focused on request results.
    replies.retain(|reply| reply.get("id").is_some());
    if case == "references-limit" {
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
            for (index, location) in result.iter().enumerate() {
                assert_eq!(location["uri"], *file_uri);
                assert_eq!(
                    location["range"]["start"],
                    serde_json::json!({"line":index + 4,"character":character})
                );
                assert_eq!(
                    location["range"]["end"],
                    serde_json::json!({"line":index + 4,"character":character + if id == 2 {6} else {5}})
                );
            }
            reply["result"] = serde_json::json!({"verifiedLocations":result.len()});
        }
    }
    if case == "workspace-symbol-limit" {
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
            reply["result"] = serde_json::json!({"verifiedSymbols":100});
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
                    serde_json::json!({"line":102-index,"character":1})
                );
                assert_eq!(
                    symbol["location"]["range"]["end"],
                    serde_json::json!({"line":102-index,"character":16})
                );
            }
            reply["result"] = serde_json::json!({"verifiedMembers":100});
        }
    }
    if case == "completion-limit" {
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
        reply["result"] = serde_json::json!({"verifiedRankedCandidates":200,"isIncomplete":true});
    }
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

fn check_semantic_streams(messages: &Value, replies: &[Value]) {
    let Some(legend) = replies
        .iter()
        .map(|reply| &reply["result"]["capabilities"]["semanticTokensProvider"]["legend"])
        .find(|legend| legend.is_object())
    else {
        return;
    };
    let kinds = legend["tokenTypes"].as_array().unwrap().len();
    let modifiers = legend["tokenModifiers"].as_array().unwrap().len();
    let mut sources = std::collections::HashMap::new();
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
                    .map(|line| line.trim_end_matches('\r').encode_utf16().count() as u64)
                    .collect();
                assert_eq!(data.len() % 5, 0);
                let (mut line, mut column) = (0u64, 0u64);
                let mut previous_end = (0, 0);
                for token in data.chunks_exact(5) {
                    let token: Vec<_> = token.iter().map(|v| v.as_u64().unwrap()).collect();
                    line += token[0];
                    column = if token[0] == 0 {
                        column + token[1]
                    } else {
                        token[1]
                    };
                    assert!(token[2] > 0 && token[3] < kinds as u64 && token[4] < (1 << modifiers));
                    assert!(
                        (line, column) >= previous_end,
                        "overlapping semantic tokens"
                    );
                    assert!(
                        column + token[2] <= lines[line as usize],
                        "multiline semantic token"
                    );
                    previous_end = (line, column + token[2]);
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
    let golden = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("tests/golden/{case}.responses.json"));
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

fn check_syntax_corpus() {
    let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut cases = Vec::new();
    let mut expected_acceptance = Vec::new();
    for file in ["operators.go", "postfix.go", "composites.go", "invalid.go"] {
        let source = std::fs::read_to_string(base.join("tests/fixtures/frontend").join(file))
            .expect("read Go corpus");
        for line in source.lines() {
            if let Some(expression) = line.strip_prefix("var _ = ") {
                cases.push(expression.to_owned());
                expected_acceptance.push(file != "invalid.go");
            }
        }
    }
    for source in ["\"\\\n\"", "`\0`", "\"\u{feff}\"", "'\0'"] {
        cases.push(source.to_owned());
        expected_acceptance.push(false);
    }
    let accepted = official_acceptance(&cases, "");
    assert_eq!(
        accepted, expected_acceptance,
        "Go source corpus validity must match its category"
    );
    assert_eq!(
        accepted.len(),
        cases.len(),
        "oracle must inspect every expression"
    );
    for (source, valid) in cases.iter().zip(accepted) {
        let tree = goquery::syntax::parse_expression(source);
        assert_eq!(
            tree.diagnostics().is_empty(),
            valid,
            "Go parser acceptance differs for {source:?}: {:?}",
            tree.diagnostics()
        );
        for span in tree
            .nodes()
            .iter()
            .map(|n| &n.span)
            .chain(tree.diagnostics().iter().map(|d| &d.span))
        {
            assert!(
                span.start <= span.end && source.get(span.clone()).is_some(),
                "parser span must refer to valid source boundaries: {span:?} in {source:?}"
            );
        }
    }
    let nested = format!("{}x{}", "(".repeat(256), ")".repeat(256));
    assert!(
        !goquery::syntax::parse_expression(&nested)
            .diagnostics()
            .is_empty(),
        "deep nesting must be diagnosed without exhausting the stack"
    );
}

fn official_acceptance(cases: &[String], mode: &str) -> Vec<bool> {
    serde_json::from_value(official_output(cases, mode)).expect("oracle acceptance results")
}

fn official_output(cases: &[String], mode: &str) -> Value {
    let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
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

fn check_block_corpus() {
    let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut cases = Vec::new();
    let mut expected = Vec::new();
    for (file, valid) in [("blocks.go", true), ("blocks_invalid.go", false)] {
        let source = std::fs::read_to_string(base.join("tests/fixtures/frontend").join(file))
            .expect("read block corpus");
        for line in source.lines().filter(|line| line.starts_with("func ")) {
            cases.push(line[line.find('{').expect("corpus function body")..].to_owned());
            expected.push(valid);
        }
    }
    let accepted = official_acceptance(&cases, "--blocks");
    assert_eq!(
        accepted, expected,
        "official parser must confirm block corpus categories"
    );
    for (source, valid) in cases.iter().zip(accepted) {
        let tree = goquery::syntax::parse_block(source);
        assert_eq!(
            tree.diagnostics().is_empty(),
            valid,
            "block acceptance differs for {source:?}: {:?}",
            tree.diagnostics()
        );
        for entry in tree.type_parameters() {
            assert!(entry.declaration.index() < tree.statements().len());
            for parameter in &entry.parameters {
                assert!(source.get(parameter.annotation.clone()).is_some());
                assert!(
                    parameter
                        .name
                        .as_ref()
                        .is_some_and(|span| source.get(span.clone()).is_some())
                );
            }
        }
    }
    let deeply_nested = format!("{}{}", "{".repeat(256), "}".repeat(256));
    assert!(
        !goquery::syntax::parse_block(&deeply_nested)
            .diagnostics()
            .is_empty(),
        "block nesting must be bounded"
    );
}

fn check_file_corpus() {
    let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut cases: Vec<String> = serde_json::from_str(
        &std::fs::read_to_string(base.join("tests/fixtures/frontend/files.json"))
            .expect("file syntax corpus"),
    )
    .expect("Go source samples");
    for statement in [
        "go f",
        "defer 1",
        "go (f())",
        "defer (f())",
        "go int(0)",
        "defer []int(nil)",
        "go (f)()",
        "go f()",
        "defer f()",
        "_ = ~x",
        "if",
        "if }",
        "for",
        "for }",
        "switch",
        "switch }",
        "for range",
        "for range }",
    ] {
        cases.push(format!("package p\nfunc F() {{ {statement} }}"));
    }
    cases.push(format!(
        "package p\nfunc F() {{ if x {{}} {} else {{}} }}",
        "else if x {} ".repeat(400)
    ));
    let accepted = official_acceptance(&cases, "--files");
    assert!(
        accepted.contains(&true) && accepted.contains(&false),
        "file corpus needs positive and negative syntax"
    );
    for (source, accepted) in cases.iter().zip(accepted) {
        let diagnostics = goquery::syntax::validate_file(source);
        assert_eq!(
            diagnostics.is_empty(),
            accepted,
            "file acceptance differs for {source:?}: {diagnostics:?}"
        );
        for diagnostic in diagnostics {
            assert!(
                diagnostic.span.start <= diagnostic.span.end
                    && source.get(diagnostic.span).is_some(),
                "diagnostic must refer to this file"
            );
        }
    }
    // Truncate at every UTF-8 boundary, exercising editor snapshots independently
    // of acceptance. Check every public parser without snapshotting its trees.
    for source in [
        "package p\nfunc F(){ if x := f(); x { go f() } else if y { defer f() } }",
        "package p\nfunc F(){ for x := range values { switch x { case 1: select { case <-ch: } } } }",
        "package p\nfunc F(){ var 涓栫晫 = func(x map[string][]*T) { if x { } }; _ = 涓栫晫 }",
    ] {
        for end in source
            .char_indices()
            .map(|(index, _)| index)
            .chain(std::iter::once(source.len()))
        {
            let prefix = &source[..end];
            let diagnostics = std::panic::catch_unwind(|| {
                let mut diagnostics = goquery::syntax::validate_file(prefix);
                diagnostics.extend(goquery::syntax::parse_block(prefix).into_diagnostics());
                diagnostics.extend(goquery::syntax::parse_expression(prefix).into_diagnostics());
                diagnostics.extend(goquery::syntax::parse_type(prefix).into_diagnostics());
                diagnostics
            })
            .unwrap_or_else(|_| panic!("parser panic on {prefix:?}"));
            assert!(diagnostics.len() <= 4 * 4096);
            assert!(
                diagnostics
                    .into_iter()
                    .all(|diagnostic| diagnostic.span.start <= diagnostic.span.end
                        && prefix.get(diagnostic.span).is_some())
            );
        }
    }
    assert!(!goquery::syntax::parse_type("~T").diagnostics().is_empty());
    let errors = goquery::syntax::validate_file(&format!("package p\n{}", "@;\n".repeat(5000)));
    assert!(
        !errors.is_empty() && errors.len() <= 4096,
        "malformed files must have bounded diagnostics"
    );
    let nested = format!(
        "package p\nfunc F() {{ {}return{} }}",
        "func(){".repeat(160),
        "}();".repeat(160)
    );
    assert!(
        !goquery::syntax::validate_file(&nested).is_empty(),
        "nested file validation must be bounded"
    );
}

/// Large external corpus gate, explicitly enabled after preparing the pinned source.
#[test]
#[ignore = "prepare the pinned Go source corpus and set GOQUERY_GO_SOURCE"]
fn go_source_package_corpus() {
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use walkdir::WalkDir;

    let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
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
    let manifest: Value =
        serde_json::from_str(include_str!("go-corpus/skip-packages.json")).expect("skip manifest");
    assert_eq!(manifest["version"], "go1.27.3");
    let skipped = manifest["packages"]
        .as_object()
        .expect("exact package names with reasons");
    let source_root = root.join("src");
    let mut packages: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
    for entry in WalkDir::new(&source_root).follow_links(false) {
        let entry = entry.expect("source traversal must not silently drop errors");
        if entry.file_type().is_file() && entry.path().extension().is_some_and(|ext| ext == "go") {
            let parent = entry.path().parent().expect("source directory");
            let package = parent
                .strip_prefix(&source_root)
                .expect("source is inside corpus")
                .to_string_lossy()
                .replace('\\', "/");
            packages.entry(package).or_default().push(entry.into_path());
        }
    }
    for (package, reason) in skipped {
        assert!(
            packages.contains_key(package),
            "stale skip entry: {package}"
        );
        assert!(
            reason.as_str().is_some_and(|r| !r.trim().is_empty()),
            "missing skip reason for {package}"
        );
    }
    let selected = std::env::var("GOQUERY_GO_PACKAGE").ok();
    let scope = std::env::var("GOQUERY_GO_SCOPE").unwrap_or_default();
    assert!(
        matches!(scope.as_str(), "" | "stdlib" | "toolchain" | "all"),
        "unknown corpus scope: {scope}"
    );
    if let Some(package) = &selected {
        assert!(packages.contains_key(package), "unknown package {package}");
    }
    let oracle = std::env::temp_dir().join(format!(
        "goquery-corpus-oracle-{}{}",
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
        "external corpus oracle requires Go 1.27.3"
    );
    let build = Command::new("go")
        .arg("build")
        .arg("-o")
        .arg(&oracle)
        .arg(base.join("tests/oracle.go"))
        .output()
        .expect("build official oracle");
    assert!(
        build.status.success(),
        "oracle build failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );
    let mut checked_packages = 0;
    let mut checked_files = 0;
    let mut checked_bodies = 0;
    let mut checked_headers = 0;
    let mut checked_types = 0;
    let mut checked_initializers = 0;
    let mut failures = Vec::new();
    let mut classifications = Vec::new();
    for (package, mut files) in packages {
        let enabled = if let Some(selected) = &selected {
            selected == &package
        } else if scope == "all" {
            true
        } else if scope == "toolchain" {
            !package.split('/').any(|part| part == "testdata")
        } else if scope == "stdlib" {
            !package
                .split('/')
                .any(|part| matches!(part, "cmd" | "testdata"))
        } else {
            !skipped.contains_key(&package)
        };
        if !enabled {
            continue;
        }
        files.sort();
        let mut child = Command::new(&oracle)
            .arg("--file-bodies")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("launch file oracle");
        child
            .stdin
            .take()
            .expect("oracle stdin")
            .write_all(&serde_json::to_vec(&files).expect("encode filenames"))
            .expect("send filenames");
        let output = child.wait_with_output().expect("wait for file oracle");
        assert!(
            output.status.success(),
            "oracle failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let reports: Vec<Value> =
            serde_json::from_slice(&output.stdout).expect("file oracle reports");
        assert_eq!(reports.len(), files.len(), "oracle must inspect every file");
        let mut package_bodies = 0;
        let mut package_headers = 0;
        let mut package_types = 0;
        let mut package_initializers = 0;
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
            classifications.push(serde_json::json!({"file": file.strip_prefix(&source_root).unwrap(), "parser_accepted": accepted, "parser_errors": report["error_count"], "native_diagnostics": file_diagnostics.len(), "stub_symbols": stub.symbols.len(), "constant_groups": stub.constant_groups.len()}));
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
        let report = serde_json::json!({"version": "go1.27.3", "scope": scope, "packages": checked_packages, "files": classifications, "failures": failures});
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

fn check_lexical_tokens() {
    let streams: Vec<String> = [
        "0x1e+2; 1.foo; 3...; 123abc; 0b10+2; 0x1.fp-2; .5e+1i",
        "package p\nvar x = 1 /* comment\n */ +2\n",
        "\u{feff}package p\r\nvar 涓栫晫 = `raw\r\ntext`\n",
        "x++ // comment\n y := <-ch; z &^= 1",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    let mut sources:Vec<String>="break case chan const continue default defer else fallthrough for func go goto if import interface map package range return select struct switch type var + - * / % & | ^ << >> &^ += -= *= /= %= &= |= ^= <<= >>= &^= && || <- ++ -- == < > = ! != <= >= := ... ( [ { , . ) ] } ; : ~".split_whitespace().map(str::to_owned).collect();
    sources.extend(
        [
            "value", "true", "0xdead", "1.0", "1e-3", "0x1.fp2", "3i", "'x'", "\"text\"", "`raw`",
        ]
        .into_iter()
        .map(str::to_owned),
    );
    sources.extend(streams);
    let expected = official_output(&sources, "--token-streams");
    let expected = expected.as_array().expect("token streams");
    assert_eq!(sources.len(), expected.len());
    for (source, tokens) in sources.iter().zip(expected) {
        check_token_stream(source, tokens.as_array().expect("stream tokens"));
        let plain: Vec<_> = goquery::syntax::Lexer::new(source).collect();
        let with_comments: Vec<_> = goquery::syntax::Lexer::with_comments(source)
            .filter(|token| token.kind != goquery::syntax::TokenKind::Comment)
            .collect();
        assert_eq!(
            plain, with_comments,
            "comments changed semicolon insertion: {source:?}"
        );
    }
}

fn check_token_stream(source: &str, expected: &[Value]) {
    use goquery::syntax::TokenKind;
    let mut actual = goquery::syntax::Lexer::new(source);
    for expected in expected {
        let token = actual.next().expect("scanner token missing");
        let kind = token.kind.fixed_text().unwrap_or(match token.kind {
            TokenKind::Ident => "IDENT",
            TokenKind::Int => "INT",
            TokenKind::Float => "FLOAT",
            TokenKind::Imag => "IMAG",
            TokenKind::Char => "CHAR",
            TokenKind::String => "STRING",
            _ => "ILLEGAL",
        });
        assert_eq!(
            kind,
            expected["kind"].as_str().expect("token kind"),
            "token at byte {} in {source:?}",
            token.span.start
        );
        assert_eq!(
            token.span.start,
            expected["start"].as_u64().expect("token start") as usize
        );
        assert_eq!(
            token.span.end,
            expected["end"].as_u64().expect("token end") as usize
        );
        assert_eq!(
            token.inserted,
            expected["inserted"].as_bool().expect("semicolon insertion")
        );
    }
    assert!(actual.next().is_none(), "unexpected trailing scanner token");
}

fn check_signature_corpus() {
    let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut sources = Vec::new();
    let mut validity = Vec::new();
    for (file, valid) in [("signatures.go", true), ("invalid-signatures.go", false)] {
        let source = std::fs::read_to_string(base.join("tests/fixtures/frontend").join(file))
            .expect("signature corpus");
        for line in source.lines().filter(|line| line.starts_with("func ")) {
            sources.push(line.to_owned());
            validity.push(valid);
        }
    }
    assert_eq!(official_acceptance(&sources, "--signatures"), validity);
    for (source, valid) in sources.iter().zip(validity) {
        let signature = goquery::syntax::parse_signature(source);
        assert_eq!(
            signature.diagnostics().is_empty(),
            valid,
            "signature {source:?}: {:?}",
            signature.diagnostics()
        );
        for parameter in signature
            .receiver
            .iter()
            .chain(&signature.type_parameters)
            .chain(&signature.parameters)
            .chain(&signature.results)
        {
            assert!(source.get(parameter.annotation.clone()).is_some());
            if let Some(name) = &parameter.name {
                assert!(source.get(name.clone()).is_some());
            }
        }
    }
    let nested = format!("func Deep(f {}int)", "func() ".repeat(256));
    assert!(
        !goquery::syntax::parse_signature(&nested)
            .diagnostics()
            .is_empty()
    );
}

fn check_type_corpus() {
    let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut cases = Vec::new();
    let mut validity = Vec::new();
    for (file, valid) in [("types.go", true), ("invalid-types.go", false)] {
        let source = std::fs::read_to_string(base.join("tests/fixtures/frontend").join(file))
            .expect("type corpus");
        for line in source.lines() {
            if let Some(ty) = line.strip_prefix("type _ = ") {
                cases.push(ty.to_owned());
                validity.push(valid);
            }
        }
    }
    assert_eq!(official_acceptance(&cases, "--types"), validity);
    for (source, valid) in cases.iter().zip(validity) {
        let parsed = goquery::syntax::parse_type(source);
        assert_eq!(
            parsed.diagnostics().is_empty(),
            valid,
            "type {source:?}: {:?}",
            parsed.diagnostics()
        );
        check_type_spans(source, &parsed);
        if valid {
            let delimited = format!("{source}, bool");
            let prefix = goquery::syntax::parse_type_prefix(&delimited);
            assert!(prefix.diagnostics().is_empty(), "type prefix {source:?}");
            assert_eq!(prefix.span, parsed.span);
            check_type_spans(&delimited, &prefix);
            assert!(
                !goquery::syntax::parse_type(&delimited)
                    .diagnostics()
                    .is_empty()
            );
        }
    }
    let nested = format!("{}int{}", "struct{ Field ".repeat(256), "}".repeat(256));
    assert!(
        !goquery::syntax::parse_type(&nested)
            .diagnostics()
            .is_empty()
    );
}

fn check_type_spans(source: &str, parsed: &goquery::syntax::TypeSyntax) {
    for member in &parsed.interface_members {
        assert_eq!(source.as_bytes().get(member.container), Some(&b'{'));
        assert!(member.container < member.annotation.start);
        assert!(source.get(member.annotation.clone()).is_some());
        if let Some(name) = &member.name {
            assert!(member.container < name.start && name.end <= member.annotation.start);
            assert!(source.get(name.clone()).is_some());
        }
    }
    for reference in &parsed.references {
        assert!(source.get(reference.name.clone()).is_some());
        if let Some(qualifier) = &reference.qualifier {
            assert!(source.get(qualifier.clone()).is_some());
        }
    }
    for field in &parsed.fields {
        assert_eq!(source.as_bytes().get(field.container), Some(&b'{'));
        assert!(field.container < field.name.start);
        if field.embedded {
            assert!(field.annotation.start <= field.name.start);
            assert!(field.name.end <= field.annotation.end);
        } else {
            assert!(field.name.end <= field.annotation.start);
        }
        assert!(source.get(field.name.clone()).is_some());
        assert!(source.get(field.annotation.clone()).is_some());
        if let Some(tag) = &field.tag {
            assert!(field.annotation.end <= tag.start);
            assert!(goquery::syntax::decode_string_literal(&source[tag.clone()]).is_some());
        }
    }
}

// Exercise the matcher library's byte-offset and fixed-buffer boundaries;
// protocol cases above cover ranking, qualification, updates and result limits.
fn check_symbol_matcher() {
    let matcher = goquery::fuzzy::Matcher::new("srHS");
    assert!(matcher.score(&["SearchHTTPServer"]).is_some());
    assert!(matcher.score(&["unrelated"]).is_none());
    let matcher = goquery::fuzzy::Matcher::new("Method");
    let hit = matcher.score(&["涓栫晫.", "Box.", "Method"]).unwrap();
    assert_eq!(hit.start, "涓栫晫.Box.".len());
    assert!((hit.score - 5.5 / 6.0).abs() < 1e-12);
    assert!(goquery::fuzzy::Matcher::new("").score(&["Name"]).is_none());
    assert!(goquery::fuzzy::Matcher::new("Name").score(&[]).is_none());
    let long = "x".repeat(256);
    assert!(
        goquery::fuzzy::Matcher::new(&long)
            .score(&[&long])
            .is_some()
    );
    assert!(
        goquery::fuzzy::Matcher::new("Z")
            .score(&[&long, "Z"])
            .is_none()
    );
}

// Grouped nested anonymous fields can multiply the number of search paths.
fn check_aggregate_limits() {
    for declaration in [
        "= pkg.Box[struct { First int; Second string }]",
        "[T any] = Box[T]",
        "= *Box[int]",
    ] {
        let stub = goquery::syntax::stub(&format!("package p\ntype Alias {declaration}\n"));
        let target = stub.symbols[0]
            .type_info
            .as_ref()
            .and_then(|info| info.target.as_ref())
            .expect("named alias metadata");
        assert!(
            goquery::syntax::parse_type(&target.name)
                .diagnostics()
                .is_empty(),
            "{target:?}"
        );
    }
    for declaration in [
        "struct { Box[int] }",
        "[T any] struct { pkg.Box[T] }",
        "struct { Box[struct { First int; Second string }] }",
    ] {
        let stub = goquery::syntax::stub(&format!("package p\ntype Holder {declaration}\n"));
        let info = stub.symbols[0]
            .type_info
            .as_ref()
            .expect("aggregate metadata");
        assert_eq!(info.embedded.len(), 1, "{declaration}");
        for spelling in &info.embedded {
            assert!(
                goquery::syntax::parse_type(spelling)
                    .diagnostics()
                    .is_empty(),
                "{spelling}"
            );
        }
    }
    for source in [
        "struct { A, B }",
        "interface { Broken(",
        "struct { Field string `tag` }",
    ] {
        for member in goquery::syntax::aggregate_members(source) {
            assert!(source.get(member.selection).is_some());
            assert!(source.get(member.annotation).is_some());
        }
    }
    let mut source = "struct { Leaf int }".to_owned();
    for _ in 0..20 {
        source = format!("struct {{ A, B {source} }}");
    }
    let members = goquery::syntax::aggregate_members(&source);
    assert!(!members.is_empty() && members.len() <= 4096);
    assert!(
        members
            .iter()
            .map(|member| member.symbol.name.len())
            .sum::<usize>()
            <= 256 * 1024
    );
    for member in &members {
        assert!(source.get(member.selection.clone()).is_some());
        assert!(source.get(member.annotation.clone()).is_some());
        assert!(member.container <= member.selection.start);
    }
    let stub = goquery::syntax::stub(&format!("package p\ntype Bounded {source}\n"));
    let info = stub.symbols[0]
        .type_info
        .as_ref()
        .expect("aggregate metadata");
    assert!(info.search_members.len() <= 4096);
    assert!(
        info.search_members
            .iter()
            .map(|member| member.name.len())
            .sum::<usize>()
            <= 256 * 1024
    );
}

// Expected scores come from the pinned Go implementation, not the Rust port.
fn check_completion_matcher() {
    let oracle: Value = serde_json::from_str(include_str!("fixtures/fuzzy/scores.json")).unwrap();
    for group in oracle["groups"].as_array().unwrap() {
        let pattern = group["pattern"].as_str().unwrap();
        let mut matcher = goquery::fuzzy::CompletionMatcher::new(pattern);
        for case in group["candidates"].as_array().unwrap() {
            let candidate = case["text"].as_str().unwrap();
            let actual = matcher.score(candidate);
            let expected = case["score"].as_f64().unwrap();
            assert!(
                (f64::from(actual) - expected).abs() < 1e-7,
                "{pattern:?}/{candidate:?}: {actual} vs {expected}"
            );
        }
    }
}

// Compare acceptance in expression, declaration, type and lazy nested-body paths.
fn check_numeric_literals() {
    let mut samples: Vec<String> = [
        "0",
        "00",
        "0755",
        "089",
        "08i",
        "0b10",
        "0B10i",
        "0b0190",
        "0b2i",
        "0o755",
        "0O755i",
        "0o9",
        "0o9i",
        "0xabc",
        "0XABi",
        "0x",
        "0b",
        "0o",
        "0x_",
        "0b_",
        "0o_",
        "1_",
        "1__0",
        "0x__1",
        "0_1",
        "0x_1",
        "0b_1",
        "0o_7",
        "1.25",
        ".5",
        "1.",
        "08.5",
        "0x1.fp2",
        "0x.8p-2",
        "0x1p0i",
        "0x1.0",
        "0x.p1",
        "0b1.0",
        "0o7.1i",
        "0b1e2",
        "0o1P-1",
        "1p2",
        "1e",
        "1E+",
        "1e-",
        "1e_2",
        "1e+_2",
        "1e2_",
        "1e2__3",
        "1e2i",
        "0x1p",
        "0x1p+",
        "0x1p_2",
        "0x1p+_2",
        "0x1p2_",
        "0x1p2__3",
        "1_.0",
        "1._0",
        "0x_1p0",
        "0x1_p0",
        "0x1p0_i",
        "0x1g",
        "0b01a0",
        "1+2i",
        "0x1e+2",
        "0x1e-2",
        "1...",
        "1..2",
        "0x1.2p3+4",
        "0x1.2e3",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    for literal in [
        "0", "0755", "0b101", "0o755", "0xabc", "1.25", "1e+3", "0x1.fp-2", "10i",
    ] {
        for offset in 0..=literal.len() {
            let mut sample = literal.to_owned();
            sample.insert(offset, '_');
            samples.push(sample);
        }
    }
    samples.sort();
    samples.dedup();
    let accepted = official_acceptance(&samples, "");
    let mut files = Vec::new();
    let mut valid_tokens = Vec::new();
    for (sample, accepted) in samples.iter().zip(accepted) {
        let parsed = goquery::syntax::parse_expression(sample);
        assert_eq!(
            parsed.diagnostics().is_empty(),
            accepted,
            "numeric expression {sample:?}: {:?}",
            parsed.diagnostics()
        );
        if accepted {
            valid_tokens.push(sample.clone());
        }
        files.push(format!("package p\nvar X = {sample}\n"));
        files.push(format!("package p\ntype T [{sample}]byte\n"));
        files.push(format!(
            "package p\nfunc F() {{ func(){{ _ = {sample} }}() }}\n"
        ));
    }
    let accepted = official_acceptance(&files, "--files");
    for (source, accepted) in files.iter().zip(accepted) {
        let diagnostics = goquery::syntax::validate_file(source);
        assert_eq!(
            diagnostics.is_empty(),
            accepted,
            "numeric file {source:?}: {diagnostics:?}"
        );
        for diagnostic in diagnostics {
            assert!(source.get(diagnostic.span).is_some());
        }
    }
    let streams = official_output(&valid_tokens, "--token-streams");
    for (source, tokens) in valid_tokens
        .iter()
        .zip(streams.as_array().expect("numeric token streams"))
    {
        check_token_stream(source, tokens.as_array().expect("numeric tokens"));
    }
}

// Check the public lexer against Go categories for every Unicode scalar value.
fn check_identifier_categories() {
    let categories = official_output(&[], "--identifier-categories");
    assert_eq!(categories["Version"], "17.0.0", "pinned Go Unicode version");
    let mut expected = vec![0u8; 0x110000];
    for (category, bit) in [("Letters", 1), ("Digits", 2), ("Uppercase", 4)] {
        for width in ["R16", "R32"] {
            for range in categories[category][width]
                .as_array()
                .expect("Go category ranges")
            {
                let low = range["Lo"].as_u64().expect("lower bound") as usize;
                let high = range["Hi"].as_u64().expect("upper bound") as usize;
                let stride = range["Stride"].as_u64().expect("range stride") as usize;
                for value in (low..=high).step_by(stride) {
                    expected[value] |= bit;
                }
            }
        }
    }
    let mut bytes = [0u8; 5];
    bytes[0] = b'a';
    for (value, category) in expected.iter().enumerate() {
        let Some(character) = char::from_u32(value as u32) else {
            continue;
        };
        let length = 1 + character.encode_utf8(&mut bytes[1..]).len();
        let source = std::str::from_utf8(&bytes[..length]).expect("encoded scalar");
        assert_eq!(
            goquery::syntax::is_exported_name(&source[1..]),
            category & 4 != 0,
            "exported U+{value:04X}"
        );
        let start = goquery::syntax::Lexer::new(&source[1..])
            .next()
            .is_some_and(|token| {
                token.kind == goquery::syntax::TokenKind::Ident && token.span.end == length - 1
            });
        let continuation = goquery::syntax::Lexer::new(source)
            .next()
            .is_some_and(|token| {
                token.kind == goquery::syntax::TokenKind::Ident && token.span.end == length
            });
        assert_eq!(
            start,
            character == '_' || category & 1 != 0,
            "identifier start U+{value:04X}"
        );
        assert_eq!(
            continuation,
            character == '_' || *category != 0,
            "identifier continuation U+{value:04X}"
        );
    }
    let mut files = Vec::new();
    for value in [
        0xaa, 0xb2, 0x345, 0x5b0, 0x660, 0x203f, 0x2160, 0x10400, 0x104a0, 0x1d400, 0x31350,
    ] {
        let character = char::from_u32(value).expect("boundary scalar");
        files.push(format!("package p\nvar {character}Name = 1\n"));
        files.push(format!("package p\nvar a{character}Name = 1\n"));
    }
    let accepted = official_acceptance(&files, "--files");
    let mut valid = Vec::new();
    for (source, accepted) in files.iter().zip(accepted) {
        let diagnostics = goquery::syntax::validate_file(source);
        assert_eq!(
            diagnostics.is_empty(),
            accepted,
            "identifier file {source:?}: {diagnostics:?}"
        );
        if accepted {
            valid.push(source.clone());
        }
    }
    let streams = official_output(&valid, "--token-streams");
    for (source, stream) in valid
        .iter()
        .zip(streams.as_array().expect("identifier streams"))
    {
        check_token_stream(source, stream.as_array().expect("identifier tokens"));
    }
}

/// External cross-package edits (agent or shell writes) must be visible without
/// the editor opening the changed files: the server registers file watching,
/// reindexes watched files, and republishes diagnostics for open documents in
/// the changed package and its direct importers.
#[test]
fn watched_file_changes_refresh_cross_package_state() {
    use std::sync::{Arc, Mutex, mpsc};
    use std::time::{Duration, Instant};

    struct Server {
        child: std::process::Child,
        input: Arc<Mutex<std::process::ChildStdin>>,
        messages: mpsc::Receiver<Value>,
        requests: Arc<Mutex<Vec<Value>>>,
        next_id: i64,
    }

    impl Server {
        fn new(child: std::process::Child) -> Self {
            Self::spawn(child, true)
        }

        fn manual(child: std::process::Child) -> Self {
            Self::spawn(child, false)
        }

        fn spawn(mut child: std::process::Child, auto_reply: bool) -> Self {
            let input = Arc::new(Mutex::new(child.stdin.take().expect("server stdin")));
            let stdout = child.stdout.take().expect("server stdout");
            let requests = Arc::new(Mutex::new(Vec::new()));
            let (sender, messages) = mpsc::channel();
            let reader_input = input.clone();
            let reader_requests = requests.clone();
            std::thread::spawn(move || {
                let mut reader = std::io::BufReader::new(stdout);
                while let Ok(Some(message)) = jsonrpc2::read_message(&mut reader) {
                    if message.get("method").is_some()
                        && let Some(id) = message.get("id")
                    {
                        if matches!(
                            message["method"].as_str(),
                            Some("client/registerCapability" | "client/unregisterCapability")
                        ) {
                            reader_requests
                                .lock()
                                .expect("request lock")
                                .push(message.clone());
                        }
                        if auto_reply {
                            let response = serde_json::json!({
                                "jsonrpc": "2.0",
                                "id": id,
                                "result": Value::Null,
                            });
                            let mut input = reader_input.lock().expect("stdin lock");
                            let _ = jsonrpc2::send(&mut *input, response);
                        }
                    } else {
                        let _ = sender.send(message);
                    }
                }
            });
            Self {
                child,
                input,
                messages,
                requests,
                next_id: 0,
            }
        }

        fn reply(&self, request: &Value) {
            self.send(&serde_json::json!({
                "jsonrpc": "2.0",
                "id": request["id"],
                "result": Value::Null,
            }));
        }

        fn send(&self, message: &Value) {
            let mut input = self.input.lock().expect("stdin lock");
            jsonrpc2::send(&mut *input, message.clone()).expect("send LSP message");
        }

        fn notify(&self, method: &str, params: Value) {
            self.send(&serde_json::json!({
                "jsonrpc": "2.0",
                "method": method,
                "params": params,
            }));
        }

        fn request(&mut self, method: &str, params: Value) -> Value {
            self.next_id += 1;
            let id = self.next_id;
            let mut message = serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": method,
            });
            if !params.is_null() {
                message["params"] = params;
            }
            self.send(&message);
            let deadline = Instant::now() + Duration::from_secs(30);
            loop {
                let remaining = deadline.saturating_duration_since(Instant::now());
                let message = self
                    .messages
                    .recv_timeout(remaining)
                    .unwrap_or_else(|_| panic!("timeout waiting for {method}"));
                if message.get("id") == Some(&serde_json::json!(id))
                    && message.get("method").is_none()
                {
                    return message;
                }
            }
        }

        fn wait_request(&self, method: &str) -> Value {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                {
                    let mut requests = self.requests.lock().expect("request lock");
                    if let Some(position) = requests
                        .iter()
                        .position(|request| request["method"] == method)
                    {
                        return requests.remove(position);
                    }
                }
                assert!(Instant::now() < deadline, "server did not send {method}");
                std::thread::sleep(Duration::from_millis(5));
            }
        }

        fn wait_registration(&self) -> Value {
            self.wait_request("client/registerCapability")["params"].clone()
        }

        fn wait_diagnostics(
            &self,
            expected: &[&str],
        ) -> std::collections::HashMap<String, Vec<Value>> {
            let deadline = Instant::now() + Duration::from_secs(15);
            let mut found = std::collections::HashMap::new();
            let mut seen = Vec::new();
            while found.len() < expected.len() {
                let remaining = deadline.saturating_duration_since(Instant::now());
                assert!(
                    !remaining.is_zero(),
                    "missing diagnostics for {expected:?}: {found:?} seen {seen:?}"
                );
                let Ok(message) = self.messages.recv_timeout(remaining) else {
                    panic!("missing diagnostics for {expected:?}: {found:?} seen {seen:?}");
                };
                if message["method"] != "textDocument/publishDiagnostics" {
                    seen.push(message);
                    continue;
                }
                let uri = message["params"]["uri"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned();
                if expected.contains(&uri.as_str()) {
                    found.insert(
                        uri,
                        message["params"]["diagnostics"]
                            .as_array()
                            .cloned()
                            .unwrap_or_default(),
                    );
                } else {
                    seen.push(message);
                }
            }
            found
        }

        /// Wait for a diagnostics notification for `uri` whose messages contain
        /// `needle`. Initial empty publishes (from didOpen) are skipped.
        fn wait_diagnostic_containing(&self, uri: &str, needle: &str) -> Vec<Value> {
            let deadline = Instant::now() + Duration::from_secs(15);
            loop {
                let remaining = deadline.saturating_duration_since(Instant::now());
                let Ok(message) = self.messages.recv_timeout(remaining) else {
                    panic!("timeout waiting for {needle:?} diagnostics on {uri}");
                };
                if message["method"] != "textDocument/publishDiagnostics"
                    || message["params"]["uri"].as_str() != Some(uri)
                {
                    continue;
                }
                let items: Vec<Value> = message["params"]["diagnostics"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                if items.iter().any(|item| {
                    item["message"]
                        .as_str()
                        .is_some_and(|message| message.contains(needle))
                }) {
                    return items;
                }
            }
        }
    }

    fn copy_tree(from: &std::path::Path, to: &std::path::Path) {
        std::fs::create_dir_all(to).expect("create fixture directory");
        for entry in std::fs::read_dir(from).expect("read fixture directory") {
            let entry = entry.expect("fixture entry");
            let target = to.join(entry.file_name());
            if entry.file_type().expect("fixture file type").is_dir() {
                copy_tree(&entry.path(), &target);
            } else {
                std::fs::copy(entry.path(), &target).expect("copy fixture file");
            }
        }
    }

    fn position(source: &str, offset: usize) -> Value {
        let prefix = source.get(..offset).expect("identifier offset");
        serde_json::json!({
            "line": prefix.matches('\n').count(),
            "character": prefix.rsplit('\n').next().unwrap_or_default().encode_utf16().count(),
        })
    }

    fn definition(server: &mut Server, uri: &str, source: &str, name: &str) -> Value {
        let offset = source.find(name).expect("query identifier");
        server.request(
            "textDocument/definition",
            serde_json::json!({
                "textDocument": {"uri": uri},
                "position": position(source, offset),
            }),
        )
    }

    let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = std::env::temp_dir().join(format!("goquery-watched-files-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    copy_tree(&base.join("tests/fixtures/watched-files"), &root);

    let child = Command::new(env!("CARGO_BIN_EXE_goquery"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .env("GOWORK", "off")
        .env(
            "GOMODCACHE",
            std::env::temp_dir().join("goquery-watched-modcache"),
        )
        .env_remove("GOQUERY_DISABLE_DIAG")
        .env_remove("GOQUERY_TRACE_QUERIES")
        .spawn()
        .expect("spawn watched-file server");
    let mut server = Server::new(child);

    let root_uri = url::Url::from_directory_path(&root)
        .expect("workspace root URI")
        .to_string();
    let reply = server.request(
        "initialize",
        serde_json::json!({
            "processId": std::process::id(),
            "rootUri": root_uri,
            "workspaceFolders": [{"uri": root_uri, "name": "watched"}],
            "capabilities": {
                "workspace": {
                    "workspaceFolders": true,
                    "didChangeWatchedFiles": {"dynamicRegistration": true},
                },
                "textDocument": {},
            },
            "initializationOptions": {"goroot": ""},
        }),
    );
    assert!(reply.get("result").is_some(), "{reply}");
    server.notify("initialized", serde_json::json!({}));
    let registration = server.wait_registration();
    let watchers = registration["registrations"]
        .as_array()
        .expect("registrations");
    let watcher = watchers
        .iter()
        .find(|registration| registration["method"] == "workspace/didChangeWatchedFiles")
        .expect("watched-file registration");
    let patterns: Vec<&str> = watcher["registerOptions"]["watchers"]
        .as_array()
        .expect("watch patterns")
        .iter()
        .filter_map(|watcher| watcher["globPattern"].as_str())
        .collect();
    for expected in ["**/*.go", "**/go.mod", "**/go.sum", "**/go.work"] {
        assert!(
            patterns.contains(&expected),
            "missing watch pattern {expected}: {patterns:?}"
        );
    }

    // Workspace-folder changes replace the dynamic registration so clients that
    // scope watchers to the folders present at registration include new roots.
    server.notify(
        "workspace/didChangeWorkspaceFolders",
        serde_json::json!({
            "event": {
                "added": [{"uri": root_uri, "name": "watched-again"}],
                "removed": [],
            },
        }),
    );
    let unregistration = server.wait_request("client/unregisterCapability");
    assert_eq!(
        unregistration["params"]["unregisterations"][0]["id"], "goquery-watched-files",
        "{unregistration}"
    );
    let re_registration = server.wait_request("client/registerCapability");
    assert_eq!(
        re_registration["params"]["registrations"][0]["method"], "workspace/didChangeWatchedFiles",
        "{re_registration}"
    );

    let main_path = root.join("main.go");
    let other_path = root.join("other/other.go");
    let lib_path = root.join("lib/lib.go");
    let main = std::fs::read_to_string(&main_path).expect("main source");
    let other = std::fs::read_to_string(&other_path).expect("other source");
    let lib = std::fs::read_to_string(&lib_path).expect("lib source");
    let main_uri = url::Url::from_file_path(&main_path)
        .expect("main URI")
        .to_string();
    let other_uri = url::Url::from_file_path(&other_path)
        .expect("other URI")
        .to_string();
    let lib_uri = url::Url::from_file_path(&lib_path)
        .expect("lib URI")
        .to_string();
    for (uri, source) in [(&main_uri, &main), (&other_uri, &other)] {
        server.notify(
            "textDocument/didOpen",
            serde_json::json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": "go",
                    "version": 1,
                    "text": source,
                },
            }),
        );
    }
    let reply = definition(&mut server, &other_uri, &other, "Value");
    let result = reply["result"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        result.first().and_then(|item| item["uri"].as_str()),
        Some(lib_uri.as_str()),
        "{reply}"
    );

    // A closed file changes on disk without any editor notification for the file
    // itself; only the watched-file batch reports it.
    let renamed = lib.replace(
        "// Value returns the current value.\nfunc Value() int {",
        "// Renamed returns the current value.\nfunc Renamed() int {",
    );
    assert_ne!(renamed, lib, "fixture declaration must change");
    std::fs::write(&lib_path, renamed).expect("write library change");
    server.notify(
        "workspace/didChangeWatchedFiles",
        serde_json::json!({
            "changes": [{"uri": lib_uri, "type": 2}],
        }),
    );
    let diagnostics = server.wait_diagnostics(&[main_uri.as_str(), other_uri.as_str()]);
    for uri in [&main_uri, &other_uri] {
        let items = diagnostics.get(uri).expect("dependent diagnostics");
        assert!(
            items.iter().any(|item| item["message"]
                .as_str()
                .is_some_and(|message| message.contains("undefined: lib.Value"))),
            "{uri}: {items:?}"
        );
    }

    // The refreshed index resolves the renamed declaration for a dependent
    // package's unsaved buffer, including its hover signature.
    let changed_other = other.replace("lib.Value()", "lib.Renamed()");
    assert_ne!(changed_other, other);
    server.notify(
        "textDocument/didChange",
        serde_json::json!({
            "textDocument": {"uri": other_uri, "version": 2},
            "contentChanges": [{"text": changed_other}],
        }),
    );
    let reply = definition(&mut server, &other_uri, &changed_other, "Renamed");
    let result = reply["result"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        result.first().and_then(|item| item["uri"].as_str()),
        Some(lib_uri.as_str()),
        "{reply}"
    );
    assert_eq!(result[0]["range"]["start"]["line"], 3, "{reply}");
    assert_eq!(result[0]["range"]["start"]["character"], 5, "{reply}");
    let reply = server.request(
        "textDocument/hover",
        serde_json::json!({
            "textDocument": {"uri": other_uri},
            "position": position(&changed_other, changed_other.find("Renamed").expect("hover")),
        }),
    );
    let value = reply["result"]["contents"]["value"]
        .as_str()
        .unwrap_or_default();
    assert!(value.contains("func lib.Renamed() int"), "{reply}");

    // Created files join the index on their watched event.
    let extra_path = root.join("lib/extra.go");
    std::fs::write(
        &extra_path,
        "package lib\n\nfunc Extra() int {\n\treturn 3\n}\n",
    )
    .expect("write created file");
    let extra_uri = url::Url::from_file_path(&extra_path)
        .expect("extra URI")
        .to_string();
    server.notify(
        "workspace/didChangeWatchedFiles",
        serde_json::json!({
            "changes": [{"uri": extra_uri, "type": 1}],
        }),
    );
    let changed_main = main.replace("lib.Value()", "lib.Extra()");
    assert_ne!(changed_main, main);
    server.notify(
        "textDocument/didChange",
        serde_json::json!({
            "textDocument": {"uri": main_uri, "version": 2},
            "contentChanges": [{"text": changed_main}],
        }),
    );
    let reply = definition(&mut server, &main_uri, &changed_main, "Extra");
    let result = reply["result"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        result.first().and_then(|item| item["uri"].as_str()),
        Some(extra_uri.as_str()),
        "{reply}"
    );

    // Module manifests are planned from disk; a saved/watched go.mod change
    // refreshes the plan and the renamed module still resolves after the
    // dependent import is updated.
    let mod_path = root.join("go.mod");
    let mod_uri = url::Url::from_file_path(&mod_path)
        .expect("module URI")
        .to_string();
    let renamed_module = "module example.test/renamed\n\ngo 1.27.3\n";
    std::fs::write(&mod_path, renamed_module).expect("write module change");
    server.notify(
        "workspace/didChangeWatchedFiles",
        serde_json::json!({
            "changes": [{"uri": mod_uri, "type": 2}],
        }),
    );
    let renamed_import =
        changed_other.replace("example.test/watched/lib", "example.test/renamed/lib");
    assert_ne!(renamed_import, changed_other);
    server.notify(
        "textDocument/didChange",
        serde_json::json!({
            "textDocument": {"uri": other_uri, "version": 3},
            "contentChanges": [{"text": renamed_import}],
        }),
    );
    let reply = definition(&mut server, &other_uri, &renamed_import, "Renamed");
    let result = reply["result"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        result.first().and_then(|item| item["uri"].as_str()),
        Some(lib_uri.as_str()),
        "{reply}"
    );

    // Re-watching an unchanged main manifest must not rebuild the index: the
    // plan digest is unchanged and package metadata stays valid.
    server.notify(
        "workspace/didChangeWatchedFiles",
        serde_json::json!({
            "changes": [{"uri": mod_uri, "type": 2}],
        }),
    );
    let reply = definition(&mut server, &other_uri, &renamed_import, "Renamed");
    let result = reply["result"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        result.first().and_then(|item| item["uri"].as_str()),
        Some(lib_uri.as_str()),
        "{reply}"
    );

    // A save (without any didOpen for that file) reindexes the on-disk content,
    // so a new declaration is available to already-open consumers.
    let saved_lib = "package lib\n\n// Renamed returns the current value.\nfunc Renamed() string { return \"\" }\n\nfunc Saved() int { return 4 }\n";
    std::fs::write(&lib_path, saved_lib).expect("write saved library change");
    server.notify(
        "textDocument/didSave",
        serde_json::json!({"textDocument": {"uri": lib_uri}}),
    );
    let saved_use = renamed_import.replace(
        "func Use() int {",
        "func SavedUse() int {\n\treturn lib.Saved()\n}\n\nfunc Use() int {",
    );
    assert_ne!(saved_use, renamed_import);
    server.notify(
        "textDocument/didChange",
        serde_json::json!({
            "textDocument": {"uri": other_uri, "version": 4},
            "contentChanges": [{"text": saved_use}],
        }),
    );
    let saved_offset = saved_use.find("lib.Saved").expect("saved use") + "lib.".len();
    let reply = server.request(
        "textDocument/definition",
        serde_json::json!({
            "textDocument": {"uri": other_uri},
            "position": position(&saved_use, saved_offset),
        }),
    );
    let result = reply["result"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        result.first().and_then(|item| item["uri"].as_str()),
        Some(lib_uri.as_str()),
        "{reply}"
    );

    // A save that removes a declaration republishes diagnostics for open
    // importers, not just the edited file.
    let removed_lib = "package lib\n\nfunc Renamed() string { return \"\" }\n";
    std::fs::write(&lib_path, removed_lib).expect("write removal");
    server.notify(
        "textDocument/didSave",
        serde_json::json!({"textDocument": {"uri": lib_uri}}),
    );
    let diagnostics = server.wait_diagnostics(&[other_uri.as_str()]);
    let items = diagnostics.get(&other_uri).expect("importer diagnostics");
    assert!(
        items.iter().any(|item| item["message"]
            .as_str()
            .is_some_and(|message| message.contains("undefined: lib.Saved"))),
        "{items:?}"
    );

    let reply = server.request("shutdown", Value::Null);
    assert!(reply.get("result").is_some(), "{reply}");
    server.notify("exit", Value::Null);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if server.child.try_wait().expect("wait server").is_some() {
            break;
        }
        assert!(Instant::now() < deadline, "server did not exit");
        std::thread::sleep(Duration::from_millis(10));
    }

    // Direct-disk edits without any editor notification: file watching picks
    // them up and reloads clean open buffers, so downstream semantics and
    // diagnostics refresh without didChange/didSave/didChangeWatchedFiles.
    {
        let file_watch = true;
        let poll_root = std::env::temp_dir().join(format!(
            "goquery-poll-files-{}-{file_watch}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&poll_root);
        copy_tree(&base.join("tests/fixtures/watched-files"), &poll_root);
        let mut polled = Server::new(
            Command::new(env!("CARGO_BIN_EXE_goquery"))
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .env("GOWORK", "off")
                .env(
                    "GOMODCACHE",
                    std::env::temp_dir().join("goquery-poll-modcache"),
                )
                .env_remove("GOQUERY_DISABLE_DIAG")
                .spawn()
                .expect("spawn polling server"),
        );
        let poll_root_uri = url::Url::from_directory_path(&poll_root)
            .expect("polling root URI")
            .to_string();
        let reply = polled.request(
            "initialize",
            serde_json::json!({
                "processId": std::process::id(),
                "rootUri": poll_root_uri,
                "workspaceFolders": [{"uri": poll_root_uri, "name": "polled"}],
                "capabilities": {"textDocument": {}},
                "initializationOptions": {"goroot": "", "fileWatch": file_watch,
                    "fileWatchExcludeDirectories": ["(^|/)generated$", "^cache-[0-9]+$"]},
            }),
        );
        assert!(reply.get("result").is_some(), "{reply}");
        polled.notify("initialized", serde_json::json!({}));
        let poll_main_path = poll_root.join("main.go");
        let poll_other_path = poll_root.join("other/other.go");
        let poll_lib_path = poll_root.join("lib/lib.go");
        let poll_main = std::fs::read_to_string(&poll_main_path).expect("poll main source");
        let poll_other = std::fs::read_to_string(&poll_other_path).expect("poll other source");
        let poll_lib = std::fs::read_to_string(&poll_lib_path).expect("poll lib source");
        let poll_main_uri = url::Url::from_file_path(&poll_main_path)
            .expect("poll main URI")
            .to_string();
        let poll_other_uri = url::Url::from_file_path(&poll_other_path)
            .expect("poll other URI")
            .to_string();
        let poll_lib_uri = url::Url::from_file_path(&poll_lib_path)
            .expect("poll lib URI")
            .to_string();
        for (uri, source) in [
            (&poll_main_uri, &poll_main),
            (&poll_other_uri, &poll_other),
            (&poll_lib_uri, &poll_lib),
        ] {
            polled.notify(
                "textDocument/didOpen",
                serde_json::json!({
                    "textDocument": {
                        "uri": uri,
                        "languageId": "go",
                        "version": 1,
                        "text": source,
                    },
                }),
            );
        }
        let poll_barrier = polled.request(
            "textDocument/documentSymbol",
            serde_json::json!({"textDocument": {"uri": poll_lib_uri}}),
        );
        assert!(poll_barrier.get("result").is_some(), "{poll_barrier}");

        let poll_renamed = poll_lib.replace(
            "// Value returns the current value.\nfunc Value() int {",
            "// Renamed returns the current value.\nfunc Renamed() int {",
        );
        std::fs::write(&poll_lib_path, &poll_renamed).expect("direct library write");
        let items = polled.wait_diagnostic_containing(&poll_other_uri, "undefined: lib.Value");
        assert!(
            items.iter().any(|item| item["message"]
                .as_str()
                .is_some_and(|message| message.contains("undefined: lib.Value"))),
            "{items:?}"
        );

        let poll_changed_other = poll_other.replace("lib.Value()", "lib.Renamed()");
        polled.notify(
            "textDocument/didChange",
            serde_json::json!({
                "textDocument": {"uri": poll_other_uri, "version": 2},
                "contentChanges": [{"text": poll_changed_other}],
            }),
        );
        let offset = poll_changed_other.find("Renamed").expect("poll rename use");
        let reply = polled.request(
            "textDocument/definition",
            serde_json::json!({
                "textDocument": {"uri": poll_other_uri},
                "position": position(&poll_changed_other, offset),
            }),
        );
        let result = reply["result"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            result.first().and_then(|item| item["uri"].as_str()),
            Some(poll_lib_uri.as_str()),
            "{reply}"
        );
        let hover_offset = poll_renamed
            .find("func Renamed")
            .expect("poll renamed declaration")
            + "func ".len();
        let reply = polled.request(
            "textDocument/hover",
            serde_json::json!({
                "textDocument": {"uri": poll_lib_uri},
                "position": position(&poll_renamed, hover_offset),
            }),
        );
        let value = reply["result"]["contents"]["value"]
            .as_str()
            .unwrap_or_default();
        assert!(value.contains("func Renamed() int"), "{reply}");

        // New files created directly on disk are discovered by the structural walk.
        let poll_extra_path = poll_root.join("lib/extra.go");
        std::fs::write(
            &poll_extra_path,
            "package lib\n\nfunc Extra() int {\n\treturn 4\n}\n",
        )
        .expect("write direct new file");
        std::thread::sleep(Duration::from_millis(500));
        let poll_extra_uri = url::Url::from_file_path(&poll_extra_path)
            .expect("poll extra URI")
            .to_string();
        let poll_changed_main = poll_main.replace("lib.Value()", "lib.Extra()");
        polled.notify(
            "textDocument/didChange",
            serde_json::json!({
                "textDocument": {"uri": poll_main_uri, "version": 2},
                "contentChanges": [{"text": poll_changed_main}],
            }),
        );
        let extra_offset = poll_changed_main.find("Extra").expect("poll extra use");
        let reply = polled.request(
            "textDocument/definition",
            serde_json::json!({
                "textDocument": {"uri": poll_main_uri},
                "position": position(&poll_changed_main, extra_offset),
            }),
        );
        let result = reply["result"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            result.first().and_then(|item| item["uri"].as_str()),
            Some(poll_extra_uri.as_str()),
            "{reply}"
        );
        // Deleted indexed files no longer occur in the directory walk. The poller
        // must still remove their declarations through its missing-path check.
        let reply = polled.request("workspace/symbol", serde_json::json!({"query": "Extra"}));
        assert!(
            reply["result"].as_array().is_some_and(|items| {
                items
                    .iter()
                    .any(|item| item["location"]["uri"] == poll_extra_uri)
            }),
            "new symbol not indexed: {reply}"
        );
        std::fs::remove_file(&poll_extra_path).expect("delete direct new file");
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let reply = polled.request("workspace/symbol", serde_json::json!({"query": "Extra"}));
            if reply.get("result").is_some_and(|result| {
                result.is_null()
                    || result.as_array().is_some_and(|items| {
                        items
                            .iter()
                            .all(|item| item["location"]["uri"] != poll_extra_uri)
                    })
            }) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "deleted symbol still indexed: {reply}"
            );
            std::thread::sleep(Duration::from_millis(25));
        }
        // Fresh listings must discover new directories and directory renames too.
        let old_directory = poll_root.join("direct/nested");
        let new_directory = poll_root.join("direct/moved");
        std::fs::create_dir_all(&old_directory).expect("create polled directory");
        std::fs::write(
            old_directory.join("leaf.go"),
            "package leaf\nfunc PollDirectoryLeaf() {}\n",
        )
        .expect("write polled directory source");
        let old_uri = url::Url::from_file_path(old_directory.join("leaf.go"))
            .expect("old leaf URI")
            .to_string();
        for renamed in [false, true] {
            if renamed {
                std::fs::rename(&old_directory, &new_directory).expect("rename polled directory");
            }
            let directory = if renamed {
                &new_directory
            } else {
                &old_directory
            };
            let uri = url::Url::from_file_path(directory.join("leaf.go"))
                .expect("leaf URI")
                .to_string();
            // A directory event must register and scan the newly created subtree.
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                let reply = polled.request(
                    "workspace/symbol",
                    serde_json::json!({"query": "PollDirectoryLeaf"}),
                );
                if reply["result"].as_array().is_some_and(|items| {
                    items.iter().any(|item| item["location"]["uri"] == uri)
                        && (!renamed || items.iter().all(|item| item["location"]["uri"] != old_uri))
                }) {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "directory change not indexed: {reply}"
                );
                std::thread::sleep(Duration::from_millis(25));
            }
        }
        let appeared_path = poll_root.join("direct/appeared.go");
        std::fs::write(&appeared_path, "package direct\nfunc PollAppeared() {}\n")
            .expect("add Go source to existing directory");
        let appeared_uri = url::Url::from_file_path(&appeared_path)
            .expect("appeared URI")
            .to_string();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let reply = polled.request(
                "workspace/symbol",
                serde_json::json!({"query": "PollAppeared"}),
            );
            if reply["result"].as_array().is_some_and(|items| {
                items
                    .iter()
                    .any(|item| item["location"]["uri"] == appeared_uri)
            }) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "new Go source not indexed: {reply}"
            );
            std::thread::sleep(Duration::from_millis(25));
        }
        // Regexes match directory prefixes, including nested paths, but not a
        // similarly named file or a nonmatching sibling. A regular event must
        // not pull excluded new files into reconciliation either.
        let excluded_generated = poll_root.join("direct/generated/deep/skip.go");
        let excluded_cache = poll_root.join("cache-42/skip.go");
        for (path, name) in [
            (&excluded_generated, "ExcludedGenerated"),
            (&excluded_cache, "ExcludedCache"),
        ] {
            std::fs::create_dir_all(path.parent().unwrap()).expect("create excluded directory");
            std::fs::write(path, format!("package skipped\nfunc {name}() {{}}\n"))
                .expect("write excluded source");
        }
        std::fs::create_dir_all(poll_root.join("cache-alpha")).expect("create included directory");
        std::fs::write(
            poll_root.join("cache-alpha/keep.go"),
            "package keep\nfunc IncludedAlpha() {}\n",
        )
        .expect("write included source");
        std::fs::write(
            poll_root.join("generated.go"),
            "package main\nfunc IncludedFilename() {}\n",
        )
        .expect("write similarly named file");
        std::fs::write(
            &appeared_path,
            "package direct\nfunc PollAppearedAfterExcludes() {}\n",
        )
        .expect("trigger regular event");
        for name in [
            "IncludedAlpha",
            "IncludedFilename",
            "PollAppearedAfterExcludes",
        ] {
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                let reply = polled.request("workspace/symbol", serde_json::json!({"query": name}));
                if reply["result"]
                    .as_array()
                    .is_some_and(|items| items.iter().any(|item| item["name"] == name))
                {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "included path did not refresh: {reply}"
                );
                std::thread::sleep(Duration::from_millis(25));
            }
        }
        for name in ["ExcludedGenerated", "ExcludedCache"] {
            let reply = polled.request("workspace/symbol", serde_json::json!({"query": name}));
            assert!(
                !reply["result"]
                    .as_array()
                    .is_some_and(|items| items.iter().any(|item| item["name"] == name)),
                "excluded new source was indexed: {reply}"
            );
        }
        let excluded_uri = url::Url::from_file_path(&excluded_generated)
            .expect("excluded URI")
            .to_string();
        polled.notify("textDocument/didOpen", serde_json::json!({"textDocument": {"uri": excluded_uri, "languageId": "go", "version": 1, "text": "package skipped\nfunc EditorExcluded() {}\n"}}));
        let reply = polled.request(
            "textDocument/documentSymbol",
            serde_json::json!({"textDocument": {"uri": excluded_uri}}),
        );
        assert!(
            reply["result"]
                .as_array()
                .is_some_and(|items| items.iter().any(|item| item["name"] == "EditorExcluded")),
            "editor buffer was excluded: {reply}"
        );

        let reply = polled.request("shutdown", Value::Null);
        assert!(reply.get("result").is_some(), "{reply}");
        polled.notify("exit", Value::Null);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if polled
                .child
                .try_wait()
                .expect("wait polling server")
                .is_some()
            {
                break;
            }
            assert!(Instant::now() < deadline, "polling server did not exit");
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = std::fs::remove_dir_all(&poll_root);
    }

    // An opened file without imports still needs the imports of admitted
    // siblings in an unselected nested module, including promoted fields.
    let nested_root =
        std::env::temp_dir().join(format!("goquery-nested-sibling-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&nested_root);
    std::fs::create_dir_all(nested_root.join("child/p")).unwrap();
    std::fs::create_dir_all(nested_root.join("child/dep")).unwrap();
    std::fs::create_dir_all(nested_root.join("client")).unwrap();
    std::fs::create_dir_all(nested_root.join("dotclient")).unwrap();
    for (path, source) in [
        ("go.mod", "module example.test/outer\n\ngo 1.27.3\n"),
        (
            "view.go",
            "package outer\ntype ViewItem struct{}\nfunc (ViewItem) Prod() {}\ntype ViewWrapper struct {\n\tViewItem\n}\ntype ViewFields struct {\n\tField int\n}\n// Links [ViewFields.Field] and [ViewWrapper.Prod], but [ViewFields.Missing], [ViewItem.TestOnly], [TestViewOnly.Field] and [TestViewGlobal] are unavailable here.\nvar ProductionView ViewItem\nvar ProductionPromoted ViewWrapper\n",
        ),
        (
            "view_test.go",
            "package outer\nfunc (ViewItem) TestOnly() {}\nfunc (ViewItem) testPrivate() {}\ntype TestViewOnly struct { Field int }\nvar TestViewGlobal int\n// Links [ViewItem.TestOnly], [TestViewOnly.Field] and [TestViewGlobal].\nvar InternalView ViewItem\nvar InternalPromoted ViewWrapper\n",
        ),
        (
            "external_view_test.go",
            "package outer_test\nimport \"example.test/outer\"\n// Links [outer.ViewItem.TestOnly], [outer.TestViewOnly.Field] and [outer.TestViewGlobal].\nvar ExternalView outer.ViewItem\nvar ExternalPromoted outer.ViewWrapper\n",
        ),
        (
            "client/view.go",
            "package client\nimport \"example.test/outer\"\n// Links [outer.ViewFields.Field] and [outer.ViewWrapper.Prod], but [outer.ViewItem.TestOnly] is unavailable here.\nvar ImportedView outer.ViewItem\nvar ImportedPromoted outer.ViewWrapper\n",
        ),
        (
            "dotclient/view.go",
            "package dotclient\nimport . \"example.test/outer\"\n// Links [ViewFields.Field], [ViewItem.Prod] and [ViewFields].\nvar Imported ViewFields\nvar ImportedView ViewItem\nvar ImportedPromoted ViewWrapper\n",
        ),
        (
            "generic.go",
            concat!(
                "package outer\nfunc Wrap[A, B any](f func() (A, B)) func() (A, B) { return f }\nvar Wrapped = Wrap(func() (int, string) { return 0, \"\" })\nvar Explicit = Wrap[int, string](func() (int, string) { return 0, \"\" })\n",
                "type T struct{}\nfunc Identity[T any](value T) T { return value }\n",
                "func Pair[A, B any](a A, b B) (first A, second B) { return a, b }\n",
                "func Many[T any](values ...T) []T { return values }\n",
                "func Shape[T any](value T) struct { T T `label:\"T\"` } { return struct { T T `label:\"T\"` }{value} }\n",
            ),
        ),
        ("child/go.mod", "module example.test/child\n\ngo 1.27.3\n"),
        (
            "child/p/types.go",
            "package p\nimport \"example.test/child/dep\"\ntype Carrier struct { dep.Inner }\ntype CarrierAlias = Carrier\n",
        ),
        (
            "child/dep/types.go",
            "package dep\ntype hidden struct { Inner int; Exported int }\nfunc (hidden) Hello() {}\ntype Inner struct { hidden }\ntype ArrayIndex int\nconst LastIndex ArrayIndex = 5\n",
        ),
        (
            "child/p/open.go",
            "package p\nvar Item Carrier\nvar Other CarrierAlias\n",
        ),
    ] {
        std::fs::write(nested_root.join(path), source).unwrap();
    }
    let mut nested_server = Server::new(
        Command::new(env!("CARGO_BIN_EXE_goquery"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .env("GOWORK", "off")
            .env("GOMODCACHE", nested_root.join("modcache"))
            .spawn()
            .unwrap(),
    );
    let reply = nested_server.request(
        "initialize",
        serde_json::json!({
            "rootUri": url::Url::from_directory_path(&nested_root).unwrap(),
            "capabilities": {"textDocument": {"semanticTokens": {
                "requests": {"full": true}, "formats": ["relative"], "tokenModifiers": [],
                "tokenTypes": ["namespace", "type", "function", "method", "variable", "property", "parameter", "keyword", "string", "number", "comment", "operator", "typeParameter"],
            }}}, "initializationOptions": {"goroot": "", "fileWatch": false, "semanticTokens": true},
        }),
    );
    assert!(reply.get("result").is_some(), "{reply}");
    let semantic_types =
        reply["result"]["capabilities"]["semanticTokensProvider"]["legend"]["tokenTypes"].clone();
    nested_server.notify("initialized", serde_json::json!({}));
    let generic_path = nested_root.join("generic.go");
    let generic_uri = url::Url::from_file_path(&generic_path).unwrap();
    let generic_source = std::fs::read_to_string(generic_path).unwrap();
    nested_server.notify(
        "textDocument/didOpen",
        serde_json::json!({"textDocument": {
            "uri": generic_uri, "languageId": "go", "version": 1, "text": generic_source,
        }}),
    );
    for name in ["Wrapped", "Explicit"] {
        let reply = nested_server.request("textDocument/hover", serde_json::json!({
            "textDocument": {"uri": generic_uri}, "position": position(&generic_source, generic_source.find(name).unwrap()),
        }));
        assert!(
            reply["result"]["contents"]["value"]
                .as_str()
                .is_some_and(|text| text.contains(&format!("var {name} func() (int, string)"))),
            "generic callable inference failed: {reply}"
        );
    }
    let formal_use = generic_source.find("value T) T").unwrap() + "value T) ".len();
    let reply = nested_server.request(
        "textDocument/hover",
        serde_json::json!({
            "textDocument": {"uri": generic_uri}, "position": position(&generic_source, formal_use),
        }),
    );
    assert!(
        reply["result"]["contents"]["value"]
            .as_str()
            .is_some_and(|text| text.contains("type parameter T any")),
        "return formal resolved to the package-level T: {reply}"
    );
    let calls = concat!(
        "package client\nimport \"example.test/outer\"\nfunc genericCalls() {\n",
        "_ = outer.Identity[int](1)\n_ = outer.Identity(1)\n",
        "_, _ = outer.Pair[int](1, \"value\")\n_ = outer.Many([]int{1}...)\n",
        "_ = outer.Shape(1)\n}\n",
    );
    let calls_uri = url::Url::from_file_path(nested_root.join("client/generic.go")).unwrap();
    nested_server.notify(
        "textDocument/didOpen",
        serde_json::json!({"textDocument": {
            "uri": calls_uri, "languageId": "go", "version": 1, "text": calls,
        }}),
    );
    for (needle, expected) in [
        ("Identity[int]", "func outer.Identity(value int) int"),
        ("Identity(1)", "func outer.Identity(value int) int"),
        (
            "Pair[int]",
            "func outer.Pair(a int, b string) (first int, second string)",
        ),
        ("Many(", "func outer.Many(values ...int) []int"),
        (
            "Shape(",
            "func outer.Shape(value int) struct{T int \"label:\\\"T\\\"\"}",
        ),
    ] {
        let reply = nested_server.request("textDocument/hover", serde_json::json!({
            "textDocument": {"uri": calls_uri}, "position": position(calls, calls.find(needle).unwrap()),
        }));
        assert!(
            reply["result"]["contents"]["value"]
                .as_str()
                .is_some_and(|text| text.contains(expected)),
            "instantiated signature lost native types or declaration names: {reply}"
        );
    }
    let format_source = concat!(
        "package client\nfunc emit(prefix, format string, args ...any) {}\n",
        "func misleadingf(format string, value any) {}\ntype Named func(string, ...any)\n",
        "func formats(named Named, direct func(string, ...any), value int) {\n",
        "emit(\"%d prefix\", \"%d emit\", value)\nmisleadingf(\"%d ordinary\", value)\n",
        "named(\"%d named\", value)\ndirect(\"%d direct\", value)\ndirect(\"\\x25d escaped\", value)\ndirect(\"\\u0025d unicode\", value)\n",
        "func(format string, args ...any) {}(\"%d literal\", value)\n",
        "emit(\"prefix\", \"%[2]*.[1]*[3]f dynamic\", value, value, 1.5)\n}\n",
    );
    let format_uri = url::Url::from_file_path(nested_root.join("client/format.go")).unwrap();
    nested_server.notify(
        "textDocument/didOpen",
        serde_json::json!({"textDocument": {
            "uri": format_uri, "languageId": "go", "version": 1, "text": format_source,
        }}),
    );
    let reply = nested_server.request(
        "textDocument/semanticTokens/full",
        serde_json::json!({
            "textDocument": {"uri": format_uri},
        }),
    );
    let data = reply["result"]["data"].as_array().expect("format tokens");
    let mut line = 0;
    let mut character = 0;
    let mut starts = Vec::new();
    for token in data.chunks_exact(5) {
        let delta = token[0].as_u64().unwrap();
        line += delta;
        character = if delta == 0 {
            character + token[1].as_u64().unwrap()
        } else {
            token[1].as_u64().unwrap()
        };
        starts.push(serde_json::json!({"line": line, "character": character}));
    }
    for (literal, split) in [
        ("%d prefix", false),
        ("%d emit", true),
        ("%d ordinary", false),
        ("%d named", false),
        ("%d direct", true),
        ("\\x25d escaped", true),
        ("\\u0025d unicode", true),
        ("%d literal", true),
    ] {
        let at = position(format_source, format_source.find(literal).unwrap());
        assert_eq!(
            starts.contains(&at),
            split,
            "format parameter was guessed for {literal}: {reply}"
        );
    }
    for (literal, width) in [
        ("%d emit", 2),
        ("%d direct", 2),
        ("%d literal", 2),
        ("\\x25d escaped", 5),
        ("\\u0025d unicode", 7),
    ] {
        let start = format_source.find(literal).unwrap();
        let line_end = start + format_source[start..].find('\n').unwrap();
        let argument = start + format_source[start..line_end].rfind("value").unwrap();
        let expected = serde_json::json!([
            {"kind": 3, "range": {"start": position(format_source, start), "end": position(format_source, start + width)}},
            {"kind": 2, "range": {"start": position(format_source, argument), "end": position(format_source, argument + 5)}},
        ]);
        for offset in [start, start + width - 1] {
            let reply = nested_server.request("textDocument/documentHighlight", serde_json::json!({
                "textDocument": {"uri": format_uri}, "position": position(format_source, offset),
            }));
            assert_eq!(
                reply["result"], expected,
                "format cursor lost its argument: {reply}"
            );
        }
        let reply = nested_server.request("textDocument/documentHighlight", serde_json::json!({
            "textDocument": {"uri": format_uri}, "position": position(format_source, start + width),
        }));
        assert!(
            reply["result"].is_null(),
            "format range must exclude its end: {reply}"
        );
    }
    let dynamic = format_source.find("%[2]*.[1]*[3]f dynamic").unwrap();
    let number = format_source[dynamic..].find("1.5").unwrap() + dynamic;
    let width_argument = dynamic + format_source[dynamic..number].rfind("value").unwrap();
    for (offset, directive, length, argument, argument_length) in [
        (dynamic + 2, dynamic + 1, 4, width_argument, 5),
        (number + 3, dynamic + 10, 4, number, 3),
    ] {
        let reply = nested_server.request(
            "textDocument/documentHighlight",
            serde_json::json!({
                "textDocument": {"uri": format_uri}, "position": position(format_source, offset),
            }),
        );
        assert_eq!(
            reply["result"],
            serde_json::json!([
                {"kind": 3, "range": {"start": position(format_source, directive), "end": position(format_source, directive + length)}},
                {"kind": 2, "range": {"start": position(format_source, argument), "end": position(format_source, argument + argument_length)}},
            ]),
            "indexed width or argument end boundary was lost: {reply}"
        );
    }
    let declarations = concat!(
        "package client\ntype InlineData struct{ LongName int; X string }\n",
        "type RawTag struct{ Value string `first\n// keep literal text\n\nlast` }\n",
        "type AliasData = InlineData\nvar _ InlineData\nvar _ RawTag\nvar _ AliasData\n",
        "func localDeclaration() { type LocalRaw struct{ Value string `first\n// keep local text\nlast` }; var _ LocalRaw }\n",
    );
    let declarations_uri =
        url::Url::from_file_path(nested_root.join("client/declarations.go")).unwrap();
    nested_server.notify(
        "textDocument/didOpen",
        serde_json::json!({"textDocument": {
            "uri": declarations_uri, "languageId": "go", "version": 1, "text": declarations,
        }}),
    );
    for (name, expected) in [
        (
            "InlineData",
            "type InlineData struct {\n\tLongName int\n\tX        string\n}",
        ),
        (
            "RawTag",
            "type RawTag struct {\n\tValue string `first\n// keep literal text\n\nlast`\n}",
        ),
        (
            "AliasData",
            "type AliasData = InlineData\n\ntype InlineData struct {\n\tLongName int\n\tX        string\n}",
        ),
        (
            "LocalRaw",
            "type LocalRaw struct {\n\tValue string `first\n// keep local text\nlast`\n}",
        ),
    ] {
        let reply = nested_server.request("textDocument/hover", serde_json::json!({
            "textDocument": {"uri": declarations_uri}, "position": position(declarations, declarations.rfind(name).unwrap()),
        }));
        assert!(
            reply["result"]["contents"]["value"]
                .as_str()
                .is_some_and(|text| text.contains(expected)),
            "declaration formatting changed a name, literal or field layout: {reply}"
        );
    }
    let invalid_generic =
        format!("{generic_source}\nfunc Plain() int {{ return 0 }}\nvar Broken = Plain[int]()\n");
    nested_server.notify("textDocument/didChange", serde_json::json!({
        "textDocument": {"uri": generic_uri, "version": 2}, "contentChanges": [{"text": invalid_generic}],
    }));
    let reply = nested_server.request("textDocument/hover", serde_json::json!({
        "textDocument": {"uri": generic_uri}, "position": position(&invalid_generic, invalid_generic.find("Broken").unwrap()),
    }));
    assert!(
        reply["result"].is_null(),
        "illegal instantiation acquired a fabricated type: {reply}"
    );
    nested_server.notify("textDocument/didChange", serde_json::json!({
        "textDocument": {"uri": generic_uri, "version": 3}, "contentChanges": [{"text": generic_source}],
    }));
    let local_source = format!(
        "{generic_source}\nfunc localTypes() {{\n\ttype sample struct {{\n\t\tCount int\n\t}}\n\tsamples := make([]sample, 2)\n\tfor _, want := range samples {{ _ = want }}\n\t_ = samples\n\ttype tagged struct {{\n\t\tData string `first\n\t\t\tkeep\nlast`\n\t}}\n}}\n"
    );
    nested_server.notify("textDocument/didChange", serde_json::json!({
        "textDocument": {"uri": generic_uri, "version": 4}, "contentChanges": [{"text": local_source}],
    }));
    for (offset, expected) in [
        (
            local_source.find("type sample").unwrap() + 5,
            "type sample struct {\n\tCount int\n}",
        ),
        (
            local_source.find("samples :=").unwrap(),
            "var samples []sample",
        ),
        (local_source.find("want :=").unwrap(), "var want sample"),
        (
            local_source.find("type tagged").unwrap() + 5,
            "`first\n\t\t\tkeep\nlast`",
        ),
    ] {
        let reply = nested_server.request(
            "textDocument/hover",
            serde_json::json!({
                "textDocument": {"uri": generic_uri}, "position": position(&local_source, offset),
            }),
        );
        assert!(
            reply["result"]["contents"]["value"]
                .as_str()
                .is_some_and(|text| text.contains(expected)),
            "local type presentation/identity lost: {reply}"
        );
    }
    let nested_uri = url::Url::from_file_path(nested_root.join("child/p/open.go")).unwrap();
    let nested_source = "package p\nvar Item Carrier\nvar Other CarrierAlias\n";
    nested_server.notify(
        "textDocument/didOpen",
        serde_json::json!({"textDocument": {
            "uri": nested_uri, "languageId": "go", "version": 1, "text": nested_source,
        }}),
    );
    let reply = nested_server.request("textDocument/hover", serde_json::json!({
        "textDocument": {"uri": nested_uri}, "position": position(nested_source, nested_source.find("Carrier").unwrap()),
    }));
    let text = reply["result"]["contents"]["value"]
        .as_str()
        .unwrap_or_default();
    assert!(
        text.contains("Exported") && text.contains("through Inner"),
        "missing sibling import promotion: {reply}"
    );
    assert!(
        text.contains("Hello()"),
        "exported method through private embedding was lost: {reply}"
    );
    let reply = nested_server.request("textDocument/hover", serde_json::json!({
        "textDocument": {"uri": nested_uri}, "position": position(nested_source, nested_source.find("CarrierAlias").unwrap()),
    }));
    assert!(
        reply["result"]["contents"]["value"]
            .as_str()
            .is_some_and(|text| text.contains("Hello()")),
        "alias lost its underlying method set: {reply}"
    );
    assert!(
        !text.lines().any(|line| {
            let mut words = line.split_whitespace();
            words.next() == Some("Inner") && words.next() == Some("int")
        }),
        "direct embedding did not shadow the nested same-name field: {reply}"
    );
    let reply = nested_server.request("workspace/symbol", serde_json::json!({"query": "Inner"}));
    assert!(
        !reply["result"]
            .as_array()
            .is_some_and(|items| items.iter().any(|item| item["name"] == "Inner")),
        "nested view leaked into main symbols: {reply}"
    );
    let relations = "package outer\ntype DataOnly interface { Data() any }\ntype WithError interface { error; DataOnly }\ntype PlainData struct{}\nfunc (PlainData) Data() any { return nil }\ntype ErrorData struct{}\nfunc (*ErrorData) Data() any { return nil }\nfunc (*ErrorData) Error() string { return \"\" }\ntype WrongData struct{}\nfunc (WrongData) Data() any { return nil }\nfunc (WrongData) Error() int { return 0 }\ntype Embedded struct { DataOnly }\ntype PointerEmbedded struct { *PlainData }\ntype GenericField[T any] struct{}\ntype GenericEmbedded struct { GenericField[int] }\nfunc useEmbedded(e Embedded) { _ = e.DataOnly }\n";
    let relations_uri = url::Url::from_file_path(nested_root.join("relations.go")).unwrap();
    nested_server.notify(
        "textDocument/didOpen",
        serde_json::json!({"textDocument": {
            "uri": relations_uri, "languageId": "go", "version": 1, "text": relations,
        }}),
    );
    let reply = nested_server.request("textDocument/implementation", serde_json::json!({
        "textDocument": {"uri": relations_uri}, "position": position(relations, relations.find("WithError").unwrap()),
    }));
    let rows = reply["result"]
        .as_array()
        .unwrap_or_else(|| panic!("implementation result: {reply}"));
    for (name, satisfies) in [
        ("PlainData", false),
        ("ErrorData", true),
        ("WrongData", false),
    ] {
        let required = position(relations, relations.find(name).unwrap());
        assert_eq!(
            rows.iter().any(
                |row| row["uri"] == relations_uri.as_str() && row["range"]["start"] == required
            ),
            satisfies,
            "embedded universe method requirement was lost for {name}: {reply}"
        );
    }
    for (needle, relative, count) in [
        ("struct { DataOnly", "struct { ".len(), 2),
        ("struct { *PlainData", "struct { *".len(), 1),
        ("struct { GenericField[int]", "struct { ".len(), 1),
    ] {
        let reply = nested_server.request("textDocument/documentHighlight", serde_json::json!({
            "textDocument": {"uri": relations_uri}, "position": position(relations, relations.find(needle).unwrap() + relative),
        }));
        let rows = reply["result"].as_array().expect("highlight result");
        assert_eq!(
            rows.len(),
            count,
            "implicit field identity was lost: {reply}"
        );
        assert!(
            rows.iter().all(|row| row["kind"] == 2),
            "embedded field should be read: {reply}"
        );
    }
    for (needle, relative, expected) in [
        ("struct { DataOnly", "struct { ".len(), "PlainData"),
        ("struct { *PlainData", "struct { *".len(), "DataOnly"),
    ] {
        let reply = nested_server.request("textDocument/implementation", serde_json::json!({
            "textDocument": {"uri": relations_uri}, "position": position(relations, relations.find(needle).unwrap() + relative),
        }));
        let expected = position(relations, relations.find(expected).unwrap());
        assert!(
            reply["result"]
                .as_array()
                .is_some_and(|rows| rows.iter().any(|row| {
                    row["uri"] == relations_uri.as_str() && row["range"]["start"] == expected
                })),
            "implementation cursor should follow the embedded type: {reply}"
        );
    }
    let arrays = "package p\nimport \"example.test/child/dep\"\nconst ( Slot = 2 + iota; NextSlot )\nvar Sparse = [...]int{dep.LastIndex: 10, 20, 1: 30}\nvar Grouped = [...]byte{NextSlot: 1}\nvar Empty = [...]int{}\nvar Fraction = [...]int{1.0: 1}\nvar Reverse = [...]int{7: 1, 2: 2, 3}\ntype SameLength [len(Sparse)]byte\nfunc locals() { const top = 5; values := [...]int{top: 1, 2}; _ = values; type item struct { N int }; local := [...]item{{}}; _ = local }\n";
    let arrays_uri = url::Url::from_file_path(nested_root.join("child/p/arrays.go")).unwrap();
    nested_server.notify(
        "textDocument/didOpen",
        serde_json::json!({"textDocument": {
            "uri": arrays_uri, "languageId": "go", "version": 1, "text": arrays,
        }}),
    );
    for (name, expected) in [
        ("Sparse", "var Sparse [7]int"),
        ("Grouped", "var Grouped [4]byte"),
        ("Empty", "var Empty [0]int"),
        ("Fraction", "var Fraction [2]int"),
        ("Reverse", "var Reverse [8]int"),
        ("SameLength", "type SameLength [7]byte"),
        ("values", "var values [7]int"),
        ("local :=", "var local [1]item"),
    ] {
        let reply = nested_server.request("textDocument/hover", serde_json::json!({
            "textDocument": {"uri": arrays_uri}, "position": position(arrays, arrays.find(name).unwrap()),
        }));
        assert!(
            reply["result"]["contents"]["value"]
                .as_str()
                .is_some_and(|text| text.contains(expected)),
            "array extent or lexical identity was lost: {reply}"
        );
    }
    for (version, initializer) in [
        "[...]int{1: 1, 1: 2}".to_owned(),
        "[...]int{2: 1, 2, 3: 3}".to_owned(),
        "[...]int{-1: 1}".to_owned(),
        "[...]int{9223372036854775807: 1}".to_owned(),
        "[...]int{len([]int{}): 1}".to_owned(),
        format!("[...]int{{{}}}", vec!["1"; 257].join(",")),
    ]
    .iter()
    .enumerate()
    {
        let source = format!("{arrays}\nvar Broken = {initializer}\n");
        nested_server.notify("textDocument/didChange", serde_json::json!({
            "textDocument": {"uri": arrays_uri, "version": version + 2}, "contentChanges": [{"text": source}],
        }));
        let reply = nested_server.request("textDocument/hover", serde_json::json!({
            "textDocument": {"uri": arrays_uri}, "position": position(&source, source.find("Broken").unwrap()),
        }));
        assert!(
            reply["result"].is_null(),
            "invalid/unavailable extent fell back to a fabricated type: {reply}"
        );
    }
    for (filename, test_methods, private_methods) in [
        ("view.go", false, false),
        ("view_test.go", true, true),
        ("external_view_test.go", true, false),
        ("client/view.go", false, false),
        ("dotclient/view.go", false, false),
    ] {
        let path = nested_root.join(filename);
        let text = std::fs::read_to_string(&path).unwrap();
        let uri = url::Url::from_file_path(path).unwrap();
        nested_server.notify(
            "textDocument/didOpen",
            serde_json::json!({"textDocument": {
                "uri": uri, "languageId": "go", "version": 1, "text": text,
            }}),
        );
        for name in ["ViewItem", "ViewWrapper"] {
            let reply = nested_server.request("textDocument/hover", serde_json::json!({
                "textDocument": {"uri": uri}, "position": position(&text, text.rfind(name).unwrap()),
            }));
            let content = reply["result"]["contents"]["value"]
                .as_str()
                .unwrap_or_default();
            assert!(
                content.contains("Prod()"),
                "production method missing: {reply}"
            );
            assert_eq!(
                content.contains("TestOnly()"),
                test_methods,
                "wrong test view for {filename}/{name}: {reply}"
            );
            assert_eq!(
                content.contains("testPrivate()"),
                private_methods,
                "wrong private visibility for {filename}/{name}: {reply}"
            );
        }
    }
    for (filename, needle, kind) in [
        ("view.go", "ViewFields.Field", Some("property")),
        ("view.go", "ViewWrapper.Prod", Some("method")),
        ("view.go", "ViewFields.Missing", None),
        ("view.go", "ViewItem.TestOnly", None),
        ("view.go", "TestViewOnly.Field", None),
        ("view.go", "TestViewGlobal", None),
        ("dotclient/view.go", "ViewFields.Field", None),
        ("dotclient/view.go", "ViewItem.Prod", None),
        ("dotclient/view.go", "ViewFields", None),
        ("client/view.go", "outer.ViewFields.Field", Some("property")),
        ("client/view.go", "outer.ViewWrapper.Prod", Some("method")),
        ("client/view.go", "outer.ViewItem.TestOnly", None),
        ("view_test.go", "ViewItem.TestOnly", Some("method")),
        ("view_test.go", "TestViewOnly.Field", Some("property")),
        ("view_test.go", "TestViewGlobal", Some("variable")),
        (
            "external_view_test.go",
            "outer.ViewItem.TestOnly",
            Some("method"),
        ),
        (
            "external_view_test.go",
            "outer.TestViewOnly.Field",
            Some("property"),
        ),
        (
            "external_view_test.go",
            "outer.TestViewGlobal",
            Some("variable"),
        ),
    ] {
        let path = nested_root.join(filename);
        let source = std::fs::read_to_string(&path).unwrap();
        let uri = url::Url::from_file_path(path).unwrap();
        let reply = nested_server.request(
            "textDocument/semanticTokens/full",
            serde_json::json!({
                "textDocument": {"uri": uri},
            }),
        );
        let data = reply["result"]["data"]
            .as_array()
            .unwrap_or_else(|| panic!("missing semantic tokens: {reply}"));
        let reference = format!("[{needle}]");
        let offset =
            source.find(&reference).unwrap() + 1 + needle.rfind('.').map_or(0, |offset| offset + 1);
        let expected = position(&source, offset);
        let mut line = 0;
        let mut character = 0;
        let mut found = None;
        for token in data.chunks_exact(5) {
            let delta_line = token[0].as_u64().unwrap();
            line += delta_line;
            character = if delta_line == 0 {
                character + token[1].as_u64().unwrap()
            } else {
                token[1].as_u64().unwrap()
            };
            if expected == serde_json::json!({"line": line, "character": character}) {
                found = semantic_types[token[3].as_u64().unwrap() as usize].as_str();
            }
        }
        assert_eq!(
            found, kind,
            "wrong doc member token for {filename}/{needle}: {reply}"
        );
    }
    nested_server.request("shutdown", Value::Null);
    nested_server.notify("exit", Value::Null);
    let deadline = Instant::now() + Duration::from_secs(5);
    while nested_server.child.try_wait().unwrap().is_none() {
        assert!(Instant::now() < deadline, "nested server did not exit");
        std::thread::sleep(Duration::from_millis(10));
    }
    let _ = std::fs::remove_dir_all(&nested_root);

    // File watch budgets: an explicit low limit logs an actionable error and
    // never falls back to polling. Raised/unlimited budgets cover both roots.
    let limits_root =
        std::env::temp_dir().join(format!("goquery-watch-limits-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&limits_root);

    // Invalid regex input must fail initialization rather than silently leave
    // the directory watched or exclude everything.
    let mut invalid_exclude = Server::new(
        Command::new(env!("CARGO_BIN_EXE_goquery"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn invalid-exclude server"),
    );
    let reply = invalid_exclude.request("initialize", serde_json::json!({
        "capabilities": {}, "initializationOptions": {"goroot": "", "fileWatchExcludeDirectories": ["["]},
    }));
    assert_eq!(reply["error"]["code"], -32602, "{reply}");
    assert!(
        reply["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("fileWatchExcludeDirectories")),
        "{reply}"
    );
    invalid_exclude.notify("exit", Value::Null);
    let deadline = Instant::now() + Duration::from_secs(5);
    while invalid_exclude
        .child
        .try_wait()
        .expect("wait invalid-exclude server")
        .is_none()
    {
        assert!(
            Instant::now() < deadline,
            "invalid-exclude server did not exit"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let folders: Vec<_> = ["first", "second"].into_iter().map(|name| {
        let directory = limits_root.join(name);
        std::fs::create_dir_all(&directory).expect("create limit module");
        std::fs::write(directory.join("go.mod"), format!("module example.test/{name}\n\ngo 1.27.3\n")).expect("write limit manifest");
        serde_json::json!({"uri": url::Url::from_directory_path(directory).expect("limit root URI"), "name": name})
    }).collect();
    let mut cases = vec![
        (1, 0, 0, Some("fileWatchMaxRoots")),
        (2, 16, 32, None),
        (0, 0, 0, None),
    ];
    if !cfg!(any(windows, target_os = "macos")) {
        cases.extend([
            (0, 1, 0, Some("fileWatchMaxDirectories")),
            (0, 0, 1, Some("fileWatchMaxEntries")),
        ]);
    }
    for (case, (roots, directories, entries, exceeded)) in cases.into_iter().enumerate() {
        for name in ["first", "second"] {
            std::fs::write(
                limits_root.join(name).join("file.go"),
                "package budget\nfunc BeforeBudgetEdit() {}\n",
            )
            .expect("write initial budget source");
        }
        let mut budgeted = Server::new(
            Command::new(env!("CARGO_BIN_EXE_goquery"))
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .env("GOWORK", "off")
                .env("GOQUERY_POLL_MS", "100")
                .env("GOMODCACHE", limits_root.join("modcache"))
                .spawn()
                .expect("spawn watch budget server"),
        );
        let reply = budgeted.request(
            "initialize",
            serde_json::json!({
                "workspaceFolders": folders,
                "capabilities": {},
                "initializationOptions": {
                    "goroot": "", "fileWatch": true,
                    "pollIntervalMs": 100,
                    "fileWatchMaxRoots": roots,
                    "fileWatchMaxDirectories": directories,
                    "fileWatchMaxEntries": entries,
                },
            }),
        );
        assert!(reply.get("result").is_some(), "{reply}");
        budgeted.notify("initialized", serde_json::json!({}));
        // Complete initial reconciliation before testing a fresh event. Legacy
        // polling settings above must not enable periodic or on-demand scans.
        let reply = budgeted.request(
            "workspace/symbol",
            serde_json::json!({"query": "BeforeBudgetEdit"}),
        );
        assert_eq!(reply["result"].as_array().map(Vec::len), Some(2), "{reply}");
        std::thread::sleep(Duration::from_millis(250));
        let symbol = format!("AfterBudgetEdit{case}");
        std::fs::write(
            limits_root.join("second/file.go"),
            format!("package budget\nfunc {symbol}() {{}}\n"),
        )
        .expect("edit budget source");
        if exceeded.is_some() {
            std::thread::sleep(Duration::from_millis(1200));
            let reply = budgeted.request("workspace/symbol", serde_json::json!({"query": symbol}));
            assert!(
                !reply["result"]
                    .as_array()
                    .is_some_and(|items| items.iter().any(|item| item["name"] == symbol)),
                "unexpected polling fallback: {reply}"
            );
        } else {
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                let reply =
                    budgeted.request("workspace/symbol", serde_json::json!({"query": symbol}));
                if reply["result"]
                    .as_array()
                    .is_some_and(|items| items.iter().any(|item| item["name"] == symbol))
                {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "watch budget case {case} did not refresh: {reply}"
                );
                std::thread::sleep(Duration::from_millis(25));
            }
        }
        let reply = budgeted.request("shutdown", Value::Null);
        assert!(reply.get("result").is_some(), "{reply}");
        budgeted.notify("exit", Value::Null);
        let deadline = Instant::now() + Duration::from_secs(5);
        while budgeted
            .child
            .try_wait()
            .expect("wait budget server")
            .is_none()
        {
            assert!(
                Instant::now() < deadline,
                "watch budget server did not exit"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let mut log = String::new();
        std::io::Read::read_to_string(
            &mut budgeted.child.stderr.take().expect("budget stderr"),
            &mut log,
        )
        .expect("read budget log");
        if let Some(key) = exceeded {
            assert!(log.contains(&format!("{key}=1 exceeded")), "{log}");
        } else {
            assert!(!log.contains("file watching unavailable"), "{log}");
        }
    }
    let _ = std::fs::remove_dir_all(&limits_root);

    let _ = std::fs::remove_dir_all(&root);

    // Clients like Zed pipeline didOpen and document queries immediately after
    // `initialized` before replying to `client/registerCapability`. Dynamic
    // registration and folder re-registration must not hold the concurrency
    // slot while waiting on the client response.
    let pipelined_root =
        std::env::temp_dir().join(format!("goquery-pipelined-watch-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&pipelined_root);
    copy_tree(&base.join("tests/fixtures/watched-files"), &pipelined_root);
    let mut pipelined = Server::manual(
        Command::new(env!("CARGO_BIN_EXE_goquery"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .env("GOWORK", "off")
            .env(
                "GOMODCACHE",
                std::env::temp_dir().join("goquery-pipelined-modcache"),
            )
            .env_remove("GOQUERY_DISABLE_DIAG")
            .spawn()
            .expect("spawn pipelined server"),
    );
    let pipelined_root_uri = url::Url::from_directory_path(&pipelined_root)
        .expect("pipelined root URI")
        .to_string();
    let reply = pipelined.request(
        "initialize",
        serde_json::json!({
            "processId": std::process::id(),
            "rootUri": pipelined_root_uri,
            "workspaceFolders": [{"uri": pipelined_root_uri, "name": "pipelined"}],
            "capabilities": {
                "workspace": {
                    "workspaceFolders": true,
                    "didChangeWatchedFiles": {"dynamicRegistration": true},
                },
                "textDocument": {},
            },
            "initializationOptions": {"goroot": "", "fileWatch": false},
        }),
    );
    assert!(reply.get("result").is_some(), "{reply}");
    let pipelined_main_path = pipelined_root.join("main.go");
    let pipelined_main = std::fs::read_to_string(&pipelined_main_path).expect("pipelined main");
    let pipelined_main_uri = url::Url::from_file_path(&pipelined_main_path)
        .expect("pipelined main URI")
        .to_string();
    pipelined.notify("initialized", serde_json::json!({}));
    pipelined.notify(
        "textDocument/didOpen",
        serde_json::json!({
            "textDocument": {
                "uri": pipelined_main_uri,
                "languageId": "go",
                "version": 1,
                "text": pipelined_main,
            },
        }),
    );
    let reply = pipelined.request(
        "textDocument/documentSymbol",
        serde_json::json!({"textDocument": {"uri": pipelined_main_uri}}),
    );
    assert!(reply["result"].is_array(), "{reply}");
    let registration = pipelined.wait_request("client/registerCapability");
    pipelined.reply(&registration);

    pipelined.notify(
        "workspace/didChangeWorkspaceFolders",
        serde_json::json!({
            "event": {
                "added": [{"uri": pipelined_root_uri, "name": "pipelined-again"}],
                "removed": [],
            },
        }),
    );
    let reply = pipelined.request(
        "textDocument/documentSymbol",
        serde_json::json!({"textDocument": {"uri": pipelined_main_uri}}),
    );
    assert!(reply["result"].is_array(), "{reply}");
    let unregistration = pipelined.wait_request("client/unregisterCapability");
    pipelined.reply(&unregistration);
    let re_registration = pipelined.wait_request("client/registerCapability");
    pipelined.reply(&re_registration);

    let reply = pipelined.request("shutdown", Value::Null);
    assert!(reply.get("result").is_some(), "{reply}");
    pipelined.notify("exit", Value::Null);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if pipelined
            .child
            .try_wait()
            .expect("wait pipelined server")
            .is_some()
        {
            break;
        }
        assert!(Instant::now() < deadline, "pipelined server did not exit");
        std::thread::sleep(Duration::from_millis(10));
    }
    let _ = std::fs::remove_dir_all(&pipelined_root);
}

// ---------------------------------------------------------------------------
// AST-shape differential: the native `syntax` view against go/ast on the pinned
// Go source corpus. Appended after the existing harness so no shared helpers are
// modified. See tests/go-corpus/README.md for the comparison contract.
// ---------------------------------------------------------------------------
mod ast_shape_differential {
    use serde_json::Value;
    use std::collections::BTreeMap;
    use std::io::Write;
    use std::path::PathBuf;
    use std::process::{Command, Stdio};

    type ShapeNode = (String, usize, usize);

    /// Mirror of the oracle's detail limit and chunk size.
    const SHAPE_NODE_DETAIL_LIMIT: usize = 8192;
    const SHAPE_CHUNK_NODES: usize = 2048;

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

    /// Compares one oversized component through sorted chunk digests, so every
    /// file is compared without shipping or retaining the full node array.
    fn compare_component_digest(
        section: &str,
        display: &str,
        base: usize,
        component: &Value,
        mut actual: Vec<ShapeNode>,
        stats: &mut Stats,
        mismatches: &mut Mismatches,
    ) {
        // Body/initializer oracle spans are absolute; type components are
        // relative to the parsed type text.
        if section != "type" {
            for node in &mut actual {
                node.1 += base;
                node.2 += base;
            }
        }
        let node_count = component["node_count"].as_u64().expect("node count") as usize;
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

    /// Compares sorted chunk digests against a native canonical node list.
    fn digest_compare(
        section: &str,
        display: &str,
        label: &str,
        node_count: usize,
        expected_chunks: &[Value],
        mut actual: Vec<ShapeNode>,
        mismatches: &mut Mismatches,
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
        for (index, (expected_chunk, actual_chunk)) in
            expected.iter().zip(&actual_chunks).enumerate()
        {
            if expected_chunk == actual_chunk {
                continue;
            }
            let first = &expected_chunks[index]["first"];
            let last = &expected_chunks[index]["last"];
            detail.push_str(&format!(
                "; chunk {index}: expected {expected_chunk:?} ({first:?} .. {last:?}), actual {actual_chunk:?}"
            ));
            let start = index * SHAPE_CHUNK_NODES;
            if let Some(node) = actual.get(start) {
                detail.push_str(&format!(", actual first {node:?}"));
            }
            break;
        }
        mismatches.record(format!("{section}:digest"), detail);
    }

    #[derive(Default)]
    struct Mismatches {
        classes: BTreeMap<String, usize>,
        examples: BTreeMap<String, Vec<String>>,
    }

    impl Mismatches {
        fn record(&mut self, class: String, detail: String) {
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

        fn total(&self) -> usize {
            self.classes.values().sum()
        }

        fn is_empty(&self) -> bool {
            self.classes.is_empty()
        }
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
        use walkdir::WalkDir;
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
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
        let limit = std::env::var("GOQUERY_SHAPE_LIMIT")
            .ok()
            .map(|value| value.parse::<usize>().expect("GOQUERY_SHAPE_LIMIT"));
        let oracle = std::env::temp_dir().join(format!(
            "goquery-shape-oracle-{}{}",
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
            "AST shape oracle requires Go 1.27.3"
        );
        let build = Command::new("go")
            .arg("build")
            .arg("-o")
            .arg(&oracle)
            .arg(base.join("tests/oracle.go"))
            .output()
            .expect("build official oracle");
        assert!(
            build.status.success(),
            "oracle build failed: {}",
            String::from_utf8_lossy(&build.stderr)
        );
        let mut stats = Stats::default();
        let mut mismatches = Mismatches::default();
        let mut checked_packages = 0usize;
        for (package, mut files) in packages {
            let enabled = if let Some(selected) = &selected {
                selected == &package
            } else if scope == "all" {
                true
            } else if scope == "stdlib" {
                !package
                    .split('/')
                    .any(|part| matches!(part, "cmd" | "testdata"))
            } else {
                !package.split('/').any(|part| part == "testdata")
            };
            if !enabled {
                continue;
            }
            if let Some(limit) = limit
                && checked_packages >= limit
            {
                break;
            }
            checked_packages += 1;
            files.sort();
            let mut child = Command::new(&oracle)
                .arg("--shapes")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("launch shape oracle");
            child
                .stdin
                .take()
                .expect("shape oracle stdin")
                .write_all(&serde_json::to_vec(&files).expect("encode filenames"))
                .expect("send filenames");
            let output = child.wait_with_output().expect("wait for shape oracle");
            assert!(
                output.status.success(),
                "shape oracle failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let reports: Vec<Value> =
                serde_json::from_slice(&output.stdout).expect("shape oracle reports");
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
                "scope": scope,
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
        if !mismatches.is_empty() {
            let mut lines = Vec::new();
            for (class, count) in &mismatches.classes {
                lines.push(format!("{class}: {count}"));
                for example in mismatches.examples.get(class).into_iter().flatten() {
                    lines.push(format!("  {example}"));
                }
            }
            panic!(
                "AST shape divergences ({} classes):\n{}",
                mismatches.classes.len(),
                lines.join("\n")
            );
        }
    }

    fn check_declarations(
        display: &str,
        source: &str,
        report: &Value,
        stats: &mut Stats,
        mismatches: &mut Mismatches,
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
        mismatches: &mut Mismatches,
    ) {
        let key = |declaration: &Declaration| {
            (
                declaration.start,
                declaration.name_start,
                declaration.name.clone(),
            )
        };
        let mut expected_by_key = BTreeMap::new();
        for declaration in expected {
            expected_by_key.insert(key(declaration), declaration);
        }
        let mut actual_by_key = BTreeMap::new();
        for declaration in actual {
            actual_by_key.insert(key(declaration), declaration);
        }
        for (key, expected) in &expected_by_key {
            let Some(actual) = actual_by_key.get(key) else {
                mismatches.record(
                    "decl:missing".to_owned(),
                    format!(
                        "{display} {} {} at byte {}",
                        expected.kind, expected.name, key.0
                    ),
                );
                continue;
            };
            if expected.kind != actual.kind {
                mismatches.record(
                    "decl:kind".to_owned(),
                    format!(
                        "{display} {}: {} vs {}",
                        expected.name, expected.kind, actual.kind
                    ),
                );
            }
            if expected.end != actual.end {
                mismatches.record(
                    "decl:end".to_owned(),
                    format!(
                        "{display} {}: end {} vs {}",
                        expected.name, expected.end, actual.end
                    ),
                );
            }
            if expected.name_end != actual.name_end {
                mismatches.record(
                    "decl:name_end".to_owned(),
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
                        class.to_owned(),
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
                    "decl:extra".to_owned(),
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

    fn check_bodies(
        display: &str,
        source: &str,
        report: &Value,
        stats: &mut Stats,
        mismatches: &mut Mismatches,
    ) {
        for component in report["bodies"].as_array().expect("oracle body components") {
            let start = component["start"].as_u64().expect("body start") as usize;
            let node_count = component_node_count(component);
            stats.max_component_nodes = stats.max_component_nodes.max(node_count);
            if component["truncated"].as_bool() == Some(true) {
                note_hard_truncated(stats, "body", display, start);
                continue;
            }
            let end = component["end"].as_u64().expect("body end") as usize;
            let text = source.get(start..end).expect("body source");
            let tree = goquery::syntax::parse_block(text);
            if !tree.diagnostics().is_empty() {
                mismatches.record(
                    "body:diagnostics".to_owned(),
                    format!("{display} byte {start}: {:?}", tree.diagnostics()),
                );
                continue;
            }
            let actual = block_shape(&tree, text);
            if component_has_digests(component) {
                compare_component_digest(
                    "body", display, start, component, actual, stats, mismatches,
                );
            } else {
                let expected = relative_nodes(component, start);
                compare_nodes("body", display, start, &expected, &actual, mismatches);
            }
            stats.bodies += 1;
        }
    }

    fn check_initializers(
        display: &str,
        source: &str,
        report: &Value,
        stats: &mut Stats,
        mismatches: &mut Mismatches,
    ) {
        for component in report["initializers"]
            .as_array()
            .expect("oracle initializer components")
        {
            let start = component["start"].as_u64().expect("initializer start") as usize;
            let node_count = component_node_count(component);
            stats.max_component_nodes = stats.max_component_nodes.max(node_count);
            if component["truncated"].as_bool() == Some(true) {
                note_hard_truncated(stats, "init", display, start);
                continue;
            }
            let end = component["end"].as_u64().expect("initializer end") as usize;
            let text = source.get(start..end).expect("initializer source");
            let tree = goquery::syntax::parse_expression(text);
            if !tree.diagnostics().is_empty() {
                mismatches.record(
                    "init:diagnostics".to_owned(),
                    format!("{display} byte {start}: {:?}", tree.diagnostics()),
                );
                continue;
            }
            let mut nodes = Vec::new();
            expression_shape(tree.nodes(), tree.root().index(), text, &mut nodes);
            if component_has_digests(component) {
                compare_component_digest(
                    "init", display, start, component, nodes, stats, mismatches,
                );
            } else {
                let expected = relative_nodes(component, start);
                compare_nodes("init", display, start, &expected, &nodes, mismatches);
            }
            stats.initializers += 1;
        }
    }

    fn check_types(
        display: &str,
        source: &str,
        report: &Value,
        stats: &mut Stats,
        mismatches: &mut Mismatches,
    ) {
        for component in report["types"].as_array().expect("oracle type components") {
            let start = component["start"].as_u64().expect("type start") as usize;
            let node_count = component_node_count(component);
            stats.max_component_nodes = stats.max_component_nodes.max(node_count);
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
                    "type:diagnostics".to_owned(),
                    format!("{display} byte {start}: {:?}", parsed.diagnostics()),
                );
                continue;
            }
            if parsed.span != (0..parsed_text.len()) {
                mismatches.record(
                    "type:span".to_owned(),
                    format!(
                        "{display} byte {start} method {method}: {:?} != {:?}",
                        parsed.span,
                        0..parsed_text.len()
                    ),
                );
            }
            let actual = type_shape(parsed.type_nodes());
            if component_has_digests(component) {
                compare_component_digest(
                    "type", display, start, component, actual, stats, mismatches,
                );
            } else {
                let expected = component_nodes(component);
                compare_nodes("type", display, start, &expected, &actual, mismatches);
            }
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
        mismatches: &mut Mismatches,
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
                for id in left {
                    child_expression(tree, *id, source, nodes);
                }
                for id in right {
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
                // The clause is folded into its enclosing range statement.
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
                // The parser builds else-if chains iteratively (no control
                // nesting limit per clause); walk the chain with a loop so a long
                // chain cannot exhaust the test thread stack.
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
        // Iterative pre-order. The parser builds left-associative operator and
        // postfix chains iteratively, so arena depth is not bounded by the
        // expression nesting limit; recursion could exhaust the test thread
        // stack on generated concatenation tables.
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
                ExprKind::Literal => push(nodes, "literal".to_owned()),
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

    /// Type-shape nodes of the inline type text at `span`, offset to the
    /// component. Inline types are parsed on demand so arenas do not retain a
    /// shared per-component type-node cap (a large generated initializer can
    /// contain more inline types than any fixed bound).
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

    /// View-level differential: imports, comment grouping, raw strings, folding
    /// groups and folding nodes against go/ast-derived oracle data.
    /// Explicit token budget for the view index in this differential: the pinned
    /// corpus's largest generated file has 616,298 tokens (measured), just above
    /// the server's 600,000-token budget. The server keeps its default; only this
    /// bounded corpus comparison passes a larger budget.
    const VIEW_GATE_TOKEN_BUDGET: usize = 1_000_000;

    fn check_view(
        display: &str,
        source: &str,
        report: &Value,
        stats: &mut Stats,
        mismatches: &mut Mismatches,
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
        mismatches: &mut Mismatches,
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
            section.to_owned(),
            format!(
                "{display}: missing {:?}, extra {:?}",
                &missing[..missing.len().min(3)],
                &extra[..extra.len().min(3)]
            ),
        );
    }

    /// Compares a bounded shape list: full nodes for small lists, sorted chunk
    /// digests above the detail limit. Returns the node count for stats.
    fn compare_shape_list(
        section: &str,
        display: &str,
        list: &Value,
        mut actual: Vec<ShapeNode>,
        mismatches: &mut Mismatches,
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
}

// ---------------------------------------------------------------------------
// Negative/malformed syntax differential: go/parser acceptance and recovery
// offsets against the native validate_file acceptance and diagnostic spans.
// Appended after the existing harness; see tests/go-corpus/README.md for the
// contract, caps and remaining exclusions. Raw error messages are never
// compared and no corpus source text is stored in the report.
// ---------------------------------------------------------------------------
mod negative_syntax_differential {
    use serde_json::Value;
    use std::collections::BTreeMap;
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};

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

    #[derive(Default)]
    struct Mismatches {
        classes: BTreeMap<String, usize>,
        examples: BTreeMap<String, Vec<String>>,
    }

    impl Mismatches {
        fn record(&mut self, class: String, detail: String) {
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

        fn total(&self) -> usize {
            self.classes.values().sum()
        }

        fn is_empty(&self) -> bool {
            self.classes.is_empty()
        }
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

    #[test]
    #[ignore = "prepare the pinned Go source corpus and set GOQUERY_GO_SOURCE"]
    fn go_source_negative_syntax_corpus() {
        use walkdir::WalkDir;
        let base = Path::new(env!("CARGO_MANIFEST_DIR"));
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
        let oracle = std::env::temp_dir().join(format!(
            "goquery-negative-oracle-{}{}",
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
            "negative syntax oracle requires Go 1.27.3"
        );
        let build = Command::new("go")
            .arg("build")
            .arg("-o")
            .arg(&oracle)
            .arg(base.join("tests/oracle.go"))
            .output()
            .expect("build official oracle");
        assert!(
            build.status.success(),
            "oracle build failed: {}",
            String::from_utf8_lossy(&build.stderr)
        );
        let mut stats = Stats::default();
        let mut mismatches = Mismatches::default();
        let mut acceptance: BTreeMap<PathBuf, bool> = BTreeMap::new();
        let mut rejected: Vec<(PathBuf, Value)> = Vec::new();
        for (package, mut files) in packages {
            let enabled = if let Some(selected) = &selected {
                selected == &package
            } else if scope == "stdlib" {
                !package
                    .split('/')
                    .any(|part| matches!(part, "cmd" | "testdata"))
            } else if scope == "toolchain" {
                !package.split('/').any(|part| part == "testdata")
            } else {
                // Default and `all` cover testdata: the rejected corpus lives there.
                true
            };
            if !enabled {
                continue;
            }
            files.sort();
            let mut child = Command::new(&oracle)
                .arg("--negative-files")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("launch negative oracle");
            child
                .stdin
                .take()
                .expect("negative oracle stdin")
                .write_all(&serde_json::to_vec(&files).expect("encode filenames"))
                .expect("send filenames");
            let output = child.wait_with_output().expect("wait for negative oracle");
            assert!(
                output.status.success(),
                "negative oracle failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let reports: Vec<Value> =
                serde_json::from_slice(&output.stdout).expect("negative oracle reports");
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
        if selected.is_none() && (scope.is_empty() || scope == "all") {
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
            let source = match String::from_utf8(bytes) {
                Ok(source) => source,
                Err(_) => {
                    // Native diagnostics operate on &str; invalid-UTF-8 testdata is
                    // an explicit exclusion counted in the report.
                    stats.rejected_skipped += 1;
                    continue;
                }
            };
            let display = file.display().to_string();
            compare_rejected(&display, &source, report, &mut stats, &mut mismatches);
            stats.rejected_compared += 1;
        }

        // (b) Deterministic strided mutations of accepted corpus files.
        let eligible: Vec<PathBuf> = acceptance
            .iter()
            .filter(|(file, accepted)| {
                **accepted
                    && std::fs::metadata(file)
                        .is_ok_and(|meta| meta.len() as usize <= MAX_SEED_BYTES)
            })
            .map(|(file, _)| file.clone())
            .collect();
        let stride = (eligible.len() / MAX_SEED_FILES).max(1);
        let seeds: Vec<&PathBuf> = eligible
            .iter()
            .step_by(stride)
            .take(MAX_SEED_FILES)
            .collect();
        let mut mutation_sources: Vec<String> = Vec::new();
        let mut mutation_labels: Vec<String> = Vec::new();
        let mut mutation_bytes = 0usize;
        'seeds: for file in seeds {
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
                    &mut mutation_sources,
                    &mut mutation_labels,
                    &mut mutation_bytes,
                ) {
                    break 'seeds;
                }
            }
            // Token-level strided mutations: delete/duplicate/delete-pair/truncate.
            let tokens: Vec<std::ops::Range<usize>> = goquery::syntax::Lexer::new(&source)
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
                    0 => format!("{}{}", &source[..span.start], &source[span.end..]),
                    1 => {
                        let mut mutated =
                            String::with_capacity(source.len() + (span.end - span.start));
                        mutated.push_str(&source[..span.end]);
                        mutated.push_str(&source[span.clone()]);
                        mutated.push_str(&source[span.end..]);
                        mutated
                    }
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
                    &mut mutation_sources,
                    &mut mutation_labels,
                    &mut mutation_bytes,
                ) {
                    break 'seeds;
                }
            }
            // Structural and keyword targets: recovery differences concentrate
            // on delimiters, separators and reserved words.
            let structural: Vec<std::ops::Range<usize>> = goquery::syntax::Lexer::new(&source)
                .filter(|token| {
                    !token.inserted
                        && matches!(
                            token.kind,
                            goquery::syntax::TokenKind::LParen
                                | goquery::syntax::TokenKind::RParen
                                | goquery::syntax::TokenKind::LBrack
                                | goquery::syntax::TokenKind::RBrack
                                | goquery::syntax::TokenKind::LBrace
                                | goquery::syntax::TokenKind::RBrace
                                | goquery::syntax::TokenKind::Semicolon
                                | goquery::syntax::TokenKind::Comma
                                | goquery::syntax::TokenKind::Colon
                        )
                })
                .map(|token| token.span)
                .collect();
            for index in 0..3 {
                if structural.len() < 4 {
                    break;
                }
                let target = structural.len() * (index + 1) / 4;
                let span = &structural[target];
                for duplicate in [false, true] {
                    let mutated = if duplicate {
                        let mut mutated =
                            String::with_capacity(source.len() + (span.end - span.start));
                        mutated.push_str(&source[..span.end]);
                        mutated.push_str(&source[span.clone()]);
                        mutated.push_str(&source[span.end..]);
                        mutated
                    } else {
                        format!("{}{}", &source[..span.start], &source[span.end..])
                    };
                    if !push_mutation(
                        &source,
                        mutated,
                        format!(
                            "{display} structural {target} byte {}{}",
                            span.start,
                            if duplicate { " duplicate" } else { "" }
                        ),
                        &mut mutation_sources,
                        &mut mutation_labels,
                        &mut mutation_bytes,
                    ) {
                        break 'seeds;
                    }
                }
            }
            let keywords: Vec<std::ops::Range<usize>> = goquery::syntax::Lexer::new(&source)
                .filter(|token| {
                    !token.inserted
                        && matches!(
                            token.kind,
                            goquery::syntax::TokenKind::Break
                                | goquery::syntax::TokenKind::Case
                                | goquery::syntax::TokenKind::Chan
                                | goquery::syntax::TokenKind::Const
                                | goquery::syntax::TokenKind::Continue
                                | goquery::syntax::TokenKind::Default
                                | goquery::syntax::TokenKind::Defer
                                | goquery::syntax::TokenKind::Else
                                | goquery::syntax::TokenKind::Fallthrough
                                | goquery::syntax::TokenKind::For
                                | goquery::syntax::TokenKind::Func
                                | goquery::syntax::TokenKind::Go
                                | goquery::syntax::TokenKind::Goto
                                | goquery::syntax::TokenKind::If
                                | goquery::syntax::TokenKind::Import
                                | goquery::syntax::TokenKind::Interface
                                | goquery::syntax::TokenKind::Map
                                | goquery::syntax::TokenKind::Package
                                | goquery::syntax::TokenKind::Range
                                | goquery::syntax::TokenKind::Return
                                | goquery::syntax::TokenKind::Select
                                | goquery::syntax::TokenKind::Struct
                                | goquery::syntax::TokenKind::Switch
                                | goquery::syntax::TokenKind::Type
                                | goquery::syntax::TokenKind::Var
                        )
                })
                .map(|token| token.span)
                .collect();
            for index in 0..2 {
                if keywords.len() < 3 {
                    break;
                }
                let target = keywords.len() * (index + 1) / 3;
                let span = &keywords[target];
                for duplicate in [false, true] {
                    let mutated = if duplicate {
                        let mut mutated =
                            String::with_capacity(source.len() + (span.end - span.start));
                        mutated.push_str(&source[..span.end]);
                        mutated.push_str(&source[span.clone()]);
                        mutated.push_str(&source[span.end..]);
                        mutated
                    } else {
                        format!("{}{}", &source[..span.start], &source[span.end..])
                    };
                    if !push_mutation(
                        &source,
                        mutated,
                        format!(
                            "{display} keyword {target} byte {}{}",
                            span.start,
                            if duplicate { " duplicate" } else { "" }
                        ),
                        &mut mutation_sources,
                        &mut mutation_labels,
                        &mut mutation_bytes,
                    ) {
                        break 'seeds;
                    }
                }
            }
            // Line-break targets exercise automatic semicolon insertion.
            let newlines: Vec<usize> = source
                .match_indices('\n')
                .map(|(offset, _)| offset)
                .collect();
            for index in 0..2 {
                if newlines.len() < 3 {
                    break;
                }
                let offset = newlines[newlines.len() * (index + 1) / 3];
                let mut mutated = String::with_capacity(source.len().saturating_sub(1));
                mutated.push_str(&source[..offset]);
                mutated.push_str(&source[offset + 1..]);
                if !push_mutation(
                    &source,
                    mutated,
                    format!("{display} delete-newline {index} byte {offset}"),
                    &mut mutation_sources,
                    &mut mutation_labels,
                    &mut mutation_bytes,
                ) {
                    break 'seeds;
                }
            }
            for index in 0..3 {
                let anchor = source.len() * (index + 1) / 4;
                let Some(offset) = char_boundary_at_or_after(&source, anchor) else {
                    continue;
                };
                let mut mutated = String::with_capacity(source.len() + 1);
                mutated.push_str(&source[..offset]);
                mutated.push('\n');
                mutated.push_str(&source[offset..]);
                if !push_mutation(
                    &source,
                    mutated,
                    format!("{display} insert-newline {index} byte {offset}"),
                    &mut mutation_sources,
                    &mut mutation_labels,
                    &mut mutation_bytes,
                ) {
                    break 'seeds;
                }
            }
        }
        for (chunk_index, chunk) in mutation_sources.chunks(MUTATION_CHUNK).enumerate() {
            let mut child = Command::new(&oracle)
                .arg("--negative-sources")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("launch mutation oracle");
            child
                .stdin
                .take()
                .expect("mutation oracle stdin")
                .write_all(&serde_json::to_vec(chunk).expect("encode mutations"))
                .expect("send mutations");
            let output = child.wait_with_output().expect("wait for mutation oracle");
            assert!(
                output.status.success(),
                "mutation oracle failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let reports: Vec<Value> =
                serde_json::from_slice(&output.stdout).expect("mutation oracle reports");
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
            let mut child = Command::new(&oracle)
                .arg("--negative-sources")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("launch curated oracle");
            child
                .stdin
                .take()
                .expect("curated oracle stdin")
                .write_all(&serde_json::to_vec(chunk).expect("encode curated samples"))
                .expect("send curated samples");
            let output = child.wait_with_output().expect("wait for curated oracle");
            assert!(
                output.status.success(),
                "curated oracle failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let reports: Vec<Value> =
                serde_json::from_slice(&output.stdout).expect("curated oracle reports");
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
        let stress_sources = stress_sources();
        for (chunk_index, chunk) in stress_sources.chunks(MUTATION_CHUNK).enumerate() {
            let mut child = Command::new(&oracle)
                .arg("--negative-sources")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("launch stress oracle");
            child
                .stdin
                .take()
                .expect("stress oracle stdin")
                .write_all(&serde_json::to_vec(chunk).expect("encode stress samples"))
                .expect("send stress samples");
            let output = child.wait_with_output().expect("wait for stress oracle");
            assert!(
                output.status.success(),
                "stress oracle failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let reports: Vec<Value> =
                serde_json::from_slice(&output.stdout).expect("stress oracle reports");
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
        let mut broad_sources: Vec<String> = Vec::new();
        let mut broad_labels: Vec<String> = Vec::new();
        let mut broad_generated = 0usize;
        for (file, accepted) in &acceptance {
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
                "scope": scope,
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
        if !mismatches.is_empty() {
            let mut lines = Vec::new();
            for (class, count) in &mismatches.classes {
                lines.push(format!("{class}: {count}"));
                for example in mismatches.examples.get(class).into_iter().flatten() {
                    lines.push(format!("  {example}"));
                }
            }
            panic!(
                "negative syntax divergences ({} classes):\n{}",
                mismatches.classes.len(),
                lines.join("\n")
            );
        }
    }

    fn compare_curated(
        index: usize,
        source: &str,
        report: &Value,
        stats: &mut Stats,
        mismatches: &mut Mismatches,
    ) {
        let accepted = report["accepted"]
            .as_bool()
            .expect("curated acceptance bool");
        stats.snippets += 1;
        let diagnostics = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            goquery::syntax::validate_file(source)
        }));
        let diagnostics = match diagnostics {
            Ok(diagnostics) => diagnostics,
            Err(_) => {
                mismatches.record(
                    "negative:panic".to_owned(),
                    format!("snippet {index}: validate_file panicked"),
                );
                return;
            }
        };
        let native_accepted = diagnostics.is_empty();
        if accepted != native_accepted {
            let class = if accepted {
                "negative:curated_false_reject"
            } else {
                "negative:curated_false_accept"
            };
            mismatches.record(class.to_owned(), format!("snippet {index} {source:?}"));
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

    /// Bounded pathological inputs for panic-safety and diagnostic-cap checks.
    /// Nesting beyond the parser's documented 128-level bounds is intentionally
    /// rejected by the native parser, so only go-rejected/native-accepted cases
    /// are treated as divergences here.
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
        mismatches: &mut Mismatches,
    ) {
        stats.stress += 1;
        let diagnostics = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            goquery::syntax::validate_file(source)
        }));
        let diagnostics = match diagnostics {
            Ok(diagnostics) => diagnostics,
            Err(_) => {
                mismatches.record(
                    "negative:panic".to_owned(),
                    format!("stress {index}: validate_file panicked"),
                );
                return;
            }
        };
        let accepted = report["accepted"]
            .as_bool()
            .expect("stress acceptance bool");
        let native_accepted = diagnostics.is_empty();
        if !accepted && native_accepted {
            mismatches.record(
                "negative:stress_false_accept".to_owned(),
                format!(
                    "stress {index}: {} bytes fp {:016x}",
                    source.len(),
                    fingerprint(source)
                ),
            );
        } else if accepted && !native_accepted {
            // Documented parser depth/DIAGNOSTIC bounds reject this path
            // intentionally; diagnostics must still be bounded and in-source.
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
        mismatches: &mut Mismatches,
    ) {
        stats.mutations += 1;
        if let Some(accepted) =
            compare_generated("negative", label, source, report, stats, mismatches)
        {
            if accepted {
                stats.mutations_accepted += 1;
            } else {
                stats.mutations_rejected += 1;
            }
        }
    }

    /// Shared mutation/curated-style acceptance and span comparison. Returns the
    /// parser acceptance on parity and `None` when a divergence or panic was
    /// recorded (callers keep their own counters).
    fn compare_generated(
        prefix: &str,
        label: &str,
        source: &str,
        report: &Value,
        stats: &mut Stats,
        mismatches: &mut Mismatches,
    ) -> Option<bool> {
        let accepted = report["accepted"]
            .as_bool()
            .expect("generated acceptance bool");
        let diagnostics = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            goquery::syntax::validate_file(source)
        }));
        let diagnostics = match diagnostics {
            Ok(diagnostics) => diagnostics,
            Err(_) => {
                mismatches.record(
                    format!("{prefix}:panic"),
                    format!("{label}: validate_file panicked"),
                );
                return None;
            }
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

    /// Optional local debugging aid: with GOQUERY_NEGATIVE_DUMP=<dir>, divergent
    /// sources are written (bounded) so they can be replayed or minimized.
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
        let path = directory.join(format!("negative-{index:03}.go"));
        let _ = std::fs::write(path, source);
    }

    fn oracle_sources(oracle: &Path, sources: &[String]) -> Vec<Value> {
        let mut child = Command::new(oracle)
            .arg("--negative-sources")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("launch negative source oracle");
        child
            .stdin
            .take()
            .expect("negative source oracle stdin")
            .write_all(&serde_json::to_vec(sources).expect("encode sources"))
            .expect("send sources");
        let output = child
            .wait_with_output()
            .expect("wait for negative source oracle");
        assert!(
            output.status.success(),
            "negative source oracle failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).expect("negative source oracle reports")
    }

    fn flush_broad_chunk(
        oracle: &Path,
        sources: &mut Vec<String>,
        labels: &mut Vec<String>,
        stats: &mut Stats,
        mismatches: &mut Mismatches,
    ) {
        if sources.is_empty() {
            return;
        }
        let reports = oracle_sources(oracle, sources);
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
        mismatches: &mut Mismatches,
    ) {
        assert_eq!(report["accepted"].as_bool(), Some(false));
        let diagnostics = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            goquery::syntax::validate_file(source)
        }));
        let diagnostics = match diagnostics {
            Ok(diagnostics) => diagnostics,
            Err(_) => {
                mismatches.record(
                    "negative:panic".to_owned(),
                    format!("{display}: validate_file panicked"),
                );
                return;
            }
        };
        if diagnostics.is_empty() {
            mismatches.record(
                "negative:false_accept".to_owned(),
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
        mismatches: &mut Mismatches,
    ) {
        if diagnostics.len() > MAX_DIAGNOSTICS {
            mismatches.record(
                "negative:overlong_diagnostics".to_owned(),
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
                    "negative:invalid_span".to_owned(),
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
            4 => {
                let mut mutated = String::with_capacity(source.len() + 1);
                mutated.push_str(&source[..offset]);
                mutated.push('`');
                mutated.push_str(&source[offset..]);
                mutated
            }
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
}

// ---------------------------------------------------------------------------
// Recovered-structure differential for malformed Go source.
//
// go/parser still recovers top-level declarations and function bodies when a
// file has syntax errors; editors/agents see exactly that partial structure
// while typing. This gate compares go/ast's recovered declaration kind/name/
// order and direct body statement-kind sequences against the native
// `syntax::stub` + `syntax::parse_block` recovery over the 45 go/parser-rejected
// corpus files plus a bounded deterministic token-mutation sample of accepted
// files. Spans are validated for containment; error messages are never
// compared. See tests/go-corpus/README.md for the contract and caps.
// ---------------------------------------------------------------------------
mod recovered_structure_differential {
    use serde_json::Value;
    use std::collections::BTreeMap;
    use std::io::Write;
    use std::ops::Range;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};

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
        informational: BTreeMap<String, usize>,
        informational_examples: BTreeMap<String, Vec<String>>,
    }

    fn record_informational(stats: &mut Stats, class: &str, detail: String) {
        *stats.informational.entry(class.to_owned()).or_default() += 1;
        let examples = stats
            .informational_examples
            .entry(class.to_owned())
            .or_default();
        if examples.len() < 5 {
            let truncated = detail.chars().count() > 240;
            let mut detail: String = detail.chars().take(240).collect();
            if truncated {
                detail.push_str("...");
            }
            examples.push(detail);
        }
    }

    #[derive(Default)]
    struct Mismatches {
        classes: BTreeMap<String, usize>,
        examples: BTreeMap<String, Vec<String>>,
    }

    impl Mismatches {
        fn record(&mut self, class: String, detail: String) {
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

        fn is_empty(&self) -> bool {
            self.classes.is_empty()
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

    /// One source's recovered projections plus the go/parser recovery markers,
    /// shared by the declaration/body comparison helpers.
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
        use walkdir::WalkDir;
        let base = Path::new(env!("CARGO_MANIFEST_DIR"));
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
        let mutation_target = std::env::var("GOQUERY_RECOVERED_LIMIT")
            .ok()
            .map(|value| {
                value
                    .parse::<usize>()
                    .expect("GOQUERY_RECOVERED_LIMIT")
                    .min(MAX_MUTATION_SOURCES)
            })
            .unwrap_or(MUTATION_TARGET);
        let oracle = std::env::temp_dir().join(format!(
            "goquery-recovered-oracle-{}{}",
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
            "recovered-structure oracle requires Go 1.27.3"
        );
        let build = Command::new("go")
            .arg("build")
            .arg("-o")
            .arg(&oracle)
            .arg(base.join("tests/oracle.go"))
            .output()
            .expect("build official oracle");
        assert!(
            build.status.success(),
            "oracle build failed: {}",
            String::from_utf8_lossy(&build.stderr)
        );
        let mut stats = Stats::default();
        let mut mismatches = Mismatches::default();
        let mut acceptance: BTreeMap<PathBuf, bool> = BTreeMap::new();
        let mut rejected: Vec<SourceCase> = Vec::new();
        for (package, mut files) in packages {
            let enabled = if let Some(selected) = &selected {
                selected == &package
            } else if scope == "stdlib" {
                !package
                    .split('/')
                    .any(|part| matches!(part, "cmd" | "testdata"))
            } else if scope == "toolchain" {
                !package.split('/').any(|part| part == "testdata")
            } else {
                // Default and `all` include testdata: the rejected corpus lives there.
                true
            };
            if !enabled {
                continue;
            }
            files.sort();
            let reports = run_oracle_files(&oracle, &files);
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

        // Deterministic mutation sample of accepted, reasonably sized files:
        // sorted-path stride selection, then one token-level mutation each
        // (delete/truncate/duplicate) selected by a fixed arithmetic sequence.
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
                _ => {
                    let mut mutated = String::with_capacity(source.len() + (span.end - span.start));
                    mutated.push_str(&source[..span.end]);
                    mutated.push_str(&source[span.clone()]);
                    mutated.push_str(&source[span.end..]);
                    mutated
                }
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
        if selected.is_none() && (scope.is_empty() || scope == "all") {
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
            let sources: Vec<String> = chunk.iter().map(|case| case.source.clone()).collect();
            let reports = run_oracle_sources(&oracle, &sources);
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
            stats.informational.values().sum::<usize>(),
            informational.join(", ")
        );
        if let Some(path) = std::env::var_os("GOQUERY_RECOVERED_REPORT") {
            let report = serde_json::json!({
                "version": "go1.27.3",
                "scope": scope,
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
                "informational_classes": stats.informational,
                "informational_examples": stats.informational_examples,
                "mismatch_classes": mismatches.classes,
                "examples": mismatches.examples,
            });
            std::fs::write(
                path,
                serde_json::to_vec_pretty(&report).expect("encode recovered report"),
            )
            .expect("write recovered report");
        }
        if !mismatches.is_empty() {
            let mut lines = Vec::new();
            for (class, count) in &mismatches.classes {
                lines.push(format!("{class}: {count}"));
                for example in mismatches.examples.get(class).into_iter().flatten() {
                    lines.push(format!("  {example}"));
                }
            }
            panic!(
                "recovered structure divergences ({} classes):\n{}",
                mismatches.classes.len(),
                lines.join("\n")
            );
        }
    }

    fn run_oracle_files(oracle: &Path, files: &[PathBuf]) -> Vec<Value> {
        let mut child = Command::new(oracle)
            .arg("--negative-files")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("launch recovered file oracle");
        child
            .stdin
            .take()
            .expect("recovered file oracle stdin")
            .write_all(&serde_json::to_vec(files).expect("encode filenames"))
            .expect("send filenames");
        let output = child
            .wait_with_output()
            .expect("wait for recovered file oracle");
        assert!(
            output.status.success(),
            "recovered file oracle failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).expect("recovered file oracle reports")
    }

    fn run_oracle_sources(oracle: &Path, sources: &[String]) -> Vec<Value> {
        let mut child = Command::new(oracle)
            .arg("--recovered-shapes")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("launch recovered source oracle");
        child
            .stdin
            .take()
            .expect("recovered source oracle stdin")
            .write_all(&serde_json::to_vec(sources).expect("encode sources"))
            .expect("send sources");
        let output = child
            .wait_with_output()
            .expect("wait for recovered source oracle");
        assert!(
            output.status.success(),
            "recovered source oracle failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).expect("recovered source oracle reports")
    }

    fn check_case(
        case: &SourceCase,
        report: &Value,
        stats: &mut Stats,
        mismatches: &mut Mismatches,
    ) {
        let label = &case.label;
        let source = &case.source;
        if report["truncated"].as_bool() == Some(true) {
            stats.truncated_sources += 1;
            return;
        }
        if report["bailed"].as_bool() == Some(true) {
            // go/parser gave up after its error limit; no recovered structure is
            // available to compare against (native still recovers, bounded).
            stats.oracle_bailed_sources += 1;
            return;
        }
        let oracle_accepted = report["accepted"]
            .as_bool()
            .expect("recovered acceptance bool");
        let native_acceptance = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            goquery::syntax::validate_file(source)
        }));
        match native_acceptance {
            Ok(diagnostics) => {
                stats.diagnostic_sources += 1;
                if diagnostics.is_empty() != oracle_accepted {
                    mismatches.record(
                        "recovered:acceptance".to_owned(),
                        format!(
                            "{label}: go/parser accepted={oracle_accepted}, native diagnostics={}",
                            diagnostics.len()
                        ),
                    );
                }
            }
            Err(_) => {
                mismatches.record(
                    "recovered:panic".to_owned(),
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
        let native =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| native_declarations(source)));
        let native = match native {
            Ok(Some(declarations)) => declarations,
            Ok(None) => {
                stats.truncated_sources += 1;
                return;
            }
            Err(_) => {
                mismatches.record(
                    "recovered:panic".to_owned(),
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
        if std::env::var("GOQUERY_RECOVERED_DEBUG").is_ok_and(|needle| label.contains(&needle)) {
            eprintln!("RECOVERED_DEBUG {label}\n go/ast: {expected:#?}\n native: {native:#?}");
        }
        let comparison = RecoveredComparison {
            label,
            source,
            oracle_accepted,
            expected_bodies: expected
                .iter()
                .filter_map(|declaration| declaration.body.clone())
                .collect(),
            actual_bodies: native
                .iter()
                .filter_map(|declaration| declaration.body.clone())
                .collect(),
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
        let native_initializers =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| native_initializers(source)));
        match native_initializers {
            Ok(Some(native_initializers)) => {
                if std::env::var("GOQUERY_RECOVERED_DEBUG")
                    .is_ok_and(|needle| label.contains(&needle))
                {
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
            Ok(None) => {
                stats.truncated_sources += 1;
            }
            Err(_) => {
                mismatches.record(
                    "recovered:panic".to_owned(),
                    format!("{label}: initializer recovery panicked"),
                );
            }
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
        let body = if value["body"].as_bool() == Some(true) {
            Some(
                value["body_start"].as_u64().expect("body start") as usize
                    ..value["body_end"].as_u64().expect("body end") as usize,
            )
        } else {
            None
        };
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
            let mut expressions = Vec::new();
            let mut expression_truncated = false;
            let mut budget = MAX_EXPRESSIONS_PER_STATEMENT;
            collect_native_statement_expressions(
                tree,
                source,
                base,
                statement,
                &mut expressions,
                &mut expression_truncated,
                &mut budget,
            );
            if expression_truncated {
                *truncated = true;
                return;
            }
            statements.push(RecoveredStatement {
                kind,
                start: base + statement.span.start,
                end: base + statement.span.end,
                expressions,
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
            // go/parser collapses a go/defer whose operand is not a call to a
            // BadStmt; mirror that so only structurally valid go/defer calls are
            // compared as statement kinds.
            StmtKind::Go { call } => {
                return call_statement_kind(tree, *call, "go");
            }
            StmtKind::Defer { call } => {
                return call_statement_kind(tree, *call, "defer");
            }
            StmtKind::Declaration { keyword, .. } => {
                format!("decl:{}", &source[keyword.clone()])
            }
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

    fn call_statement_kind(
        tree: &goquery::syntax::BlockTree,
        expression: goquery::syntax::ExprId,
        kind: &str,
    ) -> Option<String> {
        is_call_expression(tree, expression).then(|| kind.to_owned())
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

    /// Expression roots for one direct statement, mirroring the oracle slots:
    /// assignment LHS then RHS, return values, conditions/tags/range clauses and
    /// local declaration initializers. Slots are numbered in projection order.
    fn collect_native_statement_expressions(
        tree: &goquery::syntax::BlockTree,
        source: &str,
        base: usize,
        statement: &goquery::syntax::Statement,
        expressions: &mut Vec<RecoveredExpression>,
        truncated: &mut bool,
        budget: &mut usize,
    ) {
        use goquery::syntax::StmtKind;
        let mut slot = 0usize;
        match &statement.kind {
            StmtKind::Expression { expression } => {
                add_native_slot(
                    tree,
                    source,
                    base,
                    *expression,
                    &mut slot,
                    expressions,
                    truncated,
                    budget,
                );
            }
            StmtKind::Assignment { left, right, .. } => {
                for expression in left {
                    add_native_slot(
                        tree,
                        source,
                        base,
                        *expression,
                        &mut slot,
                        expressions,
                        truncated,
                        budget,
                    );
                }
                for expression in right {
                    add_native_slot(
                        tree,
                        source,
                        base,
                        *expression,
                        &mut slot,
                        expressions,
                        truncated,
                        budget,
                    );
                }
            }
            StmtKind::Return { values } => {
                for expression in values {
                    add_native_slot(
                        tree,
                        source,
                        base,
                        *expression,
                        &mut slot,
                        expressions,
                        truncated,
                        budget,
                    );
                }
            }
            StmtKind::Send { channel, value } => {
                add_native_slot(
                    tree,
                    source,
                    base,
                    *channel,
                    &mut slot,
                    expressions,
                    truncated,
                    budget,
                );
                add_native_slot(
                    tree,
                    source,
                    base,
                    *value,
                    &mut slot,
                    expressions,
                    truncated,
                    budget,
                );
            }
            StmtKind::Increment { expression, .. } => {
                add_native_slot(
                    tree,
                    source,
                    base,
                    *expression,
                    &mut slot,
                    expressions,
                    truncated,
                    budget,
                );
            }
            StmtKind::Go { call } | StmtKind::Defer { call } => {
                add_native_slot(
                    tree,
                    source,
                    base,
                    *call,
                    &mut slot,
                    expressions,
                    truncated,
                    budget,
                );
            }
            StmtKind::Declaration { values, .. } => {
                for expression in values {
                    add_native_slot(
                        tree,
                        source,
                        base,
                        *expression,
                        &mut slot,
                        expressions,
                        truncated,
                        budget,
                    );
                }
            }
            StmtKind::If {
                init, condition, ..
            } => {
                add_native_simple_expressions(
                    tree,
                    source,
                    base,
                    *init,
                    &mut slot,
                    expressions,
                    truncated,
                    budget,
                );
                add_native_slot(
                    tree,
                    source,
                    base,
                    *condition,
                    &mut slot,
                    expressions,
                    truncated,
                    budget,
                );
            }
            StmtKind::For {
                init,
                condition,
                post,
                ..
            } => {
                add_native_simple_expressions(
                    tree,
                    source,
                    base,
                    *init,
                    &mut slot,
                    expressions,
                    truncated,
                    budget,
                );
                if let Some(condition) = condition {
                    add_native_slot(
                        tree,
                        source,
                        base,
                        *condition,
                        &mut slot,
                        expressions,
                        truncated,
                        budget,
                    );
                }
                add_native_simple_expressions(
                    tree,
                    source,
                    base,
                    *post,
                    &mut slot,
                    expressions,
                    truncated,
                    budget,
                );
            }
            StmtKind::RangeFor { clause, .. } => {
                let Some(clause) = tree.statement(*clause) else {
                    return;
                };
                let goquery::syntax::StmtKind::RangeClause { left, value, .. } = &clause.kind
                else {
                    return;
                };
                // Oracle order: range expression first, then key/value operands.
                add_native_slot(
                    tree,
                    source,
                    base,
                    *value,
                    &mut slot,
                    expressions,
                    truncated,
                    budget,
                );
                for expression in left {
                    add_native_slot(
                        tree,
                        source,
                        base,
                        *expression,
                        &mut slot,
                        expressions,
                        truncated,
                        budget,
                    );
                }
            }
            StmtKind::Switch { init, tag, .. } => {
                add_native_simple_expressions(
                    tree,
                    source,
                    base,
                    *init,
                    &mut slot,
                    expressions,
                    truncated,
                    budget,
                );
                if let Some(tag) = tag {
                    add_native_slot(
                        tree,
                        source,
                        base,
                        *tag,
                        &mut slot,
                        expressions,
                        truncated,
                        budget,
                    );
                }
            }
            StmtKind::TypeSwitch { init, guard, .. } => {
                add_native_simple_expressions(
                    tree,
                    source,
                    base,
                    *init,
                    &mut slot,
                    expressions,
                    truncated,
                    budget,
                );
                add_native_simple_expressions(
                    tree,
                    source,
                    base,
                    Some(*guard),
                    &mut slot,
                    expressions,
                    truncated,
                    budget,
                );
            }
            _ => {}
        }
    }

    // The projection context (tree/source/base + shared output/caps) is threaded
    // through these test-only helpers; a wrapper struct would obscure the direct
    // correspondence with the oracle slots without adding safety.
    #[allow(clippy::too_many_arguments)]
    fn add_native_simple_expressions(
        tree: &goquery::syntax::BlockTree,
        source: &str,
        base: usize,
        statement: Option<goquery::syntax::StmtId>,
        slot: &mut usize,
        expressions: &mut Vec<RecoveredExpression>,
        truncated: &mut bool,
        budget: &mut usize,
    ) {
        use goquery::syntax::StmtKind;
        let Some(statement) = statement.and_then(|id| tree.statement(id)) else {
            return;
        };
        match &statement.kind {
            StmtKind::Expression { expression } => add_native_slot(
                tree,
                source,
                base,
                *expression,
                slot,
                expressions,
                truncated,
                budget,
            ),
            StmtKind::Assignment { left, right, .. } => {
                for expression in left {
                    add_native_slot(
                        tree,
                        source,
                        base,
                        *expression,
                        slot,
                        expressions,
                        truncated,
                        budget,
                    );
                }
                for expression in right {
                    add_native_slot(
                        tree,
                        source,
                        base,
                        *expression,
                        slot,
                        expressions,
                        truncated,
                        budget,
                    );
                }
            }
            StmtKind::Send { channel, value } => {
                add_native_slot(
                    tree,
                    source,
                    base,
                    *channel,
                    slot,
                    expressions,
                    truncated,
                    budget,
                );
                add_native_slot(
                    tree,
                    source,
                    base,
                    *value,
                    slot,
                    expressions,
                    truncated,
                    budget,
                );
            }
            StmtKind::Increment { expression, .. } => add_native_slot(
                tree,
                source,
                base,
                *expression,
                slot,
                expressions,
                truncated,
                budget,
            ),
            _ => {}
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn add_native_slot(
        tree: &goquery::syntax::BlockTree,
        source: &str,
        base: usize,
        expression: goquery::syntax::ExprId,
        slot: &mut usize,
        expressions: &mut Vec<RecoveredExpression>,
        truncated: &mut bool,
        budget: &mut usize,
    ) {
        add_native_expression_forest(
            tree,
            source,
            base,
            expression,
            *slot,
            0,
            expressions,
            truncated,
            budget,
        );
        *slot += 1;
    }

    #[allow(clippy::too_many_arguments)]
    fn add_native_expression_forest(
        tree: &goquery::syntax::BlockTree,
        source: &str,
        base: usize,
        id: goquery::syntax::ExprId,
        slot: usize,
        depth: usize,
        expressions: &mut Vec<RecoveredExpression>,
        truncated: &mut bool,
        budget: &mut usize,
    ) {
        if *truncated {
            return;
        }
        if *budget == 0 {
            *truncated = true;
            return;
        }
        let Some(expression) = tree.expression(id) else {
            return;
        };
        let Some((kind, name, children)) = native_expression_shape(source, expression) else {
            return;
        };
        expressions.push(RecoveredExpression {
            slot,
            depth,
            kind,
            name,
            start: base + expression.span.start,
            end: base + expression.span.end,
        });
        *budget -= 1;
        for child in children {
            add_native_expression_forest(
                tree,
                source,
                base,
                child,
                slot,
                depth + 1,
                expressions,
                truncated,
                budget,
            );
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
            ExprKind::Literal => node("lit"),
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
                for expression in [*low, *high, *max].into_iter().flatten() {
                    children.push(expression);
                }
                Some(("slice".to_owned(), String::new(), children))
            }
            ExprKind::TypeAssertion { operand, .. } => {
                Some(("typeassert".to_owned(), String::new(), vec![*operand]))
            }
            ExprKind::TypeSwitchGuard { operand } => {
                Some(("typeassert".to_owned(), String::new(), vec![*operand]))
            }
            _ => None,
        }
    }

    /// Top-level `var`/`const` initializer expression lists. The value text after
    /// the spec's depth-zero `=` is parsed as a function body assignment so the
    /// real expression parser (and its recovery) is reused; spans are translated
    /// back to the source.
    fn native_initializers(source: &str) -> Option<Vec<RecoveredInitializer>> {
        use goquery::syntax::SymbolKind;
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
                    goquery::syntax::TokenKind::LParen
                    | goquery::syntax::TokenKind::LBrack
                    | goquery::syntax::TokenKind::LBrace => depth += 1,
                    goquery::syntax::TokenKind::RParen
                    | goquery::syntax::TokenKind::RBrack
                    | goquery::syntax::TokenKind::RBrace => depth = depth.saturating_sub(1),
                    goquery::syntax::TokenKind::Assign if depth == 0 => {
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
            let Some(root) = tree.statement(tree.root()) else {
                continue;
            };
            let goquery::syntax::StmtKind::Block { statements } = &root.kind else {
                continue;
            };
            let Some(assignment) = statements.first().and_then(|id| tree.statement(*id)) else {
                continue;
            };
            let goquery::syntax::StmtKind::Assignment { right, .. } = &assignment.kind else {
                continue;
            };
            if right.is_empty() {
                continue;
            }
            let mut expressions = Vec::new();
            let mut truncated = false;
            for (slot, expression) in right.iter().enumerate() {
                let mut budget = MAX_EXPRESSIONS_PER_STATEMENT;
                add_native_expression_forest(
                    &tree,
                    &wrapper,
                    0,
                    *expression,
                    slot,
                    0,
                    &mut expressions,
                    &mut truncated,
                    &mut budget,
                );
                if truncated {
                    return None;
                }
            }
            let source_base = key.0 + assign.end;
            for expression in &mut expressions {
                expression.start = source_base + expression.start - prefix.len();
                expression.end = source_base + expression.end - prefix.len();
            }
            // The synthetic block wrapper adds a newline and `}` after the real
            // initializer text; recovery can consume those synthetic bytes. Drop
            // or clamp such nodes so wrapper boundaries never leak into spans or
            // structure.
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
                .filter(|expression| expression.depth == 0)
                .map(|expression| expression.start)
                .min()
                .unwrap_or(source_base);
            let end = expressions
                .iter()
                .filter(|expression| expression.depth == 0)
                .map(|expression| expression.end)
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
        mismatches: &mut Mismatches,
    ) {
        let label = comparison.label;
        let expected = comparison.expected;
        let actual = comparison.actual;
        let actual_bodies = &comparison.actual_bodies;
        let mut actual_index = 0usize;
        for expected_declaration in expected {
            let window_end = (actual_index + ALIGNMENT_WINDOW).min(actual.len());
            // Multiple recovered declarations can share a kind and name (`_`);
            // prefer the candidate whose written name span matches, then the
            // nearest name offset, so a single earlier drop does not shift the
            // whole sequence.
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
                    // go/parser recovery can place a declaration inside a region
                    // the native view recovered as one function body (typically
                    // after an unmatched bracket); that is an error-recovery
                    // artifact, recorded separately from a real missing symbol.
                    if contains_offset(actual_bodies, expected_declaration.start) {
                        *stats
                            .informational
                            .entry("recovered:go_nested_declaration".to_owned())
                            .or_default() += 1;
                    } else if actual.iter().any(|declaration| {
                        declaration.start <= expected_declaration.start
                            && expected_declaration.start < declaration.end
                    }) {
                        // An unterminated native group (unbalanced delimiter)
                        // extended a previous declaration over this one; the
                        // region is still covered, but structure is coarser.
                        *stats
                            .informational
                            .entry("recovered:native_swallowed_declaration".to_owned())
                            .or_default() += 1;
                    } else {
                        mismatches.record(
                            "recovered:declaration_missing".to_owned(),
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
        mismatches: &mut Mismatches,
    ) {
        let label = comparison.label;
        let expected = comparison.expected;
        // A true duplicate repeats the same written name span; sharing a kind and
        // name with a different offset (e.g. many `func _`) is not a duplicate.
        if expected.iter().any(|declaration| {
            same_declaration_key(declaration, extra)
                && declaration.name_start == extra.name_start
                && declaration.name_end == extra.name_end
        }) {
            mismatches.record(
                "recovered:declaration_duplicate".to_owned(),
                format!(
                    "{label}: native {} {} at {} duplicates a recovered declaration",
                    extra.kind, extra.name, extra.start
                ),
            );
        } else if contains_offset(&comparison.expected_bodies, extra.start) {
            // The native scanner separated a declaration out of a region go/parser
            // swallowed into a previous recovered body.
            *stats
                .informational
                .entry("recovered:native_split_declaration".to_owned())
                .or_default() += 1;
        } else if contains_offset(comparison.bad_declarations, extra.start) {
            // go/parser collapsed this declaration to BadDecl; native still
            // recovers a written declaration there.
            *stats
                .informational
                .entry("recovered:go_bad_declaration".to_owned())
                .or_default() += 1;
        } else if !comparison.oracle_accepted {
            // go/parser silently dropped a declaration it could not attach to the
            // AST (BadDecl is not emitted in all recovery states); the native token
            // scan still exposes the written declaration.
            *stats
                .informational
                .entry("recovered:go_dropped_declaration".to_owned())
                .or_default() += 1;
        } else {
            mismatches.record(
                "recovered:declaration_extra".to_owned(),
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

    fn same_declaration_key(
        expected: &RecoveredDeclaration,
        actual: &RecoveredDeclaration,
    ) -> bool {
        expected.kind == actual.kind && expected.name == actual.name
    }

    fn check_declaration(
        comparison: &RecoveredComparison<'_>,
        expected: &RecoveredDeclaration,
        actual: &RecoveredDeclaration,
        stats: &mut Stats,
        mismatches: &mut Mismatches,
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
                "recovered:native_span_envelope".to_owned(),
                format!(
                    "{label} {}: declaration {}..{} does not contain name {}..{}",
                    actual.name, actual.start, actual.end, actual.name_start, actual.name_end
                ),
            );
        }
        // Both sides name the same source identifier, so the selection span is
        // exact; the declaration/body extents are recovery boundaries and are
        // only required to be contained and ordered, not equal.
        if expected.name_start != actual.name_start || expected.name_end != actual.name_end {
            mismatches.record(
                "recovered:declaration_name_span".to_owned(),
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
                            "recovered:native_statement_span".to_owned(),
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
                // Oracle `bad` entries are statements go/parser could not
                // structure; they are placeholders, not comparable kinds. Native
                // concrete statements at those spans stand in for them.
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
                    *stats
                        .informational
                        .entry("recovered:oracle_bad_statement".to_owned())
                        .or_default() += 1;
                }
                // Expression structure for paired statements: exact when the
                // statement extents match, containment/kind checks otherwise.
                let comparable = if expected_kinds == actual_kinds {
                    expected_statements.len().min(actual_statements.len())
                } else if is_prefix(&expected_kinds, &actual_kinds)
                    || is_prefix(&actual_kinds, &expected_kinds)
                {
                    expected_kinds.len().min(actual_kinds.len())
                } else {
                    0
                };
                for index in 0..comparable {
                    compare_statement_expressions(
                        label,
                        source,
                        expected_statements[index],
                        actual_statements[index],
                        stats,
                        mismatches,
                    );
                }
                if expected_kinds != actual_kinds {
                    if is_prefix(&expected_kinds, &actual_kinds)
                        || is_prefix(&actual_kinds, &expected_kinds)
                    {
                        // One recovery retained a prefix the other dropped after a
                        // local error: structural agreement, different extent.
                        *stats
                            .informational
                            .entry("recovered:body_prefix".to_owned())
                            .or_default() += 1;
                    } else if expected_body != actual_body {
                        // Recovery boundaries differ, so the statement windows cover
                        // different regions; recorded rather than treated as a
                        // same-scope structural bug.
                        *stats
                            .informational
                            .entry("recovered:body_extent".to_owned())
                            .or_default() += 1;
                    } else {
                        mismatches.record(
                            "recovered:body_statements".to_owned(),
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
            (expected_body, actual_body) => {
                // go/parser can drop a recovered body after a header error while
                // the native view still finds the block; the presence difference is
                // counted, and body-kind comparison only happens when both exist.
                let _ = (expected_body, actual_body);
                *stats
                    .informational
                    .entry("recovered:body_presence".to_owned())
                    .or_default() += 1;
            }
        }
    }

    /// Owned extents for one expression comparison (statement or initializer).
    struct ExpressionContext<'a> {
        label: &'a str,
        source: &'a str,
        actual_extent: Range<usize>,
        extents_equal: bool,
    }

    fn compare_statement_expressions(
        label: &str,
        source: &str,
        expected: &RecoveredStatement,
        actual: &RecoveredStatement,
        stats: &mut Stats,
        mismatches: &mut Mismatches,
    ) {
        let extents_equal = expected.start == actual.start && expected.end == actual.end;
        let expected_slots = group_expression_slots(&expected.expressions);
        let actual_slots = group_expression_slots(&actual.expressions);
        let expected_keys: Vec<usize> = expected_slots.keys().copied().collect();
        let actual_keys: Vec<usize> = actual_slots.keys().copied().collect();
        if expected_keys != actual_keys {
            if extents_equal {
                mismatches.record(
                    "recovered:expression_slots".to_owned(),
                    format!(
                        "{label} {} {}..{}: expression slots {expected_keys:?} vs {actual_keys:?}",
                        expected.kind, expected.start, expected.end
                    ),
                );
            } else {
                *stats
                    .informational
                    .entry("recovered:expression_extent".to_owned())
                    .or_default() += 1;
            }
        }
        let context = ExpressionContext {
            label,
            source,
            actual_extent: actual.start..actual.end,
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
        mismatches: &mut Mismatches,
    ) {
        let paired = expected.len().min(actual.len());
        stats.initializers_compared += paired;
        for index in 0..paired {
            let expected_initializer = &expected[index];
            let actual_initializer = &actual[index];
            let extents_equal = expected_initializer.start == actual_initializer.start
                && expected_initializer.end == actual_initializer.end;
            if !extents_equal {
                *stats
                    .informational
                    .entry("recovered:initializer_extent".to_owned())
                    .or_default() += 1;
            }
            let expected_slots = group_expression_slots(&expected_initializer.expressions);
            let actual_slots = group_expression_slots(&actual_initializer.expressions);
            let expected_keys: Vec<usize> = expected_slots.keys().copied().collect();
            let actual_keys: Vec<usize> = actual_slots.keys().copied().collect();
            if expected_keys != actual_keys {
                if extents_equal {
                    mismatches.record(
                        "recovered:expression_slots".to_owned(),
                        format!(
                            "{label} initializer {index} {}..{}: expression slots {expected_keys:?} vs {actual_keys:?}",
                            expected_initializer.start, expected_initializer.end
                        ),
                    );
                } else {
                    *stats
                        .informational
                        .entry("recovered:initializer_extent".to_owned())
                        .or_default() += 1;
                }
            }
            let context = ExpressionContext {
                label,
                source,
                actual_extent: actual_initializer.start..actual_initializer.end,
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
        if expected.len() != actual.len() {
            *stats
                .informational
                .entry("recovered:initializer_extent".to_owned())
                .or_default() += expected.len().abs_diff(actual.len()).max(1);
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

    /// Compares one expression slot's pre-order nodes. Spans are validated for
    /// containment; exact-extent kind/operator conflicts are hard, while
    /// structure differences with differing recovery extents are counted.
    /// go/parser synthesizes missing identifier/selector text during recovery
    /// (e.g. `_` for `pkg..Name`); only nodes whose text is actually written are
    /// hard comparison material.
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
        mismatches: &mut Mismatches,
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
                    "recovered:expression_span".to_owned(),
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
        // go/parser BadExpr placeholders have no native counterpart; both sides
        // drop them from structure comparison (their spans are still validated).
        let expected_nodes: Vec<&RecoveredExpression> =
            expected.iter().filter(|node| node.kind != "bad").collect();
        let actual_nodes: Vec<&RecoveredExpression> =
            actual.iter().filter(|node| node.kind != "bad").collect();
        let mut seen = std::collections::BTreeSet::new();
        for node in &actual_nodes {
            if !seen.insert((node.start, node.end, node.kind.clone(), node.name.clone())) {
                mismatches.record(
                    "recovered:expression_duplicate".to_owned(),
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
                    "recovered:expression_kind".to_owned(),
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
                    "recovered:expression_structure".to_owned(),
                    format!("{}: one side recovered no expression nodes", context.label),
                );
            } else {
                record_informational(
                    stats,
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
            let expected_depths: Vec<usize> =
                expected_nodes.iter().map(|node| node.depth).collect();
            let actual_depths: Vec<usize> = actual_nodes.iter().map(|node| node.depth).collect();
            if expected_depths != actual_depths {
                if roots_equal && context.extents_equal {
                    // Same node extents/kinds/order but different parent grouping:
                    // go/parser recovery attached them at another depth.
                    *stats
                        .informational
                        .entry("recovered:expression_nesting".to_owned())
                        .or_default() += 1;
                } else {
                    record_informational(
                        stats,
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
                if synthetic_expected > 0 {
                    // go/parser fabricated expression text while recovering; the
                    // native parser recovered a different concrete shape.
                    record_informational(
                        stats,
                        "recovered:go_synthetic_expression",
                        format!(
                            "{}: go/ast {:?} native {:?}",
                            context.label,
                            expression_summary(
                                &expected_nodes
                                    .iter()
                                    .map(|node| (
                                        node.depth,
                                        node.kind.as_str(),
                                        node.name.as_str()
                                    ))
                                    .collect::<Vec<_>>()
                            ),
                            expression_summary(
                                &actual_nodes
                                    .iter()
                                    .map(|node| (
                                        node.depth,
                                        node.kind.as_str(),
                                        node.name.as_str()
                                    ))
                                    .collect::<Vec<_>>()
                            )
                        ),
                    );
                } else {
                    let expected_summary: Vec<(usize, &str, &str)> = expected_nodes
                        .iter()
                        .map(|node| (node.depth, node.kind.as_str(), node.name.as_str()))
                        .collect();
                    let actual_summary: Vec<(usize, &str, &str)> = actual_nodes
                        .iter()
                        .map(|node| (node.depth, node.kind.as_str(), node.name.as_str()))
                        .collect();
                    mismatches.record(
                        "recovered:expression_structure".to_owned(),
                        format!(
                            "{}: go/ast {:?} native {:?}",
                            context.label,
                            expression_summary(&expected_summary),
                            expression_summary(&actual_summary)
                        ),
                    );
                }
            }
        } else {
            record_informational(
                stats,
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
        mismatches: &mut Mismatches,
        origin: &str,
        require_boundaries: bool,
    ) {
        // go/ast recovery can point a zero-width/partial node at a byte inside a
        // multi-byte rune; that is a parser-internal position, so oracle spans
        // only need byte containment. Native spans must be valid UTF-8 boundaries.
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
}
