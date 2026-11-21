use crate::harness::{
    TestServer, assert_has_location, assert_hover_contains, completion_item,
    decode_semantic_tokens, jsonrpc2, location, locations, pos, range, range_of,
};
use serde_json::{Value, json};

type DiagnosticCase<'a> = (&'a str, &'a [(&'a str, &'a str, &'a str)]);

fn assert_diagnostic_cases(name: &str, cases: &[DiagnosticCase<'_>]) {
    let first = format!("package p\n{}", cases[0].0);
    let server = TestServer::start(
        name,
        &[
            ("go.mod", "module example.com/m\n\ngo 1.27\n"),
            ("main.go", &first),
        ],
    );
    let uri = server.uri("main.go");
    server.did_open(&uri, &first);
    for (index, &(body, expected)) in cases.iter().enumerate() {
        let source = format!("package p\n{body}");
        let version = index as i64 + 1;
        if index != 0 {
            server.did_change(&uri, version, &source);
        }
        let diagnostics = server.published_diagnostics(&uri, version);
        let lines: Vec<_> = source.lines().collect();
        let mut actual: Vec<_> = diagnostics
            .iter()
            .map(|diagnostic| {
                let start = &diagnostic["range"]["start"];
                let end = &diagnostic["range"]["end"];
                assert_eq!(start["line"], end["line"]);
                let line = lines[start["line"].as_u64().unwrap() as usize];
                let start = start["character"].as_u64().unwrap() as usize;
                let end = end["character"].as_u64().unwrap() as usize;
                (
                    diagnostic["code"].as_str().unwrap(),
                    diagnostic["message"].as_str().unwrap(),
                    &line[start..end],
                )
            })
            .collect();
        actual.sort_unstable();
        let mut expected = expected.to_vec();
        expected.sort_unstable();
        assert_eq!(actual, expected, "{source}\n{diagnostics:?}");
    }
    server.shutdown();
}

#[test]
fn cancels_queued_highlight_and_reads_the_next_buffer_version() {
    let server = TestServer::start_session(
        "cancel-highlight",
        &[("go.mod", "module example.com/cancel\n\ngo 1.27.3\n")],
        ".",
        false,
        |_| json!({"goroot": "", "fileWatch": false}),
    );
    let uri = server.uri("main.go");
    let mut original = String::from("package p\nvar X = 1\nvar _ = X\n");
    // Keep didOpen on the worker while the bounded ingress reads the rest
    // of this burst. A single write puts cancellation behind the request.
    original.push_str(&"// padding for the initial full buffer\n".repeat(12_000));
    let updated = "package p\n// moved\nvar X = 2\nvar _ = X\n";
    let burst: Vec<u8> = [
        json!({"jsonrpc": "2.0", "method": "textDocument/didOpen", "params": {
            "textDocument": {"uri": uri, "version": 1, "languageId": "go", "text": original}
        }}),
        json!({"jsonrpc": "2.0", "id": "obsolete", "method": "textDocument/documentHighlight", "params": {
            "textDocument": {"uri": uri}, "position": {"line": 2, "character": 8}
        }}),
        json!({"jsonrpc": "2.0", "method": "$/cancelRequest", "params": {"id": "obsolete"}}),
        json!({"jsonrpc": "2.0", "method": "textDocument/didChange", "params": {
            "textDocument": {"uri": uri, "version": 2}, "contentChanges": [{"text": updated}]
        }}),
        json!({"jsonrpc": "2.0", "id": 20, "method": "textDocument/definition", "params": {
            "textDocument": {"uri": uri}, "position": {"line": 3, "character": 8}
        }}),
    ]
    .into_iter()
    .flat_map(jsonrpc2::frame)
    .collect();
    server.send_raw(&burst);
    let cancelled = server.recv_message();
    assert_eq!(cancelled["id"], "obsolete");
    assert_eq!(cancelled["error"]["code"], -32800);
    let definition = server.recv_message();
    assert_eq!(definition["id"], 20);
    assert_eq!(
        definition["result"],
        json!([{"uri": uri, "range": {
            "start": {"line": 2, "character": 4}, "end": {"line": 2, "character": 5}
        }}])
    );
}

#[test]
fn code_lens_preflight_preserves_escaped_cgo_imports() {
    let source = "package p\nimport \"\\U00000043\"\n";
    let mut server = TestServer::start_session(
        "cgo-import-spelling",
        &[("main.go", source)],
        ".",
        false,
        |_| json!({"goroot": "", "fileWatch": false}),
    );
    let uri = server.uri("main.go");
    server.did_open(&uri, source);
    let reply = server.code_lens(&uri);
    assert_eq!(
        reply["result"],
        json!([{
            "range": {"start": {"line": 1, "character": 7}, "end": {"line": 1, "character": 19}},
            "command": {
                "title": "regenerate cgo definitions",
                "command": "gopls.regenerate_cgo",
                "arguments": [{"URI": uri}, {"source": "codelens"}],
            }
        }])
    );
    server.did_change(&uri, 2, "package p\nvar text = \"\\U00000043\"\n");
    let reply = server.code_lens(&uri);
    assert!(
        reply["result"].is_null(),
        "string literal is not an import: {reply}"
    );
}

#[test]
fn highlights_concrete_methods_across_single_and_multiple_candidate_bodies() {
    let source = concat!(
        "package p\n",
        "type A struct{}\nfunc (A) Mark() {}\n",
        "type B struct{ Mark func() }\n",
        "func many(a A, b B) { a.Mark(); b.Mark(); a.Mark() }\n",
        "func single(a A) { a.Mark() }\n",
    );
    let mut server = TestServer::start("highlight-method-prefix", &[("main.go", source)]);
    let uri = server.uri("main.go");
    server.did_open(&uri, source);
    let uses: Vec<_> = source
        .match_indices("a.Mark()")
        .map(|(i, _)| i + 2)
        .collect();
    let declaration = source.find("Mark()").unwrap();
    let expected: Vec<_> = std::iter::once(declaration)
        .chain(uses.iter().copied())
        .map(|offset| json!({"kind": 1, "range": range(source, offset, offset + 4)}))
        .collect();
    for &offset in [&uses[0], &uses[2]] {
        let reply = server.document_highlight(&uri, source, offset);
        assert_eq!(reply["result"], json!(expected));
    }
}

#[test]
fn default_diagnostics_wait_for_save() {
    let initial = "package p\nfunc f() { _ = openMissing }\n";
    let edited = "package p\nfunc f() { _ = savedMissing }\n";
    let fixed = "package p\nfunc f() { _ = 1 }\n";
    let mut server = TestServer::start_session(
        "save-diagnostics",
        &[
            ("go.mod", "module example.test/save\n\ngo 1.27\n"),
            ("p.go", initial),
        ],
        ".",
        false,
        |_| json!({"goroot": "", "fileWatch": false}),
    );
    let uri = server.uri("p.go");
    server.did_open(&uri, initial);
    server.request_before_diagnostics("workspace/symbol", json!({"query": "f"}), &uri);
    server.did_change(&uri, 2, edited);
    server.did_change_watched_files(&[(&uri, 2)]);
    server.request_before_diagnostics("workspace/symbol", json!({"query": "f"}), &uri);

    std::fs::write(server.root().join("p.go"), edited).expect("save the edited fixture");
    server.did_save(&uri);
    let diagnostics = server.published_diagnostics(&uri, 2);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(diagnostics[0]["message"], "undefined: savedMissing");

    server.did_change(&uri, 3, fixed);
    server.request_before_diagnostics("workspace/symbol", json!({"query": "f"}), &uri);
    std::fs::write(server.root().join("p.go"), fixed).expect("save the fixed fixture");
    server.did_save(&uri);
    assert!(server.published_diagnostics(&uri, 3).is_empty());
    server.shutdown();
}

#[test]
fn sema_member_completion_ignores_unrelated_receiver_budgets() {
    let mut source = String::from(concat!(
        "package p\ntype Target struct{ Value int }\ntype Alias = Target\n",
        "func (Alias) Extra() {}\nfunc (*Target) Read() int { return 0 }\n",
        "type Other struct{}\n",
    ));
    for index in 0..4500 {
        source.push_str(&format!("func (Other) Unrelated{index}() {{}}\n"));
    }
    source.push_str(
        "func probe(value *Target) { _ = value.Read; _ = value.Value; _ = value.Extra }\n",
    );
    let mut server = TestServer::start_configured(
        "member-inventory",
        &[
            ("go.mod", "module example.com/members\n\ngo 1.27\n"),
            ("p.go", &source),
        ],
        None,
        true,
    );
    let uri = server.uri("p.go");
    server.did_open(&uri, &source);
    for (name, kind, detail, edit) in [
        ("Read", 2, "func() int", "Read()"),
        ("Value", 5, "int", "Value"),
        ("Extra", 2, "func()", "Extra()"),
    ] {
        let start = source.rfind(&format!("value.{name}")).unwrap() + "value.".len();
        let reply = server.completion(&uri, &source, start + 3);
        let item = completion_item(
            name,
            kind,
            Some(detail),
            2,
            edit,
            range(&source, start, start + name.len()),
            0,
        );
        assert_eq!(
            reply["result"],
            json!({"isIncomplete": true, "items": [item]}),
            "{name}: {reply}"
        );
    }
    server.shutdown();
}

#[test]
fn sema_signature_docs_follow_the_universe_error_method_identity() {
    let source = concat!(
        "package p\ntype Alias = error\ntype Wrapped interface{ error }\n",
        "type Local struct{}\nfunc (Local) Error() string { return \"\" }\n",
        "func probe(err error, alias Alias, wrapped Wrapped, local Local) {\n",
        " _ = err.Error()\n _ = alias.Error()\n _ = wrapped.Error()\n _ = local.Error()\n}\n",
    );
    let mut server = TestServer::start(
        "universe-method-docs",
        &[
            ("go.mod", "module example.com/errorhelp\n\ngo 1.27\n"),
            ("p.go", source),
        ],
    );
    let uri = server.uri("p.go");
    server.did_open(&uri, source);
    for receiver in ["err", "alias", "wrapped", "local"] {
        let call = format!("{receiver}.Error(");
        let offset = source.find(&call).unwrap() + call.len();
        let response = server.signature_help_invoked(&uri, source, offset);
        let doc = if receiver == "local" {
            ""
        } else {
            "Error returns the error message."
        };
        assert_eq!(
            response["result"],
            json!({
                "signatures": [{"label": "Error() string", "documentation": {"kind": "markdown", "value": doc}}],
                "activeSignature": 0,
            }),
            "{receiver}: {response}"
        );
    }
    server.shutdown();
}

#[test]
fn sema_implementation_checks_only_required_native_signatures() {
    let mut source = String::from(concat!(
        "package p\ntype Reader interface { Read() int }\ntype Target struct{}\n",
        "func (*Target) Read() int { return 0 }\n",
        "type Wrong struct{}\nfunc (Wrong) Read() string { return \"\" }\n",
        "type Shadow struct{ Read func() int }\n",
    ));
    for index in 0..400 {
        source.push_str(&format!(
            "func (*Target) Spare{index}(value map[int][]*Target) (func() int, chan *Target) {{ return nil, nil }}\n"
        ));
    }
    let mut server = TestServer::start(
        "required-methods",
        &[
            ("go.mod", "module example.com/required\n\ngo 1.27\n"),
            ("p.go", &source),
        ],
    );
    let uri = server.uri("p.go");
    server.did_open(&uri, &source);
    let type_start = source.find("Target struct").unwrap();
    let method_start = source.find("func (*Target) Read").unwrap() + "func (*Target) ".len();
    for (query, target, length) in [
        (
            source.find("Reader interface").unwrap(),
            type_start,
            "Target".len(),
        ),
        (
            source.find("Read() int").unwrap(),
            method_start,
            "Read".len(),
        ),
    ] {
        let response = server.implementation(&uri, &source, query);
        assert_eq!(
            response["result"],
            locations(&uri, &source, target, length),
            "{response}"
        );
    }
    server.shutdown();
}

#[test]
fn sema_assignment_diagnostics_follow_unsaved_regions() {
    let source = "package main\nfunc f() int {\n\tvar 数 int\n\tvar text string\n\t数 = text\n\treturn 数\n}\n";
    let server = TestServer::start(
        "assignment-diagnostics",
        &[
            ("go.mod", "module example.com/m\n\ngo 1.24\n"),
            ("main.go", source),
        ],
    );
    let uri = server.uri("main.go");
    server.did_open(&uri, source);
    let diagnostics = server.published_diagnostics(&uri, 1);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(diagnostics[0]["code"], "IncompatibleAssign");
    assert_eq!(diagnostics[0]["source"], "compiler");
    assert_eq!(
        diagnostics[0]["message"],
        "cannot use text (variable of type string) as int value in assignment"
    );
    let start = source.find("数 = text").unwrap() + "数 = ".len();
    assert_eq!(diagnostics[0]["range"], range(source, start, start + 4));
    server.did_change(&uri, 2, &source.replace("var text string", "var text int"));
    assert!(server.published_diagnostics(&uri, 2).is_empty());
    server.shutdown();
}

#[test]
fn diagnostics_follow_parsed_statement_shapes() {
    let valid = concat!(
        "package p\n",
        "func f(ch chan int) {\n",
        "    type count = int\n",
        "    select {\n",
        "    case x := (<-ch) /* receive */ :\n",
        "        var value count = x\n",
        "        _ = value\n",
        "    default:\n",
        "    }\n",
        "}\n",
    );
    let server = TestServer::start(
        "statement-diagnostics",
        &[
            ("go.mod", "module example.com/m\n\ngo 1.24\n"),
            ("main.go", valid),
        ],
    );
    let uri = server.uri("main.go");
    server.did_open(&uri, valid);
    assert!(server.published_diagnostics(&uri, 1).is_empty());

    for (index, (body, code, message, highlighted)) in [
        (
            "select { case x := 4: _ = x }",
            Some("InvalidSelectCase"),
            "select case must be send or receive (possibly with assignment)",
            "x := 4",
        ),
        (
            "for ; ; i := 0 { break }",
            Some("InvalidPostDecl"),
            "cannot declare in post statement",
            "i := 0",
        ),
        (
            "if 17 {}",
            Some("InvalidCond"),
            "non-boolean condition in if statement",
            "17",
        ),
        (
            "_ = func() { /* 😀 */ go (f(ch)) }; missing()",
            None,
            "expression in go must not be parenthesized",
            "(f(ch))",
        ),
        (
            "f(ch\n)",
            None,
            "missing ',' before newline in argument list",
            "\n)",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let source = format!("package p\nfunc f(ch chan int) {{ {body} }}\n");
        let version = index as i64 + 2;
        server.did_change(&uri, version, &source);
        let diagnostics = server.published_diagnostics(&uri, version);
        assert_eq!(diagnostics.len(), 1, "{source}\n{diagnostics:?}");
        assert_eq!(diagnostics[0]["message"], message);
        assert_eq!(diagnostics[0]["code"].as_str(), code);
        let start = source.find(highlighted).expect("diagnostic source");
        let end = if code.is_some() {
            start + highlighted.len()
        } else {
            start
        };
        assert_eq!(diagnostics[0]["range"], range(&source, start, end));
    }
    let renamed_import = "package p\nimport (\n\t_ \"strings\"\n\tunused \"os\"\n)\n";
    server.did_change(&uri, 7, renamed_import);
    let diagnostics = server.published_diagnostics(&uri, 7);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(diagnostics[0]["code"], "UnusedImport");
    assert_eq!(
        diagnostics[0]["message"],
        "\"os\" imported as unused and not used"
    );
    assert_eq!(diagnostics[0]["range"], range_of(renamed_import, "unused"));
    server.did_change(&uri, 8, valid);
    assert!(server.published_diagnostics(&uri, 8).is_empty());
    server.shutdown();
}

#[test]
fn diagnostics_match_go_types_operand_counts() {
    // Reduced cases from stmt0.go, decls0.go, expr3.go and conversions0.go
    // in Go 1.27.3's internal/types/testdata/check corpus.
    let cases: &[DiagnosticCase<'_>] = &[
        (
            concat!(
                "func f(x int, a []int) {\n",
                " x := 1; _ := 2; y, y := 1, 2; _ = y\n",
                " a[0] := 2\n",
                " const n = 1; n := 2\n",
                "}\n",
            ),
            &[
                ("NoNewVar", "no new variables on left side of :=", "x := 1"),
                ("NoNewVar", "no new variables on left side of :=", "_ := 2"),
                ("RepeatedDecl", "y repeated on left side of :=", "y"),
                ("BadDecl", "non-name a[0] on left side of :=", "a[0]"),
                ("UnassignableOperand", "cannot assign to n", "n"),
            ],
        ),
        (
            concat!(
                "func pair() (int, int) { return 0, 0 }\n",
                "func noValue(...int) {}\n",
                "func f(a, b, c int) {\n",
                " a,b,c = 1,2\n",
                " a,b,c = pair()\n",
                " a = pair()\n",
                " a,b += 1,2\n",
                " var d,e,f = 1,2; _,_,_ = d,e,f\n",
                " x,y,z := 1,2; _,_,_ = x,y,z\n",
                " const i, j = 1\n",
                " a = noValue /* ERROR \"used as value\" */ (1, 2)\n",
                "}\n",
            ),
            &[
                (
                    "WrongAssignCount",
                    "assignment mismatch: 3 variables but 2 values",
                    "1",
                ),
                (
                    "WrongAssignCount",
                    "assignment mismatch: 3 variables but pair returns 2 values",
                    "pair()",
                ),
                (
                    "WrongAssignCount",
                    "assignment mismatch: 1 variable but pair returns 2 values",
                    "pair()",
                ),
                (
                    "MultiValAssignOp",
                    "assignment operation += requires single-valued expressions",
                    "a,b += 1,2",
                ),
                ("WrongAssignCount", "missing init expr for f", "f"),
                (
                    "WrongAssignCount",
                    "assignment mismatch: 3 variables but 2 values",
                    "1",
                ),
                ("WrongAssignCount", "missing init expr for j", "j"),
                (
                    "TooManyValues",
                    "noValue(1, 2) (no value) used as value",
                    "noValue /* ERROR \"used as value\" */ (1, 2)",
                ),
            ],
        ),
        (
            concat!(
                "func f() (int, int) { return 1 }\n",
                "func g() int { return 1, 2 }\n",
                "func h() int { return }\n",
                "func i() (x int) { return }\n",
                "func pair() (int, int) { return 0, 0 }\n",
                "func j() int { return pair() }\n",
            ),
            &[
                (
                    "WrongResultCount",
                    "not enough return values\n\thave (number)\n\twant (int, int)",
                    "1",
                ),
                (
                    "WrongResultCount",
                    "too many return values\n\thave (number, number)\n\twant (int)",
                    "2",
                ),
                (
                    "WrongResultCount",
                    "not enough return values\n\thave ()\n\twant (int)",
                    "return",
                ),
                (
                    "WrongResultCount",
                    "too many return values\n\thave (int, int)\n\twant (int)",
                    "pair()",
                ),
            ],
        ),
        (
            concat!(
                "func f1(int) {}\n",
                "func g2() (float32, string) { return 0, \"\" }\n",
                "func variadic(int, ...string) {}\n",
                "func f(x int, xs []int) {\n",
                " f1()\n",
                " f1(x, x)\n",
                " f1(g2())\n",
                " variadic()\n",
                " f1(xs...)\n",
                " _ = int()\n",
                " _ = int(1, 2)\n",
                "}\n",
            ),
            &[
                (
                    "WrongArgCount",
                    "not enough arguments in call to f1\n\thave ()\n\twant (int)",
                    "",
                ),
                (
                    "WrongArgCount",
                    "too many arguments in call to f1\n\thave (int, int)\n\twant (int)",
                    "x",
                ),
                (
                    "WrongArgCount",
                    "too many arguments in call to f1\n\thave (float32, string)\n\twant (int)",
                    "g2()",
                ),
                (
                    "WrongArgCount",
                    "not enough arguments in call to variadic\n\thave ()\n\twant (int, ...string)",
                    "",
                ),
                (
                    "NonVariadicDotDotDot",
                    "cannot use ... in call to non-variadic f1",
                    "f1(xs...)",
                ),
                (
                    "WrongArgCount",
                    "missing argument in conversion to int",
                    "int()",
                ),
                (
                    "WrongArgCount",
                    "too many arguments in conversion to int",
                    "2",
                ),
            ],
        ),
        (
            concat!(
                "func pair() (int, int) { return 0, 0 }\n",
                "func sink(...int) {}\n",
                "func genericSink[T any](...T) {}\n",
                "func f(m map[string]int, ch chan int, x any) (int, int) {\n",
                " a,b := pair(); c,d := (m[\"x\"]); e,f := (<-ch); g,h := (x.(int))\n",
                " _,_,_,_,_,_,_,_ = a,b,c,d,e,f,g,h\n",
                " sink(); sink(1, 2); sink([]int{1}...)\n",
                " genericSink(1); genericSink[int]([]int{1}...)\n",
                " a, more := 2, 3; _,_ = a,more\n",
                " return pair()\n",
                "}\n",
            ),
            &[],
        ),
        (
            "func f() { a,b := Missing,0,1; _,_ = a,b }\n",
            &[("UndeclaredName", "undefined: Missing", "Missing")],
        ),
    ];
    assert_diagnostic_cases("go-types-count-diagnostics", cases);
}

#[test]
fn diagnostics_match_go_types_builtins_indices_and_constants() {
    // Reduced from Go's builtins0.go, expr3.go, const0.go and shifts.go.
    let cases: &[DiagnosticCase<'_>] = &[
        (
            r#"func pair() (int, int) { return 1, 2 }
func f(s []int, n int, in <-chan int, x float32, y float64) {
 _ = len()
 _ = cap(pair())
 _ = len(n)
 _ = cap("x")
 _ = len(s /* ... inside a comment */ ...)
 _ = append(nil, 1)
 _ = append(s, "x")
 _ = append(s, 1.5)
 _ = append(s...)
 _ = append(s, 1, s...)
 _ = copy(s, []string{})
 _ = make(int)
 _ = make([]int)
 _ = make([]int, 2, 1)
 _ = make([]int, -1)
 _ = make([]int, -2, -3)
 _ = new(nil)
 close(in)
 clear(n)
 delete(n, 0)
 _ = complex(n, n)
 _ = complex(x, y)
 _ = imag(x)
 _ = real(n)
 _ = min(false)
 _ = max(x, y)
}
"#,
            &[
                (
                    "WrongArgCount",
                    "invalid operation: not enough arguments for len() (expected 1, found 0)",
                    "len()",
                ),
                (
                    "WrongArgCount",
                    "invalid operation: too many arguments for cap(pair()) (expected 1, found 2)",
                    "cap(pair())",
                ),
                (
                    "InvalidLen",
                    "invalid argument: n (variable of type int) for built-in len",
                    "n",
                ),
                (
                    "InvalidCap",
                    "invalid argument: \"x\" (untyped string constant) for built-in cap",
                    "\"x\"",
                ),
                (
                    "InvalidDotDotDot",
                    "invalid operation: invalid use of ... with built-in len",
                    "...",
                ),
                (
                    "InvalidAppend",
                    "invalid append: argument must be a slice; have untyped nil",
                    "nil",
                ),
                (
                    "IncompatibleAssign",
                    "cannot use \"x\" (untyped string constant) as int value in argument to append",
                    "\"x\"",
                ),
                (
                    "TruncatedFloat",
                    "cannot use 1.5 (untyped float constant) as int value in argument to append (truncated)",
                    "1.5",
                ),
                (
                    "WrongArgCount",
                    "not enough arguments in call to append\n\thave ([]int...)\n\twant ([]int, ...int)",
                    "",
                ),
                (
                    "WrongArgCount",
                    "too many arguments in call to append\n\thave ([]int, number, []int...)\n\twant ([]int, ...int)",
                    "s",
                ),
                (
                    "InvalidCopy",
                    "invalid copy: arguments s (variable of type []int) and []string{} (value of type []string) have different element types int and string",
                    "s",
                ),
                (
                    "InvalidMake",
                    "invalid argument: cannot make int: type must be slice, map, or channel",
                    "int",
                ),
                (
                    "WrongArgCount",
                    "invalid operation: make([]int) expects 2 or 3 arguments; found 1",
                    "make([]int)",
                ),
                (
                    "SwappedMakeArgs",
                    "invalid argument: length and capacity swapped",
                    "2",
                ),
                (
                    "InvalidIndex",
                    "invalid argument: index -1 (constant of type int) must not be negative",
                    "-1",
                ),
                (
                    "InvalidIndex",
                    "invalid argument: index -2 (constant of type int) must not be negative",
                    "-2",
                ),
                (
                    "InvalidIndex",
                    "invalid argument: index -3 (constant of type int) must not be negative",
                    "-3",
                ),
                (
                    "UntypedNilUse",
                    "use of untyped nil in argument to new",
                    "nil",
                ),
                (
                    "InvalidClose",
                    "invalid operation: cannot close receive-only channel in (variable of type <-chan int)",
                    "in",
                ),
                (
                    "InvalidClear",
                    "invalid argument: cannot clear n (variable of type int): argument must be (or constrained by) map or slice",
                    "n",
                ),
                (
                    "InvalidDelete",
                    "invalid argument: n (variable of type int) is not a map",
                    "n",
                ),
                (
                    "InvalidComplex",
                    "invalid argument: arguments have type int, expected floating-point",
                    "n",
                ),
                (
                    "InvalidComplex",
                    "invalid operation: complex(x, y) (mismatched types float32 and float64)",
                    "x",
                ),
                (
                    "InvalidImag",
                    "invalid argument: argument has type float32, expected complex type",
                    "x",
                ),
                (
                    "InvalidReal",
                    "invalid argument: argument has type int, expected complex type",
                    "n",
                ),
                (
                    "InvalidMinMaxOperand",
                    "invalid argument: false (untyped bool constant) cannot be ordered",
                    "false",
                ),
                (
                    "MismatchedTypes",
                    "invalid argument: mismatched types float32 (previous argument) and float64 (type of y)",
                    "y",
                ),
            ],
        ),
        (
            r#"func f(a [2]int, s []int, m map[int]bool, text string, f float64, n int) {
 _ = a[2]
 _ = (&a)[-1]
 _ = "a"[1]
 _ = s[1.5]
 _ = s[f]
 _ = s[true]
 _ = n[0]
 _ = s[0, int]
 _ = m[text]
 _ = m[1 << 100]
 _, _ = m["x"]
}
"#,
            &[
                (
                    "InvalidIndex",
                    "invalid argument: index 2 out of bounds [0:2]",
                    "2",
                ),
                (
                    "InvalidIndex",
                    "invalid argument: index -1 (constant of type int) must not be negative",
                    "-1",
                ),
                (
                    "InvalidIndex",
                    "invalid argument: index 1 out of bounds [0:1]",
                    "1",
                ),
                (
                    "TruncatedFloat",
                    "1.5 (untyped float constant) truncated to int",
                    "1.5",
                ),
                (
                    "InvalidIndex",
                    "invalid argument: index f (variable of type float64) must be integer",
                    "f",
                ),
                (
                    "InvalidConstVal",
                    "cannot convert true (untyped bool constant) to type int",
                    "true",
                ),
                (
                    "NonIndexableOperand",
                    "cannot index n (variable of type int)",
                    "n",
                ),
                (
                    "InvalidIndex",
                    "invalid operation: more than one index",
                    "int",
                ),
                (
                    "IncompatibleAssign",
                    "cannot use text (variable of type string) as int value in map index",
                    "text",
                ),
                (
                    "NumericOverflow",
                    "cannot use 1 << 100 (untyped int constant 1267650600228229401496703205376) as int value in map index (overflows)",
                    "1 << 100",
                ),
                (
                    "IncompatibleAssign",
                    "cannot use \"x\" (untyped string constant) as int value in map index",
                    "\"x\"",
                ),
            ],
        ),
        (
            r#"func f(a [2]int, s []int, f float64, n int) {
 _ = a[:3]
 _ = s[2:1]
 _ = "x"[0:1:1]
 _ = [2]int{}[:]
 _ = n[:]
 _ = a[-1:f]
}
"#,
            &[
                (
                    "InvalidIndex",
                    "invalid argument: index 3 out of bounds [0:3]",
                    "3",
                ),
                ("SwappedSliceIndices", "invalid slice indices: 1 < 2", "1"),
                (
                    "InvalidSliceExpr",
                    "invalid operation: 3-index slice of string",
                    "1",
                ),
                (
                    "NonSliceableOperand",
                    "cannot slice unaddressable value [2]int{} (value of type [2]int)",
                    "[2]int{}",
                ),
                (
                    "NonSliceableOperand",
                    "cannot slice n (variable of type int)",
                    "n",
                ),
                (
                    "InvalidIndex",
                    "invalid argument: index -1 (constant of type int) must not be negative",
                    "-1",
                ),
                (
                    "InvalidIndex",
                    "invalid argument: index f (variable of type float64) must be integer",
                    "f",
                ),
            ],
        ),
        (
            r#"const _ = 1 / 0
const _ = 1 / 0.0
const _ = 1 << -1
const _ = 1 << 1.5
const _ = 1 << 1075
const _ = uint8(256)
const _ = int(1.5)
const _ = int8(127) + 1
const _ uint8 = 256
func f(n int, x float64) {
 _ = n % 0
 n /= 0
 _ = x / 0
 _ = x << 2
 _ = n << x
 var m uint8 = 256
 _ = m
}
"#,
            &[
                ("DivByZero", "invalid operation: division by zero", "0"),
                ("DivByZero", "invalid operation: division by zero", "0.0"),
                (
                    "InvalidShiftCount",
                    "invalid operation: negative shift count -1 (untyped int constant)",
                    "-1",
                ),
                (
                    "TruncatedFloat",
                    "1.5 (untyped float constant) truncated to uint",
                    "1.5",
                ),
                (
                    "InvalidShiftCount",
                    "invalid operation: invalid shift count 1075 (untyped int constant)",
                    "1075",
                ),
                ("InvalidConversion", "constant 256 overflows uint8", "256"),
                (
                    "InvalidConversion",
                    "cannot convert 1.5 (untyped float constant) to type int",
                    "1.5",
                ),
                (
                    "NumericOverflow",
                    "int8(127) + 1 (constant 128 of type int8) overflows int8",
                    "int8(127) + 1",
                ),
                (
                    "NumericOverflow",
                    "cannot use 256 (untyped int constant) as uint8 value in constant declaration (overflows)",
                    "256",
                ),
                (
                    "InvalidShiftOperand",
                    "invalid operation: shifted operand x (variable of type float64) must be integer",
                    "x",
                ),
                (
                    "InvalidShiftCount",
                    "invalid operation: shift count x (variable of type float64) must be integer",
                    "x",
                ),
                ("DivByZero", "invalid operation: division by zero", "0"),
                ("DivByZero", "invalid operation: division by zero", "0"),
                (
                    "NumericOverflow",
                    "cannot use 256 (untyped int constant) as uint8 value in variable declaration (overflows)",
                    "256",
                ),
            ],
        ),
        (
            r#"func pair() ([]int, []int) { return nil, nil }
func f[T ~[2]int | ~[]int](a T, m map[string]int, s []byte, in chan<- int, n float64) {
 _, _ = m["x"]
 _, _ = a[1.0], a[1+0i]
 _ = "a"[:1]
 _ = s[0:1:2]
 _ = len(a)
 _ = append(s, "x"...)
 _ = copy(pair())
 _ = make([]int, 1, 2)
 _ = new(1)
 _ = n / 0
 const _ = 1 << 127
 const _ = 1.0 << 5
 close(in)
 clear(m)
 delete(m, "x")
}
"#,
            &[],
        ),
    ];
    assert_diagnostic_cases("go-types-operand-diagnostics", cases);
}

#[test]
fn diagnostics_track_reads_when_an_imported_callee_is_invalid() {
    let source = concat!(
        "package p\n",
        "import \"example.com/m/dep\"\n",
        "func f() {\n",
        "    var gate, up, nested *dep.MissingType\n",
        "    _ = dep.Broken(gate)\n",
        "    _, _ = dep.Broken(up)\n",
        "    consume(dep.Broken(nested))\n",
        "    read := 1\n",
        "    _ = dep.MissingCall(read)\n",
        "}\n",
        "func consume(...int) {}\n",
    );
    let server = TestServer::start(
        "imported-callee-diagnostics",
        &[
            ("go.mod", "module example.com/m\n\ngo 1.27\n"),
            ("main.go", source),
            ("dep/dep.go", "package dep\nvar Broken = excluded()\n"),
            (
                "dep/excluded.go",
                "//go:build never_tag\n\npackage dep\nfunc excluded() func(...any) int { return nil }\n",
            ),
        ],
    );
    let uri = server.uri("main.go");
    server.did_open(&uri, source);
    let rows = server.published_diagnostics(&uri, 1);
    let mut messages: Vec<_> = rows
        .iter()
        .map(|row| {
            (
                row["code"].as_str().unwrap(),
                row["message"].as_str().unwrap(),
            )
        })
        .collect();
    messages.sort_unstable();
    assert_eq!(
        messages,
        [
            ("UndeclaredImportedName", "undefined: dep.MissingCall"),
            ("UndeclaredImportedName", "undefined: dep.MissingType"),
            ("UnusedVar", "declared and not used: gate"),
            ("UnusedVar", "declared and not used: nested"),
            ("UnusedVar", "declared and not used: up"),
        ],
        "{rows:?}"
    );
    server.shutdown();
}

#[test]
fn sema_generic_nested_call_hover_and_signature_help() {
    let main_src = concat!(
        "package main\n\n",
        "type Pair[A, B any] struct {\n",
        "\tFirst  A\n",
        "\tSecond B\n",
        "}\n\n",
        "func PairOf[A, B any](a A, b B) Pair[A, B] {\n",
        "\treturn Pair[A, B]{First: a, Second: b}\n",
        "}\n\n",
        "func Wrap[T any](v T) []T {\n",
        "\treturn []T{v}\n",
        "}\n\n",
        "func main() {\n",
        "\tx := Wrap( PairOf(1, \"a\"))\n",
        "\t_ = x\n",
        "}\n",
    );
    let mut server = TestServer::start(
        "generic-nested",
        &[
            ("go.mod", "module example.com/m\n\ngo 1.24\n"),
            ("main.go", main_src),
        ],
    );
    let uri = server.uri("main.go");
    server.did_open(&uri, main_src);

    for (needle, expected, ctx) in [
        (
            "x :=",
            "var x []Pair[int, string]",
            "expected instantiated slice type on x",
        ),
        (
            "PairOf(1",
            "func PairOf(a int, b string) Pair[int, string]",
            "expected instantiated PairOf hover",
        ),
        (
            "Wrap( PairOf",
            "func Wrap(v Pair[int, string]) []Pair[int, string]",
            "expected instantiated Wrap hover",
        ),
    ] {
        let reply = server.hover(&uri, main_src, main_src.find(needle).unwrap());
        assert_hover_contains(&reply, expected, ctx);
    }

    let wrap_off = main_src.find("Wrap( PairOf").expect("Wrap call");
    let sig_wrap = server.signature_help(&uri, main_src, wrap_off + "Wrap(".len());
    assert!(
        sig_wrap["result"]["signatures"][0]["label"]
            .as_str()
            .is_some_and(|l| l.starts_with("Wrap(")),
        "expected Wrap signatureHelp: {sig_wrap}"
    );
    assert_eq!(sig_wrap["result"]["activeParameter"], 0, "{sig_wrap}");

    let pairof_second_off = main_src.find(", \"a\"").expect("second arg comma") + 1;
    let sig_pairof = server.signature_help(&uri, main_src, pairof_second_off);
    assert!(
        sig_pairof["result"]["signatures"][0]["label"]
            .as_str()
            .is_some_and(|l| l.starts_with("PairOf(")),
        "expected PairOf signatureHelp: {sig_pairof}"
    );
    assert_eq!(sig_pairof["result"]["activeParameter"], 1, "{sig_pairof}");

    server.shutdown();
}

#[test]
fn definition_reuses_grouped_import_spans_with_escapes() {
    let path = r#""example.test/im\x70orts/dep""#;
    let source = format!(
        "package p\nimport (\n\tπdep {path}\n)\n\nfunc f() {{ _ = \"🧪\"; _ = πdep.T(0) }}\nconst marker = \"import\"\n"
    );
    let dependency = "package dep\ntype T int\n";
    let mut server = TestServer::start(
        "grouped-import-navigation",
        &[
            ("go.mod", "module example.test/imports\n\ngo 1.27.3\n"),
            ("main.go", &source),
            ("dep/dep.go", dependency),
        ],
    );
    let uri = server.uri("main.go");
    let dep_uri = server.uri("dep/dep.go");
    server.did_open(&uri, &source);
    let written_path = source.find(path).unwrap();
    let alias = source.find("πdep").unwrap();
    let usage = source.find("πdep.T").unwrap();
    let target = dependency.find("T int").unwrap();
    for (offset, target_uri, target_source, selection, length) in [
        (
            written_path + 1,
            &uri,
            source.as_str(),
            written_path,
            path.len(),
        ),
        (usage, &uri, source.as_str(), alias, "πdep".len()),
        (usage + "πdep.".len(), &dep_uri, dependency, target, 1),
    ] {
        let reply = server.definition(&uri, &source, offset);
        assert_eq!(
            reply["result"],
            locations(target_uri, target_source, selection, length)
        );
    }
}

#[test]
fn sema_navigation_uses_checked_header_and_selector_identities() {
    let source = concat!(
        "package p\nimport alias \"example.com/navigation/dep\"\n",
        "type I interface{ Call(input Missing) (output Missing) }\n",
        "type S struct{ Field Missing }\nvar F func(arg Missing) (result Missing)\nvar arg int\n",
        "func Decl(value alias.T) (answer alias.T)\n",
        "func Probe[alias any](p alias) { _ = alias.T }\n",
        "func Use() { alias := struct{ T int }{}; _ = alias.T; _ = arg }\n",
    );
    let dependency = "package dep\ntype T int\n";
    let documented = "// Package p documents admitted files.\npackage p\n";
    let detached_doc = "// Package q belongs only to Windows.\npackage q\n";
    let other = "package q\n";
    let mut server = TestServer::start_in_target(
        "native-navigation",
        &[
            ("go.mod", "module example.com/navigation\n\ngo 1.27.3\n"),
            ("p/main.go", source),
            ("p/a_doc.go", documented),
            ("dep/dep.go", dependency),
            ("q/main.go", other),
            ("q/doc_windows_amd64.go", detached_doc),
        ],
        Some(("linux", "amd64")),
    );
    let uri = server.uri("p/main.go");
    server.did_open(&uri, source);
    for (written, relative, declaration, length) in [
        ("Call(input", 0, "Call(input", 4),
        ("input Missing", 0, "input Missing", 5),
        ("output Missing", 0, "output Missing", 6),
        ("Field Missing", 0, "Field Missing", 5),
        ("arg Missing", 0, "arg Missing", 3),
        ("result Missing", 0, "result Missing", 6),
        ("value alias.T", 0, "value alias.T", 5),
        ("answer alias.T", 0, "answer alias.T", 6),
        ("alias.T", 0, "alias \"example", 5),
        ("alias any", 0, "alias any", 5),
        ("p alias", 2, "alias any", 5),
        ("_ = arg", 4, "arg int", 3),
    ] {
        let reply = server.definition(&uri, source, source.find(written).unwrap() + relative);
        assert_eq!(
            reply["result"],
            locations(&uri, source, source.find(declaration).unwrap(), length),
            "{written}: {reply}"
        );
    }
    let dependency_uri = server.uri("dep/dep.go");
    let reply = server.definition(
        &uri,
        source,
        source.find("alias.T").unwrap() + "alias.".len(),
    );
    assert_eq!(
        reply["result"],
        locations(
            &dependency_uri,
            dependency,
            dependency.find("T int").unwrap(),
            1
        ),
        "{reply}"
    );
    let shadow = source.find("_ = alias.T").unwrap() + "_ = alias.".len();
    let reply = server.definition(&uri, source, shadow);
    assert!(reply.get("error").is_none(), "{reply}");
    assert!(
        reply["result"].is_null(),
        "shadowed import produced a guessed declaration: {reply}"
    );

    let selected = source.rfind("alias.T").unwrap() + "alias.".len();
    let uses: Vec<_> = source
        .match_indices("alias.T")
        .take(2)
        .map(|(start, _)| start + "alias.".len())
        .collect();
    let mut expected_references = vec![location(
        &dependency_uri,
        dependency,
        dependency.find("T int").unwrap(),
        1,
    )];
    expected_references.extend(uses.iter().map(|start| location(&uri, source, *start, 1)));
    let reply = server.references(&uri, source, uses[0], true);
    assert_eq!(
        reply["result"],
        json!(expected_references),
        "reference identity escaped its native object: {reply}"
    );
    let reply = server.document_highlight(&uri, source, uses[0]);
    assert_eq!(
        reply["result"],
        json!(
            uses.iter()
                .map(|start| json!({"range": range(source, *start, start + 1), "kind": 1}))
                .collect::<Vec<_>>()
        ),
        "highlight identity escaped its native object: {reply}"
    );
    let reply = server.rename(&uri, source, uses[0], "Renamed");
    assert_eq!(
        reply["result"],
        json!({"documentChanges": [
            {"textDocument": {"uri": dependency_uri, "version": 0}, "edits": [{"range": expected_references[0]["range"], "newText": "Renamed"}]},
            {"textDocument": {"uri": uri, "version": 1}, "edits": expected_references[1..].iter().map(|loc| json!({"range": loc["range"], "newText": "Renamed"})).collect::<Vec<_>>()},
        ]}),
        "rename used a spelling supplement instead of native references: {reply}"
    );
    let reply = server.definition(&uri, source, selected);
    assert_eq!(
        reply["result"],
        locations(&uri, source, source.find("T int").unwrap(), 1),
        "{reply}"
    );
    let reply = server.definition(&uri, source, "package ".len());
    assert_eq!(
        reply["result"],
        locations(
            &server.uri("p/a_doc.go"),
            documented,
            documented.rfind("p\n").unwrap(),
            1
        ),
        "{reply}"
    );
    let other_uri = server.uri("q/main.go");
    server.did_open(&other_uri, other);
    let reply = server.definition(&other_uri, other, "package ".len());
    assert_eq!(
        reply["result"],
        locations(&other_uri, other, "package ".len(), 1),
        "excluded disk file escaped admission: {reply}"
    );
    server.shutdown();
}

#[test]
fn sema_orphan_view_admits_offline_dependency_variants() {
    let source = "package p\nimport \"example.com/view/dep\"\nvar Read = dep.Value\nvar Missing = dep.WindowsOnly\nvar Local = Shared{}\n";
    let mut server = TestServer::start_in_target(
        "orphan-variants",
        &[
            ("go.mod", "module example.com/view\n\ngo 1.27.3\n"),
            ("p/base.go", "package p\ntype Shared struct{}\n"),
            ("p/use_linux_amd64.go", source),
            (
                "dep/value_windows_amd64.go",
                "package dep\nconst Value string = \"host\"\nconst WindowsOnly = 1\n",
            ),
            (
                "dep/value_linux_amd64.go",
                "package dep\nimport \"example.com/view/leaf\"\nconst Value leaf.Number = 7\n",
            ),
            (
                "leaf/number_linux_amd64.go",
                "package leaf\ntype Number uint64\n",
            ),
        ],
        Some(("windows", "amd64")),
    );
    let uri = server.uri("p/use_linux_amd64.go");
    server.did_open(&uri, source);
    for (name, expected) in [
        ("Read", Some("var Read leaf.Number")),
        ("Missing", None),
        ("Local", Some("var Local Shared")),
    ] {
        let reply = server.hover(&uri, source, source.find(name).unwrap());
        if let Some(expected) = expected {
            assert_hover_contains(
                &reply,
                expected,
                &format!("orphan dependency view lost {name}"),
            );
        } else {
            assert!(
                reply["result"].is_null(),
                "host-only declaration escaped into orphan view: {reply}"
            );
        }
    }
    let reply = server.definition(
        &uri,
        source,
        source.find("dep.Value").unwrap() + "dep.".len(),
    );
    assert_eq!(
        reply["result"][0]["uri"],
        server.uri("dep/value_linux_amd64.go"),
        "{reply}"
    );
    server.did_close(&uri);
    let host_source = "package p\nimport \"example.com/view/dep\"\nvar Host = dep.Value\n";
    let host_uri = server.uri("p/host.go");
    server.did_open(&host_uri, host_source);
    let reply = server.hover(&host_uri, host_source, host_source.find("Host").unwrap());
    assert_hover_contains(
        &reply,
        "var Host string",
        "orphan close damaged the host package view",
    );
    server.shutdown();
}

#[test]
fn sema_request_view_survives_shared_declarations_and_hierarchy_items() {
    let source = concat!(
        "package p\nimport (\n\"example.com/request/bridge\"\n\"example.com/request/leaf\"\n)\n",
        "var Read = bridge.Value\nvar Direct port.Number\n",
        "var Selected = bridge.Make().Chosen()\nvar Wide uint64 = 1 << 40\n",
        "var TooWide = int(1 << 40)\nvar Bad [1 << 32]int\n",
        "var Object bridge.Common\ntype Sized struct{ Word uintptr; Tail byte }\n",
        "type Embedded struct{ bridge.Common }\n",
    );
    let bridge = concat!(
        "package bridge\nimport leaf \"example.com/request/leaf\"\n",
        "var Value leaf.Number\ntype Common struct{}\n",
        "type Want interface{ Chosen() int32 }\n",
        "func Make() Common { _ = leaf.Target(); return Common{} }\n",
    );
    let leaf = "package port\ntype Number uint64\nfunc Target() Number { return 0 }\n";
    let mut server = TestServer::start_in_target(
        "request-view",
        &[
            ("go.mod", "module example.com/request\n\ngo 1.27.3\n"),
            ("p/base.go", "package p\n"),
            ("p/use_linux_386.go", source),
            ("bridge/shared.go", bridge),
            (
                "bridge/m_linux_386.go",
                "package bridge\nfunc (Common) Chosen() int32 { return 0 }\n",
            ),
            (
                "bridge/m_windows_amd64.go",
                "package bridge\nfunc (Common) Chosen() string { return \"\" }\n",
            ),
            ("leaf/number_linux_386.go", leaf),
            (
                "leaf/number_windows_amd64.go",
                "package host\ntype Number string\nfunc Target() Number { return \"host\" }\n",
            ),
        ],
        Some(("windows", "amd64")),
    );
    let uri = server.uri("p/use_linux_386.go");
    server.did_open(&uri, source);
    let number_use = source.find("port.Number").unwrap() + "port.".len();
    let reply = server.references(&uri, source, number_use, true);
    let leaf_uri = server.uri("leaf/number_linux_386.go");
    let return_type = leaf.find("Target() Number").unwrap() + "Target() ".len();
    assert_eq!(
        reply["result"],
        json!([
            {"uri": server.uri("bridge/shared.go"), "range": range_of(bridge, "Number")},
            {"uri": leaf_uri, "range": range_of(leaf, "Number")},
            location(&leaf_uri, leaf, return_type, "Number".len()),
            location(&uri, source, number_use, "Number".len()),
        ]),
        "cross-file references lost the request view or its default import name: {reply}"
    );
    for (name, ty) in [
        ("Read", "port.Number"),
        ("Direct", "port.Number"),
        ("Selected", "int32"),
        ("Wide", "uint64"),
    ] {
        let reply = server.hover(&uri, source, source.find(name).unwrap());
        assert!(reply.get("error").is_none(), "{reply}");
        assert_eq!(
            reply["result"],
            json!({
                "contents": {"kind": "markdown", "value": format!("```go\nvar {name} {ty}\n```\n\n---\n\n[`p.{name}` on pkg.go.dev](https://pkg.go.dev/example.com/request/p#{name})")},
                "range": range_of(source, name),
            }),
            "request view lost for {name}: {reply}"
        );
    }
    for name in ["TooWide", "Bad"] {
        let reply = server.hover(&uri, source, source.find(name).unwrap());
        assert!(reply.get("error").is_none(), "{reply}");
        assert!(
            reply["result"].is_null(),
            "32-bit overflow accepted for {name}: {reply}"
        );
    }
    let reply = server.hover(&uri, source, source.find("Word").unwrap());
    assert_eq!(
        reply["result"],
        json!({
            "contents": {"kind": "markdown", "value": "```go\nfield Word uintptr // size=4, class=8, offset=0\n```\n\n---\n\n[`(p.Sized).Word` on pkg.go.dev](https://pkg.go.dev/example.com/request/p#Sized.Word)"},
            "range": range_of(source, "Word"),
        }),
        "layout used the host ABI: {reply}"
    );

    let prepared = server.text_doc_pos(
        "textDocument/prepareCallHierarchy",
        &uri,
        source,
        source.find("Make").unwrap(),
    );
    assert_eq!(prepared["result"][0]["data"]["viewUri"], uri, "{prepared}");
    let outgoing = server.request(
        "callHierarchy/outgoingCalls",
        json!({"item": prepared["result"][0]}),
    );
    let call = bridge.find("leaf.Target").unwrap() + "leaf.".len();
    assert_eq!(
        outgoing["result"],
        json!([{
            "to": {
                "name": "Target", "kind": 12,
                "detail": "example.com/request/leaf \u{2022} number_linux_386.go",
                "uri": leaf_uri, "range": range_of(leaf, "Target"), "selectionRange": range_of(leaf, "Target"),
                "data": {"uri": leaf_uri, "start": leaf.find("Target").unwrap(), "name": "Target"},
            },
            "fromRanges": [range(bridge, call, call + "Target".len())],
        }]),
        "call continuation lost request view: {outgoing}"
    );
    let prepared = server.text_doc_pos(
        "textDocument/prepareTypeHierarchy",
        &uri,
        source,
        source.find("bridge.Common").unwrap() + "bridge.".len(),
    );
    assert_eq!(prepared["result"][0]["data"]["viewUri"], uri, "{prepared}");
    let supers = server.request(
        "typeHierarchy/supertypes",
        json!({"item": prepared["result"][0]}),
    );
    assert_eq!(
        supers["result"],
        json!([{
            "name": "Want", "kind": 11, "detail": "example.com/request/bridge",
            "uri": server.uri("bridge/shared.go"), "range": range_of(bridge, "Want"), "selectionRange": range_of(bridge, "Want"),
            "data": {"viewUri": uri},
        }]),
        "method relation continuation lost request view: {supers}"
    );
    let subtypes = server.request(
        "goquery/embedding/subtypes",
        json!({"item": prepared["result"][0]}),
    );
    let embedded = json!({
        "name": "Embedded", "kind": 5, "detail": "example.com/request/p", "uri": uri,
        "range": range_of(source, "Embedded"), "selectionRange": range_of(source, "Embedded"),
    });
    assert_eq!(
        subtypes["result"],
        json!([embedded]),
        "embedding lookup lost the original request view: {subtypes}"
    );
    let supers = server.request("goquery/embedding/supertypes", json!({"item": embedded}));
    assert_eq!(
        supers["result"], prepared["result"],
        "native embedding continuation lost its shared declaration: {supers}"
    );
    server.shutdown();
}

#[test]
fn sema_completion_context_ranks_native_types_and_edits_literal_keys() {
    let source = concat!(
        "package p\ntype Box[T any] struct{ Value T; Count int }\n",
        "var PickA string\nvar PickB int\nfunc Handler(v int) int { return v }\n",
        "func f() {\nvar _ int = Pick\nvar _ func(int)int = Hand\n",
        "_ = Box[string]{\nCount:1,\nVa\n}\n_ = Box[string]{Valu: \"\"}\n",
        "_ = Box[string]{\nCount:1,\nVal,\n}\n}\n",
    );
    let mut server = TestServer::start_configured(
        "completion-context",
        &[
            ("go.mod", "module example.com/context\n\ngo 1.27.3\n"),
            ("p.go", source),
        ],
        Some(("linux", "amd64")),
        true,
    );
    let uri = server.uri("p.go");
    server.did_open(&uri, source);
    for (written, prefix, expected) in [
        (
            "= Pick",
            "Pick",
            vec![("PickB", 6, None, "PickB"), ("PickA", 6, None, "PickA")],
        ),
        (
            "= Hand",
            "Hand",
            vec![("Handler", 3, Some("func(v int) int"), "Handler")],
        ),
        (
            "\nVa\n",
            "Va",
            vec![("Value", 5, Some("string"), "Value: ${1:string},")],
        ),
        (
            "{Valu:",
            "Valu",
            vec![("Value", 5, Some("string"), "Value")],
        ),
        (
            "\nVal,",
            "Val",
            vec![("Value", 5, Some("string"), "Value: ${1:string}")],
        ),
    ] {
        let start = source.find(written).unwrap() + written.find(prefix).unwrap();
        let reply = server.completion(&uri, source, start + prefix.len());
        let edit_range = range(source, start, start + prefix.len());
        let items: Vec<_> = expected
            .into_iter()
            .enumerate()
            .map(|(rank, (label, kind, detail, text))| {
                completion_item(label, kind, detail, 2, text, edit_range.clone(), rank)
            })
            .collect();
        assert_eq!(
            reply["result"],
            json!({"isIncomplete": true, "items": items}),
            "{written}: {reply}"
        );
    }
    server.shutdown();
}

#[test]
fn sema_package_document_links_preserve_the_request_build_view() {
    let source = "package p\nimport b \"example.com/docview/bridge\"\nvar _ b.Common\n";
    let shared =
        "// Package bridge documents [Common.Chosen].\npackage bridge\ntype Common struct{}\n";
    let linux = "package bridge\nfunc (Common) Chosen() int32 { return 0 }\n";
    let mut server = TestServer::start_in_target(
        "package-doc-view",
        &[
            ("go.mod", "module example.com/docview\n\ngo 1.27.3\n"),
            ("p/base.go", "package p\n"),
            ("p/use_linux_386.go", source),
            ("bridge/shared.go", shared),
            ("bridge/m_linux_386.go", linux),
            (
                "bridge/m_windows_amd64.go",
                "package bridge\nfunc (Common) Chosen() string { return \"\" }\n",
            ),
        ],
        Some(("windows", "amd64")),
    );
    let uri = server.uri("p/use_linux_386.go");
    server.did_open(&uri, source);
    let reply = server.hover(&uri, source, source.find("b \"").unwrap());
    let path_start = source.find('"').unwrap();
    let path_end = source[path_start + 1..].find('"').unwrap() + path_start + 2;
    let target = pos(linux, linux.find("Chosen").unwrap());
    let link = format!(
        "{}#{},{}",
        server.uri("bridge/m_linux_386.go"),
        target["line"].as_u64().unwrap() + 1,
        target["character"].as_u64().unwrap() + 1
    );
    assert_eq!(
        reply["result"],
        json!({
            "contents": {"kind": "markdown", "value": format!("```go\npackage bridge\n```\n\n---\n\nPackage bridge documents [Common.Chosen]({link}).\n")},
            "range": range(source, path_start, path_end),
        }),
        "package docs lost the original native member view: {reply}"
    );
    server.shutdown();
}

#[test]
fn sema_literal_keys_refresh_with_module_language_and_respect_file_overrides() {
    let source = concat!(
        "package p\ntype Base[T any] struct{ Value T }\n",
        "type Outer struct{ Base[string] }\nfunc f(){ _ = Outer{Va} }\n",
    );
    let downgraded = format!("//go:build go1.26\n\n{source}");
    let mut server = TestServer::start_configured(
        "literal-version",
        &[
            ("go.mod", "module example.com/version\ngo 1.26.1\n"),
            ("p.go", source),
            ("old/p.go", &downgraded),
        ],
        Some(("linux", "amd64")),
        true,
    );
    let uri = server.uri("p.go");
    server.did_open(&uri, source);
    let mod_uri = server.uri("go.mod");
    let old_uri = server.uri("old/p.go");
    for (version, promoted) in [(26, false), (27, true), (26, false)] {
        std::fs::write(
            server.root().join("go.mod"),
            format!("module example.com/version\ngo 1.{version}.1\n"),
        )
        .unwrap();
        server.did_change_watched_files(&[(&mod_uri, 2)]);
        let start = source.find("{Va}").unwrap() + 1;
        let reply = server.completion(&uri, source, start + 2);
        let items = if promoted {
            vec![completion_item(
                "Value",
                5,
                Some("string"),
                2,
                "Value: ${1:string}",
                range(source, start, start + 2),
                0,
            )]
        } else {
            Vec::new()
        };
        assert_eq!(
            reply["result"],
            json!({"isIncomplete": true, "items": items}),
            "module go1.{version}: {reply}"
        );
        server.did_open(&old_uri, &downgraded);
        let start = downgraded.find("{Va}").unwrap() + 1;
        let reply = server.completion(&old_uri, &downgraded, start + 2);
        assert_eq!(
            reply["result"],
            json!({"isIncomplete": true, "items": []}),
            "file override: {reply}"
        );
    }
    server.shutdown();
}

#[test]
fn sema_generic_member_hover_and_completion_use_unimported_native_instances() {
    let library = concat!(
        "package a\ntype Box[T any] struct{ Value T; Transform func(input T) (output T) }\n",
        "type Port[T any] interface{ Do(value T) (result T) }\n",
    );
    let bridge = concat!(
        "package b\nimport \"example.com/native/a\"\n",
        "func Make() a.Box[string] { return a.Box[string]{} }\n",
        "func Port() a.Port[int] { return nil }\n",
    );
    let source = concat!(
        "package c\nimport \"example.com/native/b\"\n",
        "func f() { s := b.Make(); p := b.Port(); _ = s.Value; _ = s.Transform; _ = p.Do }\n",
    );
    let mut server = TestServer::start(
        "native-member-hover",
        &[
            ("go.mod", "module example.com/native\n\ngo 1.27.3\n"),
            ("a/a.go", library),
            ("b/b.go", bridge),
            ("c/c.go", source),
        ],
    );
    let uri = server.uri("c/c.go");
    server.did_open(&uri, source);
    for (written, code, owner) in [
        ("s.Value", "field Value string", "Box"),
        (
            "s.Transform",
            "field Transform func(input string) (output string)",
            "Box",
        ),
        (
            "p.Do",
            "func (a.Port[int]) Do(value int) (result int)",
            "Port",
        ),
    ] {
        let name = &written[2..];
        let start = source.find(written).unwrap() + 2;
        let reply = server.hover(&uri, source, start);
        assert_eq!(
            reply["result"],
            json!({
                "contents": {"kind": "markdown", "value": format!("```go\n{code}\n```\n\n---\n\n[`(a.{owner}).{name}` on pkg.go.dev](https://pkg.go.dev/example.com/native/a#{owner}.{name})")},
                "range": range(source, start, start + name.len()),
            }),
            "generic member type escaped its native instance: {reply}"
        );
    }
    for (written, detail, kind) in [
        ("s.Value", "string", 5),
        ("s.Transform", "func(input string) (output string)", 5),
        ("p.Do", "func(value int) (result int)", 2),
    ] {
        let name = &written[2..];
        let start = source.find(written).unwrap() + 2;
        let reply = server.completion(&uri, source, start + 1);
        let item = completion_item(
            name,
            kind,
            Some(detail),
            1,
            name,
            range(source, start, start + name.len()),
            0,
        );
        assert_eq!(
            reply["result"],
            json!({"isIncomplete": true, "items": [item]}),
            "completion lost the checked generic instance: {reply}"
        );
    }
    server.shutdown();
}

#[test]
fn sema_definition_and_declaration_share_builtin_and_assembly_locations() {
    let go = concat!(
        "package p\n",
        "func Asm()\n",
        "func NoText()\n",
        "func f() { Asm(); NoText(); _ = len(\"abc\"); missing() }\n",
    );
    let assembly = "TEXT ·Asm(SB), $0-0\n    RET\n";
    let builtin = "package builtin\n\nfunc len(v Type) int\n";
    let sdk_file = ".sdk/src/builtin/builtin.go";
    let mut server = TestServer::start_with_sdk(
        "definition-locations",
        &[
            ("go.mod", "module example.com/definitions\n\ngo 1.27\n"),
            ("main.go", go),
            ("main.s", assembly),
            (sdk_file, builtin),
        ],
        Some(("linux", "amd64")),
        false,
        Some(".sdk"),
        ".",
    );
    let uri = server.uri("main.go");
    server.did_open(&uri, go);
    let loc_of = |file: &str, source: &str, name: &str| {
        locations(
            &server.uri(file),
            source,
            source.find(name).unwrap(),
            name.len(),
        )
    };
    for (name, expected) in [
        ("Asm(", loc_of("main.s", assembly, "Asm")),
        ("NoText(", loc_of("main.go", go, "NoText")),
        ("len(", loc_of(sdk_file, builtin, "len")),
        ("missing(", Value::Null),
    ] {
        for method in ["textDocument/definition", "textDocument/declaration"] {
            let reply = server.text_doc_pos(method, &uri, go, go.rfind(name).unwrap());
            assert!(reply.get("error").is_none(), "{method} {name}: {reply}");
            assert_eq!(reply["result"], expected, "{method} {name}: {reply}");
        }
    }
    server.shutdown();
}

#[test]
fn sema_type_definition_returns_source_locations_in_result_order() {
    let named = "package dep\n\ntype Remote struct{}\n";
    let builtin = "package builtin\n\ntype int int\n";
    let disk = concat!(
        "package p\n",
        "import \"example.com/m/dep\"\n",
        "var Mixed func() (dep.Remote, int, dep.Remote)\n",
        "func f() {\n",
        "    type 本地 struct{}\n",
        "    var local 本地\n",
        "    _ = local\n",
        "}\n",
    );
    let edited = disk.replace("type 本地", "type /* 😀 */ 本地");
    let sdk_file = ".sdk/src/builtin/builtin.go";
    let mut server = TestServer::start_with_sdk(
        "type-definition-locations",
        &[
            ("go.mod", "module example.com/m\n\ngo 1.27\n"),
            ("main.go", disk),
            ("dep/dep.go", named),
            (sdk_file, builtin),
        ],
        None,
        false,
        Some(".sdk"),
        ".",
    );
    let uri = server.uri("main.go");
    server.did_open(&uri, disk);
    server.did_change(&uri, 2, &edited);

    let loc = |file: &str, source: &str, declaration: &str, name: &str| {
        let start = source.find(declaration).unwrap() + declaration.len() - name.len();
        location(&server.uri(file), source, start, name.len())
    };
    let remote = loc("dep/dep.go", named, "type Remote", "Remote");
    let int = loc(sdk_file, builtin, "type int", "int");
    let local = loc("main.go", &edited, "type /* 😀 */ 本地", "本地");

    for (name, expected) in [
        ("Mixed", json!([remote, int, remote])),
        ("local\n", json!([local])),
    ] {
        let reply = server.type_definition(&uri, &edited, edited.rfind(name).unwrap());
        assert_eq!(reply["result"], expected, "{name}: {reply}");
    }
    server.shutdown();
}

#[test]
fn sema_unimported_transitive_type_hover_and_completion() {
    let a_src = concat!(
        "package a\n\n",
        "type Secret struct {\n",
        "\tValue int\n",
        "}\n\n",
        "func (Secret) Ping() string { return \"ok\" }\n",
    );
    let b_src =
        "package b\n\nimport \"example.com/m/a\"\n\nfunc Make() a.Secret { return a.Secret{} }\n";
    let c_src = concat!(
        "package c\n\n",
        "import \"example.com/m/b\"\n\n",
        "func run() {\n",
        "\ts := b.Make()\n",
        "\t_ = s.Ping()\n",
        "}\n",
    );
    let mut server = TestServer::start(
        "transitive",
        &[
            ("go.mod", "module example.com/m\n\ngo 1.24\n"),
            ("a/a.go", a_src),
            ("b/b.go", b_src),
            ("c/c.go", c_src),
        ],
    );
    let c_uri = server.uri("c/c.go");
    let a_uri = server.uri("a/a.go");
    server.did_open(&c_uri, c_src);

    let s_off = c_src.find("s :=").expect("s decl");
    let hover_s = server.hover(&c_uri, c_src, s_off);
    assert_hover_contains(
        &hover_s,
        "var s a.Secret",
        "expected qualified transitive type on s",
    );

    let tdef_s = server.type_definition(&c_uri, c_src, s_off);
    assert_eq!(
        tdef_s["result"][0]["uri"].as_str(),
        Some(a_uri.as_str()),
        "expected typeDefinition to point to a/a.go: {tdef_s}"
    );

    let c_edit = concat!(
        "package c\n\n",
        "import \"example.com/m/b\"\n\n",
        "func run() {\n",
        "\ts := b.Make()\n",
        "\ts.\n",
        "}\n",
    );
    server.did_change(&c_uri, 2, c_edit);
    let comp_off = c_edit.find("\ts.").expect("selector dot") + "\ts.".len();
    let comp = server.completion(&c_uri, c_edit, comp_off);
    let items = comp["result"]["items"]
        .as_array()
        .expect("completion items");
    for label in ["Value", "Ping"] {
        assert!(
            items.iter().any(|item| item["label"] == label),
            "expected {label} in transitive completion: {comp}"
        );
    }

    server.shutdown();
}

#[test]
fn sema_conversion_suppresses_signature_help() {
    let main_src = concat!(
        "package main\n\n",
        "type MyInt int\n\n",
        "func Take(v MyInt) {}\n\n",
        "func main() {\n",
        "\ty := MyInt(42)\n",
        "\tTake( MyInt(42))\n",
        "\t_ = y\n",
        "}\n",
    );
    let mut server = TestServer::start(
        "conversion-sig",
        &[
            ("go.mod", "module example.com/m\n\ngo 1.24\n"),
            ("main.go", main_src),
        ],
    );
    let uri = server.uri("main.go");
    server.did_open(&uri, main_src);

    let sig_conv =
        server.signature_help(&uri, main_src, main_src.find("42").expect("conversion arg"));
    assert_eq!(
        sig_conv["result"],
        Value::Null,
        "conversion must suppress signatureHelp: {sig_conv}"
    );

    let take_arg_off = main_src.find("Take( MyInt").expect("Take call") + "Take(".len();
    let sig_take = server.signature_help(&uri, main_src, take_arg_off);
    assert_eq!(
        sig_take["result"]["signatures"][0]["label"].as_str(),
        Some("Take(v MyInt)"),
        "expected Take signatureHelp: {sig_take}"
    );
    assert_eq!(sig_take["result"]["activeParameter"], 0, "{sig_take}");

    server.shutdown();
}

#[test]
fn sema_streaming_semantic_tokens_and_local_defined_types() {
    let main_src = concat!(
        "package main\n\n",
        "const (\n",
        "\tBase = iota + 2\n",
        "\tNext\n",
        ")\n\n",
        "var _ = Base\n",
        "var Arr [Base * Base]int\n",
        "var PairA, PairB = [4]int{}, 7\n\n",
        "func first() int {\n",
        "\ttype Fn1 func() int\n",
        "\ttype Fn2 func() int\n",
        "\tvar f1 Fn1 = func() int { return PairB }\n",
        "\t_ = f1\n",
        "\tvar raw func() int = f1\n",
        "\tvar _ Fn2 = raw\n",
        "\treturn f1() + raw()\n",
        "}\n\n",
        "func second() int {\n",
        "\treturn len(Arr) + len(PairA) + Next\n",
        "}\n\n",
        "func third() int {\n",
        "\tx := first() + second()\n",
        "\treturn x\n",
        "}\n",
    );
    let mut server = TestServer::start(
        "streaming-tokens",
        &[
            ("go.mod", "module example.com/m\n\ngo 1.21\n"),
            ("main.go", main_src),
        ],
    );
    let uri = server.uri("main.go");
    server.did_open(&uri, main_src);

    let full = server.semantic_tokens_full(&uri);
    let full_tokens = decode_semantic_tokens(
        full["result"]["data"]
            .as_array()
            .expect("semanticTokens/full data array"),
    );

    for (start_needle, end_needle, label) in [
        ("func first()", "\nfunc second()", "first()"),
        ("func second()", "\nfunc third()", "second()"),
    ] {
        let start_off = main_src.find(start_needle).expect("range start");
        let end_off = main_src.find(end_needle).expect("range end");
        let range_reply = server.semantic_tokens_range(&uri, main_src, start_off, end_off);
        let range_tokens = decode_semantic_tokens(
            range_reply["result"]["data"]
                .as_array()
                .expect("range data"),
        );
        let start_line = main_src[..start_off].matches('\n').count() as u32;
        let end_line = main_src[..end_off].matches('\n').count() as u32;
        let expected: Vec<_> = full_tokens
            .iter()
            .copied()
            .filter(|(line, ..)| *line >= start_line && *line <= end_line)
            .collect();
        assert_eq!(
            range_tokens, expected,
            "range window over {label} must match full tokens slice"
        );
    }

    let token_at = |needle: &str| {
        let off = main_src.find(needle).expect(needle);
        let p = pos(main_src, off);
        let target_line = p["line"].as_u64().expect("line") as u32;
        let target_col = p["character"].as_u64().expect("character") as u32;
        full_tokens
            .iter()
            .copied()
            .find(|(line, col, ..)| *line == target_line && *col == target_col)
            .unwrap_or_else(|| panic!("missing token for {needle:?} at {target_line}:{target_col}"))
    };

    const MOD_DEFINITION: u32 = 1 << 1;
    const MOD_READONLY: u32 = 1 << 2;
    const MOD_SIGNATURE: u32 = 1 << 5;
    const MOD_NUMBER: u32 = 1 << 6;
    const MOD_ARRAY: u32 = 1 << 12;

    // Package blank declaration `var _ = Base` vs local blank assignment `_ = f1`
    assert_ne!(
        token_at("_ = Base").4 & MOD_DEFINITION,
        0,
        "package blank declaration should carry definition modifier"
    );
    assert_eq!(
        token_at("_ = f1").4 & MOD_DEFINITION,
        0,
        "local blank assignment must not carry definition modifier"
    );
    // Non-decimal constant array shape [Base * Base]int on Arr and composite literal [4]int{} on PairA
    assert_ne!(
        token_at("Arr) + len(PairA)").4 & MOD_ARRAY,
        0,
        "Arr ([Base * Base]int) should carry array shape modifier"
    );
    assert_ne!(
        token_at("PairA) + Next").4 & MOD_ARRAY,
        0,
        "PairA ([4]int{{}}) should carry array shape modifier"
    );
    // Local defined function type signature modifier
    assert_ne!(
        token_at("f1\n\tvar raw").4 & MOD_SIGNATURE,
        0,
        "f1 (local defined func() int) should carry signature modifier"
    );
    // Const iota progression readonly + number modifier on Next
    let next_use = token_at("Next\n}");
    assert_eq!(
        next_use.4 & (MOD_READONLY | MOD_NUMBER),
        MOD_READONLY | MOD_NUMBER,
        "Next const use should carry both readonly and number modifiers: {next_use:?}"
    );
    // Local defined type assignable to unnamed func() int check: raw's initializer f1 stays typed
    assert_ne!(
        token_at("f1\n\tvar _ Fn2").4 & MOD_SIGNATURE,
        0,
        "f1 assigned to raw func() int should retain signature modifier"
    );

    server.shutdown();
}

#[test]
fn sdk_navigation_follows_private_functions_and_promoted_methods() {
    let client = "package main\nimport \"stdnav\"\nfunc main() { stdnav.Start(nil) }\n";
    let entry = concat!(
        "package stdnav\n",
        "type Runner interface { run(); Ready() }\n",
        "type Partial interface { run() }\n",
        "func Start(r Runner) {\n",
        "\tvalidate()\n\tr.run()\n\tgoto done\ndone:\n\tundefinedName()\n}\n",
    );
    let helpers = concat!(
        "package stdnav\nfunc validate() {}\ntype worker struct{}\n",
        "func (worker) run() {}\nfunc (worker) Ready() {}\n",
    );
    let mut server = TestServer::start_with_sdk(
        "private-sdk-navigation",
        &[
            ("app/go.mod", "module example.com/private\n\ngo 1.27\n"),
            ("app/main.go", client),
            ("_sdk/src/builtin/builtin.go", "package builtin\n"),
            ("_sdk/src/stdnav/entry.go", entry),
            ("_sdk/src/stdnav/helpers.go", helpers),
            (
                "app/bridge/bridge.go",
                "package bridge\nimport \"stdnav\"\ntype Middle struct { stdnav.Partial }\n",
            ),
            (
                "app/wrap/wrap.go",
                concat!(
                    "package wrap\nimport \"example.com/private/bridge\"\n",
                    "type Wrapper struct { bridge.Middle }\n",
                    "func (Wrapper) Ready() {}\n",
                ),
            ),
            (
                "app/rival/rival.go",
                concat!(
                    "package rival\ntype Other struct{}\n",
                    "func (Other) run() {}\nfunc (Other) Ready() {}\n",
                ),
            ),
        ],
        None,
        false,
        Some("_sdk"),
        "app",
    );
    let client_uri = server.uri("app/main.go");
    let entry_uri = server.uri("_sdk/src/stdnav/entry.go");
    let helper_uri = server.uri("_sdk/src/stdnav/helpers.go");
    let definition = server.definition(&client_uri, client, client.find("Start").unwrap());
    assert_eq!(definition["result"][0]["uri"], entry_uri, "{definition}");
    server.did_open(&entry_uri, entry);
    let follow_up = json!({
        "textDocument": {"uri": entry_uri},
        "position": pos(entry, entry.find("validate").unwrap()),
    });
    let second =
        server.request_before_diagnostics("textDocument/definition", follow_up.clone(), &entry_uri);
    let diagnostics = server.published_diagnostics(&entry_uri, 1);
    assert!(
        diagnostics
            .iter()
            .any(|row| row["code"] == "UndeclaredName"),
        "{diagnostics:?}"
    );
    let declaration = server.request("textDocument/declaration", follow_up);
    for reply in [second, declaration] {
        assert_eq!(reply["result"][0]["uri"], helper_uri, "{reply}");
        assert_eq!(
            reply["result"][0]["range"]["start"],
            pos(helpers, helpers.find("validate").unwrap()),
            "{reply}"
        );
    }
    let label = server.definition(&entry_uri, entry, entry.find("done").unwrap());
    assert_eq!(
        label["result"][0]["range"]["start"],
        pos(entry, entry.find("done:").unwrap()),
        "{label}"
    );

    let implementations = server.implementation(&entry_uri, entry, entry.find(".run").unwrap() + 1);
    let rows = implementations["result"]
        .as_array()
        .expect("method implementations");
    for (uri, start) in [
        (&helper_uri, pos(helpers, helpers.find("run()").unwrap())),
        (&entry_uri, pos(entry, entry.find("run() }").unwrap())),
    ] {
        assert_has_location(&implementations, uri, &start, "missing implementation");
    }
    assert_eq!(rows.len(), 2, "{implementations}");
    server.shutdown();
}

#[test]
fn module_cache_open_defers_diagnostics_for_follow_up_definition() {
    let client = "package p\nimport \"example.test/dep\"\nfunc run() { dep.Public() }\n";
    let generated = "package dep\nfunc Public() { hidden(); missing() }\n";
    let helper = "package dep\nfunc hidden() {}\n";
    let dependency = "_gomodcache/example.test/dep@v1.0.0";
    let generated_path = format!("{dependency}/api_gen.go");
    let helper_path = format!("{dependency}/helper.go");
    let mut server = TestServer::start_with_sdk(
        "module-cache-navigation",
        &[
            (
                "app/go.mod",
                "module example.test/app\n\ngo 1.27\n\nrequire example.test/dep v1.0.0\n",
            ),
            ("app/main.go", client),
            (
                "_gomodcache/cache/download/example.test/dep/@v/v1.0.0.mod",
                "module example.test/dep\n\ngo 1.27\n",
            ),
            (&generated_path, generated),
            (&helper_path, helper),
        ],
        None,
        false,
        None,
        "app",
    );
    let generated_uri = server.uri(&generated_path);
    let first = server.definition(
        &server.uri("app/main.go"),
        client,
        client.find("Public").unwrap(),
    );
    assert_eq!(first["result"][0]["uri"], generated_uri, "{first}");

    server.did_open(&generated_uri, generated);
    let second = server.request_before_diagnostics(
        "textDocument/definition",
        json!({
            "textDocument": {"uri": generated_uri},
            "position": pos(generated, generated.find("hidden").unwrap()),
        }),
        &generated_uri,
    );
    assert_eq!(second["result"][0]["uri"], server.uri(&helper_path));
    assert_eq!(
        second["result"][0]["range"]["start"],
        pos(helper, helper.find("hidden").unwrap())
    );
    let diagnostics = server.published_diagnostics(&generated_uri, 1);
    assert!(
        diagnostics
            .iter()
            .any(|row| row["code"] == "UndeclaredName"),
        "{diagnostics:?}"
    );
    server.shutdown();
}

#[test]
fn sema_promoted_receiver_members_and_interface_implementation() {
    let store_src = concat!(
        "package store\n\n",
        "type Reader interface {\n",
        "\tReadItem(id int) (string, bool)\n",
        "}\n\n",
        "type Base struct {\n",
        "\tPrefix string\n",
        "}\n\n",
        "func (b *Base) ReadItem(id int) (string, bool) {\n",
        "\treturn b.Prefix, id > 0\n",
        "}\n\n",
        "type Service struct {\n",
        "\t*Base\n",
        "}\n",
    );
    let main_src = concat!(
        "package main\n\n",
        "import \"example.com/app/pkg/store\"\n\n",
        "func run() {\n",
        "\tsvc := &store.Service{Base: &store.Base{Prefix: \"ok\"}}\n",
        "\tval, ok := svc.ReadItem(1)\n",
        "\tbuf := make([]string, 0, 2)\n",
        "\tbuf = append(buf, val)\n",
        "\t_, _ = buf, ok\n",
        "}\n",
    );
    let mut server = TestServer::start(
        "promoted-members",
        &[
            ("go.mod", "module example.com/app\n\ngo 1.24\n"),
            ("pkg/store/store.go", store_src),
            ("main.go", main_src),
        ],
    );
    let main_uri = server.uri("main.go");
    let store_uri = server.uri("pkg/store/store.go");
    server.did_open(&store_uri, store_src);
    server.did_open(&main_uri, main_src);

    let hover_val = server.hover(
        &main_uri,
        main_src,
        main_src.find("val, ok").expect("val decl"),
    );
    assert_hover_contains(&hover_val, "var val string", "expected string hover on val");

    let hover_ok = server.hover(
        &main_uri,
        main_src,
        main_src.find("ok := svc").expect("ok decl"),
    );
    assert_hover_contains(&hover_ok, "var ok bool", "expected bool hover on ok");

    let def_read = server.definition(
        &main_uri,
        main_src,
        main_src.find("ReadItem(1)").expect("ReadItem call"),
    );
    let read_impl_off = store_src
        .find("func (b *Base) ReadItem")
        .expect("method def")
        + "func (b *Base) ".len();
    assert_has_location(
        &def_read,
        &store_uri,
        &pos(store_src, read_impl_off),
        "expected promoted method definition in pkg/store/store.go",
    );

    let impls = server.implementation(
        &store_uri,
        store_src,
        store_src.find("Reader interface").expect("Reader decl"),
    );
    let base_pos = pos(store_src, store_src.find("Base struct").expect("Base decl"));
    let service_pos = pos(
        store_src,
        store_src.find("Service struct").expect("Service decl"),
    );
    let impl_rows = impls["result"]
        .as_array()
        .unwrap_or_else(|| panic!("expected implementation list: {impls}"));
    assert!(
        impl_rows.iter().any(|loc| {
            loc["uri"].as_str() == Some(store_uri.as_str())
                && (loc["range"]["start"] == base_pos || loc["range"]["start"] == service_pos)
        }),
        "expected Base or Service to implement Reader: {impls}"
    );

    let main_edit = concat!(
        "package main\n\n",
        "import \"example.com/app/pkg/store\"\n\n",
        "func run() {\n",
        "\tsvc := &store.Service{Base: &store.Base{Prefix: \"ok\"}}\n",
        "\tsvc.\n",
        "\t_ = svc\n",
        "}\n",
    );
    server.did_change(&main_uri, 2, main_edit);
    let comp_off = main_edit.find("\tsvc.").expect("svc dot") + "\tsvc.".len();
    let comp = server.completion(&main_uri, main_edit, comp_off);
    let items = comp["result"]["items"]
        .as_array()
        .expect("completion items");
    for label in ["ReadItem", "Prefix"] {
        assert!(
            items.iter().any(|item| item["label"] == label),
            "expected promoted member {label} in completion: {comp}"
        );
    }

    server.shutdown();
}

#[test]
fn sema_composite_key_hover_prefers_field_over_shadowing_local() {
    let src = concat!(
        "package main\n\n",
        "type entry struct {\n",
        "\tt int\n",
        "}\n\n",
        "func build() entry {\n",
        "\tvar t int\n",
        "\treturn entry{t: t}\n",
        "}\n",
    );
    let mut server = TestServer::start(
        "composite-key-hover-shadow",
        &[
            ("go.mod", "module example.com/m\n\ngo 1.27.3\n"),
            ("main.go", src),
        ],
    );
    let uri = server.uri("main.go");
    server.did_open(&uri, src);

    let key_off = src.find("entry{t: t}").unwrap() + "entry{".len();
    let val_off = key_off + "t: ".len();

    let key_hover = server.hover(&uri, src, key_off);
    assert_hover_contains(
        &key_hover,
        "field t int",
        "expected field hover on composite key",
    );

    let val_hover = server.hover(&uri, src, val_off);
    assert_hover_contains(
        &val_hover,
        "var t int",
        "expected var hover on composite value",
    );

    server.shutdown();
}

#[test]
fn sema_document_highlight_large_composite_table_preserves_all_writes() {
    let mut src = String::from(
        "package main\n\ntype Item struct {\n\tFirst []int\n\tValues []int\n}\n\nfunc testTable() {\n\tcases := []struct {\n\t\tname string\n\t\ta *Item\n\t\tb *Item\n\t}{\n",
    );
    for i in 0..50 {
        src.push_str(&format!(
            "\t\t{{\"case-{i}\", &Item{{First: []int{{1, 2}}, Values: []int{{3, 4}}}}, &Item{{First: []int{{5, 6}}, Values: []int{{7, 8}}}}}},\n"
        ));
    }
    src.push_str("\t}\n\t_ = cases\n}\n");

    let mut server = TestServer::start(
        "highlight-large-composite-table",
        &[
            ("go.mod", "module example.com/m\n\ngo 1.27.3\n"),
            ("main.go", &src),
        ],
    );
    let uri = server.uri("main.go");
    server.did_open(&uri, &src);

    let last_values = src.rfind("Values:").unwrap();
    let reply = server.document_highlight(&uri, &src, last_values);
    let items = reply["result"]
        .as_array()
        .expect("documentHighlight result array");
    assert_eq!(items.len(), 101, "expected decl + 100 writes: {reply}");
    assert_eq!(items[0]["kind"], 1, "decl should be Text: {}", items[0]);
    assert!(
        items[1..].iter().all(|row| row["kind"] == 3),
        "every composite key should be Write: {reply}"
    );

    server.shutdown();
}
