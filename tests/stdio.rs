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
