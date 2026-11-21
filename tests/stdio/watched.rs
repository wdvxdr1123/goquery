use crate::harness::{
    TempDir, TestServer, assert_diagnostics_contain, assert_first_location_uri,
    assert_has_location, assert_hover_contains, decode_semantic_tokens, dir_uri, file_uri,
    goquery_command, hover_text, pos, range,
};
use serde_json::{Value, json};
use std::process::Stdio;
use std::time::Duration;

pub fn run_all() {
    check_dynamic_watched_files();
    check_direct_disk_file_watch();
    check_nested_module_and_sema_features();
    check_watch_limits_and_invalid_regex();
    check_pipelined_registration();
}

fn def_by_name(server: &mut TestServer, uri: &str, source: &str, name: &str) -> Value {
    let offset = source.find(name).expect("query identifier");
    server.definition(uri, source, offset)
}

fn check_dynamic_watched_files() {
    let root = TempDir::from_fixture("watched-files", "tests/fixtures/watched-files");
    let mut cmd = goquery_command();
    cmd.env(
        "GOMODCACHE",
        std::env::temp_dir().join("goquery-watched-modcache"),
    );
    let mut server = TestServer::from_command(cmd, true);

    let root_uri = root.dir_uri();
    server.initialize_ok(json!({
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
        "initializationOptions": {"goroot": "", "diagnostics": {"onSave": false}},
    }));

    let registration = server.wait_registration();
    let watcher = registration["registrations"]
        .as_array()
        .expect("registrations")
        .iter()
        .find(|reg| reg["method"] == "workspace/didChangeWatchedFiles")
        .expect("watched-file registration");
    let patterns: Vec<&str> = watcher["registerOptions"]["watchers"]
        .as_array()
        .expect("watch patterns")
        .iter()
        .filter_map(|w| w["globPattern"].as_str())
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
        json!({"event": {"added": [{"uri": root_uri, "name": "watched-again"}], "removed": []}}),
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

    let main = root.read("main.go");
    let other = root.read("other/other.go");
    let lib = root.read("lib/lib.go");
    let main_uri = root.uri("main.go");
    let other_uri = root.uri("other/other.go");
    let lib_uri = root.uri("lib/lib.go");
    server.did_open(&main_uri, &main);
    server.did_open(&other_uri, &other);

    let reply = def_by_name(&mut server, &other_uri, &other, "Value");
    assert_first_location_uri(&reply, &lib_uri);

    // A closed file changes on disk without any editor notification for the file
    // itself; only the watched-file batch reports it.
    let renamed = lib.replace(
        "// Value returns the current value.\nfunc Value() int {",
        "// Renamed returns the current value.\nfunc Renamed() int {",
    );
    assert_ne!(renamed, lib, "fixture declaration must change");
    root.write("lib/lib.go", &renamed);
    server.did_change_watched_files(&[(&lib_uri, 2)]);
    let diagnostics = server.wait_diagnostics(&[main_uri.as_str(), other_uri.as_str()]);
    for uri in [&main_uri, &other_uri] {
        let items = diagnostics.get(uri).expect("dependent diagnostics");
        assert_diagnostics_contain(items, "undefined: lib.Value", uri);
    }

    // The refreshed index resolves the renamed declaration for a dependent
    // package's unsaved buffer, including its hover signature.
    let changed_other = other.replace("lib.Value()", "lib.Renamed()");
    assert_ne!(changed_other, other);
    server.did_change(&other_uri, 2, &changed_other);
    let reply = def_by_name(&mut server, &other_uri, &changed_other, "Renamed");
    assert_first_location_uri(&reply, &lib_uri);
    assert_eq!(reply["result"][0]["range"]["start"]["line"], 3, "{reply}");
    assert_eq!(
        reply["result"][0]["range"]["start"]["character"], 5,
        "{reply}"
    );
    let reply = server.hover(
        &other_uri,
        &changed_other,
        changed_other.find("Renamed").expect("hover"),
    );
    assert_hover_contains(&reply, "func lib.Renamed() int", "renamed hover");

    // Created files join the index on their watched event.
    root.write(
        "lib/extra.go",
        "package lib\n\nfunc Extra() int {\n\treturn 3\n}\n",
    );
    let extra_uri = root.uri("lib/extra.go");
    server.did_change_watched_files(&[(&extra_uri, 1)]);
    let changed_main = main.replace("lib.Value()", "lib.Extra()");
    assert_ne!(changed_main, main);
    server.did_change(&main_uri, 2, &changed_main);
    let reply = def_by_name(&mut server, &main_uri, &changed_main, "Extra");
    assert_first_location_uri(&reply, &extra_uri);

    // Module manifests are planned from disk; a saved/watched go.mod change
    // refreshes the plan and the renamed module still resolves after the
    // dependent import is updated.
    let mod_uri = root.uri("go.mod");
    root.write("go.mod", "module example.test/renamed\n\ngo 1.27.3\n");
    server.did_change_watched_files(&[(&mod_uri, 2)]);
    let renamed_import =
        changed_other.replace("example.test/watched/lib", "example.test/renamed/lib");
    assert_ne!(renamed_import, changed_other);
    server.did_change(&other_uri, 3, &renamed_import);
    let reply = def_by_name(&mut server, &other_uri, &renamed_import, "Renamed");
    assert_first_location_uri(&reply, &lib_uri);

    // Re-watching an unchanged main manifest must not rebuild the index: the
    // plan digest is unchanged and package metadata stays valid.
    server.did_change_watched_files(&[(&mod_uri, 2)]);
    let reply = def_by_name(&mut server, &other_uri, &renamed_import, "Renamed");
    assert_first_location_uri(&reply, &lib_uri);

    // A save (without any didOpen for that file) reindexes the on-disk content,
    // so a new declaration is available to already-open consumers.
    root.write(
        "lib/lib.go",
        "package lib\n\n// Renamed returns the current value.\nfunc Renamed() string { return \"\" }\n\nfunc Saved() int { return 4 }\n",
    );
    server.did_save(&lib_uri);
    let saved_use = renamed_import.replace(
        "func Use() int {",
        "func SavedUse() int {\n\treturn lib.Saved()\n}\n\nfunc Use() int {",
    );
    assert_ne!(saved_use, renamed_import);
    server.did_change(&other_uri, 4, &saved_use);
    let saved_offset = saved_use.find("lib.Saved").expect("saved use") + "lib.".len();
    let reply = server.definition(&other_uri, &saved_use, saved_offset);
    assert_first_location_uri(&reply, &lib_uri);

    // A save that removes a declaration republishes diagnostics for open
    // importers, not just the edited file.
    root.write(
        "lib/lib.go",
        "package lib\n\nfunc Renamed() string { return \"\" }\n",
    );
    server.did_save(&lib_uri);
    let diagnostics = server.wait_diagnostics(&[other_uri.as_str()]);
    let items = diagnostics.get(&other_uri).expect("importer diagnostics");
    assert_diagnostics_contain(items, "undefined: lib.Saved", &other_uri);

    server.shutdown();
}

fn check_direct_disk_file_watch() {
    // Direct-disk edits without any editor notification: file watching picks
    // them up and reloads clean open buffers, so downstream semantics and
    // diagnostics refresh without didChange/didSave/didChangeWatchedFiles.
    let poll_root = TempDir::from_fixture("poll-files-true", "tests/fixtures/watched-files");
    let mut cmd = goquery_command();
    cmd.env(
        "GOMODCACHE",
        std::env::temp_dir().join("goquery-poll-modcache"),
    );
    let mut polled = TestServer::from_command(cmd, true);

    let poll_root_uri = poll_root.dir_uri();
    polled.initialize_ok(json!({
        "processId": std::process::id(),
        "rootUri": poll_root_uri,
        "workspaceFolders": [{"uri": poll_root_uri, "name": "polled"}],
        "capabilities": {"textDocument": {}},
        "initializationOptions": {
            "goroot": "",
            "fileWatch": true,
            "diagnostics": {"onSave": false},
            "fileWatchExcludeDirectories": ["(^|/)generated$", "^cache-[0-9]+$"],
        },
    }));

    let poll_main = poll_root.read("main.go");
    let poll_other = poll_root.read("other/other.go");
    let poll_lib = poll_root.read("lib/lib.go");
    let poll_main_uri = poll_root.uri("main.go");
    let poll_other_uri = poll_root.uri("other/other.go");
    let poll_lib_uri = poll_root.uri("lib/lib.go");
    for (uri, source) in [
        (&poll_main_uri, &poll_main),
        (&poll_other_uri, &poll_other),
        (&poll_lib_uri, &poll_lib),
    ] {
        polled.did_open(uri, source);
    }
    let poll_barrier = polled.document_symbol(&poll_lib_uri);
    assert!(poll_barrier.get("result").is_some(), "{poll_barrier}");

    let poll_renamed = poll_lib.replace(
        "// Value returns the current value.\nfunc Value() int {",
        "// Renamed returns the current value.\nfunc Renamed() int {",
    );
    poll_root.write("lib/lib.go", &poll_renamed);
    let items = polled.wait_diagnostic_containing(&poll_other_uri, "undefined: lib.Value");
    assert_diagnostics_contain(&items, "undefined: lib.Value", &poll_other_uri);

    let poll_changed_other = poll_other.replace("lib.Value()", "lib.Renamed()");
    polled.did_change(&poll_other_uri, 2, &poll_changed_other);
    let reply = def_by_name(&mut polled, &poll_other_uri, &poll_changed_other, "Renamed");
    assert_first_location_uri(&reply, &poll_lib_uri);
    let hover_offset = poll_renamed
        .find("func Renamed")
        .expect("poll renamed declaration")
        + "func ".len();
    let reply = polled.hover(&poll_lib_uri, &poll_renamed, hover_offset);
    assert_hover_contains(&reply, "func Renamed() int", "direct write hover");

    // New files created directly on disk are discovered by the structural walk.
    let poll_extra_path = poll_root.write(
        "lib/extra.go",
        "package lib\n\nfunc Extra() int {\n\treturn 4\n}\n",
    );
    std::thread::sleep(Duration::from_millis(500));
    let poll_extra_uri = file_uri(&poll_extra_path);
    let poll_changed_main = poll_main.replace("lib.Value()", "lib.Extra()");
    polled.did_change(&poll_main_uri, 2, &poll_changed_main);
    let reply = def_by_name(&mut polled, &poll_main_uri, &poll_changed_main, "Extra");
    assert_first_location_uri(&reply, &poll_extra_uri);

    // Deleted indexed files no longer occur in the directory walk. The watcher
    // must still remove their declarations through its missing-path check.
    let reply = polled.workspace_symbol("Extra");
    assert!(
        reply["result"]
            .as_array()
            .is_some_and(|items| items.iter().any(|i| i["location"]["uri"] == poll_extra_uri)),
        "new symbol not indexed: {reply}"
    );
    std::fs::remove_file(&poll_extra_path).expect("delete direct new file");
    polled.wait_workspace_symbol(
        "Extra",
        Duration::from_secs(5),
        |reply| {
            reply.get("result").is_some_and(|result| {
                result.is_null()
                    || result.as_array().is_some_and(|items| {
                        items.iter().all(|i| i["location"]["uri"] != poll_extra_uri)
                    })
            })
        },
        "deleted symbol still indexed",
    );

    // Fresh listings must discover new directories and directory renames too.
    let old_directory = poll_root.join("direct/nested");
    let new_directory = poll_root.join("direct/moved");
    poll_root.write(
        "direct/nested/leaf.go",
        "package leaf\nfunc PollDirectoryLeaf() {}\n",
    );
    let old_uri = file_uri(old_directory.join("leaf.go"));
    for renamed in [false, true] {
        if renamed {
            std::fs::rename(&old_directory, &new_directory).expect("rename polled directory");
        }
        let directory = if renamed {
            &new_directory
        } else {
            &old_directory
        };
        let uri = file_uri(directory.join("leaf.go"));
        // A directory event must register and scan the newly created subtree.
        polled.wait_workspace_symbol(
            "PollDirectoryLeaf",
            Duration::from_secs(2),
            |reply| {
                reply["result"].as_array().is_some_and(|items| {
                    items.iter().any(|i| i["location"]["uri"] == uri)
                        && (!renamed || items.iter().all(|i| i["location"]["uri"] != old_uri))
                })
            },
            "directory change not indexed",
        );
    }

    let appeared_path = poll_root.write(
        "direct/appeared.go",
        "package direct\nfunc PollAppeared() {}\n",
    );
    let appeared_uri = file_uri(&appeared_path);
    polled.wait_workspace_symbol(
        "PollAppeared",
        Duration::from_secs(2),
        |reply| {
            reply["result"]
                .as_array()
                .is_some_and(|items| items.iter().any(|i| i["location"]["uri"] == appeared_uri))
        },
        "new Go source not indexed",
    );

    // Regexes match directory prefixes, including nested paths, but not a
    // similarly named file or a nonmatching sibling. A regular event must
    // not pull excluded new files into reconciliation either.
    let excluded_generated = poll_root.write(
        "direct/generated/deep/skip.go",
        "package skipped\nfunc ExcludedGenerated() {}\n",
    );
    poll_root.write(
        "cache-42/skip.go",
        "package skipped\nfunc ExcludedCache() {}\n",
    );
    poll_root.write(
        "cache-alpha/keep.go",
        "package keep\nfunc IncludedAlpha() {}\n",
    );
    poll_root.write("generated.go", "package main\nfunc IncludedFilename() {}\n");
    poll_root.write(
        "direct/appeared.go",
        "package direct\nfunc PollAppearedAfterExcludes() {}\n",
    );
    for name in [
        "IncludedAlpha",
        "IncludedFilename",
        "PollAppearedAfterExcludes",
    ] {
        polled.wait_workspace_symbol(
            name,
            Duration::from_secs(2),
            |reply| {
                reply["result"]
                    .as_array()
                    .is_some_and(|items| items.iter().any(|i| i["name"] == name))
            },
            "included path did not refresh",
        );
    }
    for name in ["ExcludedGenerated", "ExcludedCache"] {
        let reply = polled.workspace_symbol(name);
        assert!(
            !reply["result"]
                .as_array()
                .is_some_and(|items| items.iter().any(|i| i["name"] == name)),
            "excluded new source was indexed: {reply}"
        );
    }
    let excluded_uri = file_uri(&excluded_generated);
    polled.did_open(&excluded_uri, "package skipped\nfunc EditorExcluded() {}\n");
    let reply = polled.document_symbol(&excluded_uri);
    assert!(
        reply["result"]
            .as_array()
            .is_some_and(|items| items.iter().any(|i| i["name"] == "EditorExcluded")),
        "editor buffer was excluded: {reply}"
    );

    polled.shutdown();
}

fn check_nested_module_and_sema_features() {
    // An opened file without imports still needs the imports of admitted
    // siblings in an unselected nested module, including promoted fields.
    let nested_root = TempDir::new("nested-sibling");
    nested_root.write_files(&[
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
                "func Constrained[T ~int](value T) T { return value }\n",
                "func Pair[A, B any](a A, b B) (first A, second B) { return a, b }\n",
                "func Many[T any](values ...T) []T { return values }\n",
                "type Sequence[T any] func(yield func(T) bool)\nfunc Frames() Sequence[int] { return nil }\nfunc Collect[T any](input Sequence[T]) []T { return nil }\n",
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
    ]);

    let mut cmd = goquery_command();
    cmd.env("GOMODCACHE", nested_root.join("modcache"));
    let mut server = TestServer::from_command(cmd, true);
    let init = server.initialize_ok(json!({
        "rootUri": nested_root.dir_uri(),
        "capabilities": {"textDocument": {"semanticTokens": {
            "requests": {"full": true},
            "formats": ["relative"],
            "tokenModifiers": [],
            "tokenTypes": [
                "namespace", "type", "function", "method", "variable", "property",
                "parameter", "keyword", "string", "number", "comment", "operator", "typeParameter",
            ],
        }}},
        "initializationOptions": {"goroot": "", "fileWatch": false, "semanticTokens": true},
    }));
    let semantic_types =
        init["result"]["capabilities"]["semanticTokensProvider"]["legend"]["tokenTypes"].clone();

    check_generic_hover_and_signatures(&nested_root, &mut server);
    check_format_and_key_highlights(&nested_root, &mut server, &semantic_types);
    check_anonymous_fields_and_declarations(&nested_root, &mut server);
    check_nested_siblings_relations_and_aliases(&nested_root, &mut server);
    check_build_views_and_doc_links(&nested_root, &mut server, &semantic_types);

    server.shutdown();
}

fn check_generic_hover_and_signatures(root: &TempDir, server: &mut TestServer) {
    let generic_uri = root.uri("generic.go");
    let generic_source = root.read("generic.go");
    server.did_open(&generic_uri, &generic_source);

    for name in ["Wrapped", "Explicit"] {
        let reply = server.hover(
            &generic_uri,
            &generic_source,
            generic_source.find(name).unwrap(),
        );
        assert_hover_contains(
            &reply,
            &format!("var {name} func() (int, string)"),
            "generic callable inference failed",
        );
    }
    let formal_use = generic_source.find("value T) T").unwrap() + "value T) ".len();
    let reply = server.hover(&generic_uri, &generic_source, formal_use);
    assert_hover_contains(
        &reply,
        "type parameter T any",
        "return formal resolved to the package-level T",
    );

    let calls = concat!(
        "package client\nimport \"example.test/outer\"\nconst ByteValue = \"\\xff\"\n// Global callable.\nvar GlobalIdentity = outer.Identity[int]\nfunc genericCalls() {\n",
        "_ = outer.Identity[int](1)\n_ = outer.Identity(1)\n",
        "_, _ = outer.Pair[int](1, \"value\")\n_ = outer.Many([]int{1}...)\n",
        "_ = outer.Shape(1)\n// Copied callable.\ncopied := outer.Identity[string]\n_ = copied(\"value\")\n_ = GlobalIdentity(1)\n",
        "zero := func() int { return 1 }\n_ = zero()\n_ = ReturnsCallable()(1)\n",
        "_ = outer.Collect(outer.Frames())\n_ = outer.Collect( func(yield func(int) bool) {})\n",
        "NestedCallback(nil)\nGenericCallback[int](nil,1)\nvar receive <-chan int\nGenericChannel[<-chan int](nil,receive)\nTaggedCallback(nil)\n}\n",
        "func ReturnsCallable() func(value int) int { return GlobalIdentity }\n",
        "func Produce() outer.Sequence[int] { return nil }\nfunc Gather[T any](input outer.Sequence[T]) []T { return nil }\n",
        "func NestedCallback(callback func(first,second func(x,y int)(a,b string))) {}\n",
        "func GenericCallback[T any](callback func(first,second T), value T) {}\n",
        "func GenericChannel[T any](callback func(first,second chan T), value T) {}\n",
        "func TaggedCallback(callback func(first,second struct{Value string `json:\"func(a,b T)\"`; Other string \"doc:\\\"\\u0085\\u2028\\ufeff\\\"\"})) {}\n",
    );
    let calls_uri = root.uri("client/generic.go");
    server.did_open(&calls_uri, calls);

    for (needle, expected) in [
        ("Identity[int]", "func outer.Identity(value int) int"),
        ("Identity(1)", "func outer.Identity(value int) int"),
        (
            "Collect(outer.Frames())",
            "func outer.Collect(input outer.Sequence[int]) []int",
        ),
        (
            "Collect( func(yield",
            "func outer.Collect(input outer.Sequence[int]) []int",
        ),
        (
            "Pair[int]",
            "func outer.Pair(a int, b string) (first int, second string)",
        ),
        ("Many(", "func outer.Many(values ...int) []int"),
        (
            "Shape(",
            "func outer.Shape(value int) struct{T int \"label:\\\"T\\\"\"}",
        ),
        (
            "Identity[string]",
            "func outer.Identity(value string) string // func[T any](value T) T",
        ),
    ] {
        let reply = server.hover(&calls_uri, calls, calls.find(needle).unwrap());
        assert_hover_contains(
            &reply,
            expected,
            "instantiated signature lost native types or declaration names",
        );
    }

    for (needle, expected, doc) in [
        (
            "copied(",
            "var copied func(value string) string",
            "Copied callable.",
        ),
        (
            "GlobalIdentity(",
            "var GlobalIdentity func(value int) int",
            "Global callable.",
        ),
    ] {
        let start = calls.find(needle).unwrap();
        let reply = server.hover(&calls_uri, calls, start);
        assert_hover_contains(
            &reply,
            expected,
            "function value lost its instantiated signature",
        );
        let reply = server.signature_help_invoked(&calls_uri, calls, start + needle.len());
        let expected_sig = expected
            .strip_prefix("var ")
            .unwrap()
            .replace(" func(", "(");
        assert_eq!(
            reply["result"]["signatures"][0]["label"], expected_sig,
            "function value was not callable through its original declaration: {reply}"
        );
        assert_eq!(
            reply["result"]["signatures"][0]["documentation"],
            json!({"kind": "markdown", "value": doc})
        );
    }

    for (needle, label) in [
        ("outer.Identity(1)", "Identity(value int) int"),
        ("outer.Identity[int](1)", "func(value int) int"),
        ("ReturnsCallable()(1)", "func(value int) int"),
    ] {
        let start = calls.find(needle).unwrap() + needle.rfind('(').unwrap() + 1;
        let reply = server.signature_help_invoked(&calls_uri, calls, start);
        assert_eq!(
            reply["result"],
            json!({
                "activeSignature": 0, "activeParameter": 0,
                "signatures": [{"label": label, "activeParameter": 0,
                    "parameters": [{"label": "value int"}],
                    "documentation": {"kind": "markdown", "value": ""}}],
            }),
            "callable signature lost type proof or result names: {reply}"
        );
    }

    for needle in ["outer.Collect(outer.Frames())", "outer.Collect( func(yield"] {
        let start = calls.find(needle).unwrap() + "outer.Collect(".len();
        let reply = server.signature_help_invoked(&calls_uri, calls, start);
        assert_eq!(
            reply["result"]["signatures"][0]["label"], "Collect(input outer.Sequence[int]) []int",
            "named callback inference or presentation failed for {needle}: {reply}"
        );
    }

    let unimported = "package client\nfunc gatherFrames() { _ = Gather( Produce()) }\n";
    let unimported_uri = root.uri("client/iterator.go");
    server.did_open(&unimported_uri, unimported);
    let reply = server.signature_help_invoked(
        &unimported_uri,
        unimported,
        unimported.find("Gather(").unwrap() + "Gather(".len(),
    );
    assert_eq!(
        reply["result"]["signatures"][0]["label"], "Gather(input outer.Sequence[int]) []int",
        "displaying a proven type must not require a consumer import: {reply}"
    );

    for (needle, label, parameters) in [
        (
            "NestedCallback(nil)",
            "NestedCallback(callback func(first func(x int, y int) (a string, b string), second func(x int, y int) (a string, b string)))",
            vec![
                "callback func(first func(x int, y int) (a string, b string), second func(x int, y int) (a string, b string))",
            ],
        ),
        (
            "GenericCallback[int](nil,1)",
            "func(callback func(first int, second int), value int)",
            vec!["callback func(first int, second int)", "value int"],
        ),
        (
            "GenericChannel[<-chan int](nil,receive)",
            "func(callback func(first chan (<-chan int), second chan (<-chan int)), value <-chan int)",
            vec![
                "callback func(first chan (<-chan int), second chan (<-chan int))",
                "value <-chan int",
            ],
        ),
        (
            "TaggedCallback(nil)",
            r#"TaggedCallback(callback func(first struct{Value string "json:\"func(a,b T)\""; Other string "doc:\"\u0085\u2028\ufeff\""}, second struct{Value string "json:\"func(a,b T)\""; Other string "doc:\"\u0085\u2028\ufeff\""}))"#,
            vec![
                r#"callback func(first struct{Value string "json:\"func(a,b T)\""; Other string "doc:\"\u0085\u2028\ufeff\""}, second struct{Value string "json:\"func(a,b T)\""; Other string "doc:\"\u0085\u2028\ufeff\""})"#,
            ],
        ),
    ] {
        let start = calls.find(needle).unwrap() + needle.find('(').unwrap() + 1;
        let reply = server.signature_help_invoked(&calls_uri, calls, start);
        let parameters: Vec<_> = parameters
            .into_iter()
            .map(|l| json!({"label": l}))
            .collect();
        assert_eq!(
            reply["result"],
            json!({
                "activeSignature": 0, "activeParameter": 0,
                "signatures": [{"label": label, "parameters": parameters, "activeParameter": 0,
                    "documentation": {"kind": "markdown", "value": ""}}],
            }),
            "nested signatures lost names, channel identity or literal bytes: {reply}"
        );
    }

    let mut expanded_type = "func(a,b int)".to_owned();
    for _ in 0..12 {
        expanded_type = format!("func(a,b {expanded_type})");
    }
    let bounded_source = format!(
        "package client\nfunc wide(callback {expanded_type}) {{}}\nfunc useWide() {{ wide(nil) }}\n"
    );
    let bounded_uri = root.uri("client/bounded.go");
    server.did_open(&bounded_uri, &bounded_source);
    let reply = server.signature_help_invoked(
        &bounded_uri,
        &bounded_source,
        bounded_source.find("wide(nil)").unwrap() + "wide(".len(),
    );
    assert!(
        reply["result"].is_null(),
        "exponential signature expansion must return unknown on budget exhaustion: {reply}"
    );

    for offset in [0, "zero(".len()] {
        let start = calls.find("zero()").unwrap();
        let reply = server.signature_help_invoked(&calls_uri, calls, start + offset);
        assert_eq!(
            reply["result"],
            json!({
                "activeSignature": 0,
                "signatures": [{"label": "zero() int", "documentation": {"kind": "markdown", "value": ""}}],
            }),
            "zero-parameter signature should omit parameter metadata: {reply}"
        );
    }

    let builtin_source = concat!(
        "package client\nfunc builtinCalls() {\nvalues := []int{1}\n_ = len(values)\n",
        "_ = len(\"constant\")\nvar r float64\n_ = complex(r, r)\n",
        "// Local length.\nlen := func(value int) int { return value }\n_ = len(1)\n}\n",
    );
    let builtin_uri = root.uri("client/builtins.go");

    let mut plaintext_server = TestServer::from_command(goquery_command(), true);
    plaintext_server.initialize_ok(json!({
        "rootUri": root.dir_uri(),
        "capabilities": {"textDocument": {
            "hover": {"contentFormat": ["plaintext", "markdown"]},
            "signatureHelp": {"signatureInformation": {"documentationFormat": ["markdown"]}},
        }},
        "initializationOptions": {"goroot": "", "fileWatch": false},
    }));
    plaintext_server.did_open(&calls_uri, calls);
    let reply = plaintext_server.signature_help_invoked(
        &calls_uri,
        calls,
        calls.find("GlobalIdentity(1)").unwrap() + "GlobalIdentity(".len(),
    );
    assert_eq!(
        reply["result"]["signatures"][0]["documentation"],
        json!({"kind": "plaintext", "value": "Global callable.\n"}),
        "signature documentation ignored hover preference or stripped plaintext newline: {reply}"
    );
    plaintext_server.did_open(&builtin_uri, builtin_source);
    let reply = plaintext_server.signature_help_invoked(
        &builtin_uri,
        builtin_source,
        builtin_source.find("len(values)").unwrap() + "len(".len(),
    );
    assert!(
        reply["result"].is_null(),
        "disabled SDK must not fabricate builtin documentation: {reply}"
    );
    plaintext_server.shutdown();

    let signature_sdk = root.join(".signature-sdk");
    root.write(
        ".signature-sdk/src/builtin/builtin.go",
        concat!(
            "package builtin\ntype Type int\ntype FloatType float32\ntype ComplexType complex64\n",
            "// SDK length documentation.\nfunc len(v Type) int\n",
            "// SDK complex documentation.\nfunc complex(r, i FloatType) ComplexType\n",
        ),
    );
    let mut builtin_server = TestServer::from_command(goquery_command(), true);
    builtin_server.initialize_ok(json!({
        "rootUri": root.dir_uri(),
        "capabilities": {},
        "initializationOptions": {"goroot": signature_sdk, "fileWatch": false},
    }));
    builtin_server.did_open(&builtin_uri, builtin_source);
    for (needle, label, parameters, doc) in [
        (
            "len(values)",
            "len(v Type) int",
            vec!["v Type"],
            "SDK length documentation.",
        ),
        (
            "complex(r, r)",
            "complex(r float64, i float64) complex128",
            vec!["r float64", "i float64"],
            "SDK complex documentation.",
        ),
        (
            "len(1)",
            "len(value int) int",
            vec!["value int"],
            "Local length.",
        ),
    ] {
        let start = builtin_source.find(needle).unwrap() + needle.find('(').unwrap() + 1;
        let reply = builtin_server.signature_help_invoked(&builtin_uri, builtin_source, start);
        let parameters: Vec<_> = parameters
            .into_iter()
            .map(|l| json!({"label": l}))
            .collect();
        assert_eq!(
            reply["result"],
            json!({
                "activeSignature": 0, "activeParameter": 0,
                "signatures": [{"label": label, "parameters": parameters, "activeParameter": 0,
                    "documentation": {"kind": "markdown", "value": doc}}],
            }),
            "SDK signature template, substitution or shadow guard failed: {reply}"
        );
    }
    let reply = builtin_server.signature_help_invoked(
        &builtin_uri,
        builtin_source,
        builtin_source.find("len(\"constant\")").unwrap() + "len(".len(),
    );
    assert!(
        reply["result"].is_null(),
        "constant-folded builtin should have no callable signature: {reply}"
    );
    builtin_server.shutdown();

    let byte_hover = server.hover(&calls_uri, calls, calls.find("ByteValue").unwrap());
    assert_hover_contains(
        &byte_hover,
        r#"const ByteValue untyped string = "\xff""#,
        "byte string rendering replaced invalid UTF-8",
    );

    let invalid_values = concat!(
        "package client\nimport \"example.test/outer\"\nfunc invalidValues() {\n",
        "missing := outer.Pair[int]\nextra := outer.Identity[int, string]\n",
        "constraint := outer.Constrained[string]\n_, _, _ = missing, extra, constraint\n}\n",
    );
    let invalid_values_uri = root.uri("client/invalid_values.go");
    server.did_open(&invalid_values_uri, invalid_values);
    for name in ["missing", "extra", "constraint"] {
        let reply = server.hover(
            &invalid_values_uri,
            invalid_values,
            invalid_values.rfind(name).unwrap(),
        );
        assert!(
            reply["result"].is_null(),
            "invalid type argument arity invented a function type: {reply}"
        );
    }
    server.did_close(&invalid_values_uri);

    let invalid_generic =
        format!("{generic_source}\nfunc Plain() int {{ return 0 }}\nvar Broken = Plain[int]()\n");
    server.did_change(&generic_uri, 2, &invalid_generic);
    let reply = server.hover(
        &generic_uri,
        &invalid_generic,
        invalid_generic.find("Broken").unwrap(),
    );
    assert!(
        reply["result"].is_null(),
        "illegal instantiation acquired a fabricated type: {reply}"
    );
    server.did_change(&generic_uri, 3, &generic_source);

    let local_source = format!(
        "{generic_source}\nfunc localTypes() {{\n\ttype sample struct {{\n\t\tCount int\n\t}}\n\tsamples := make([]sample, 2)\n\tfor _, want := range samples {{ _ = want }}\n\t_ = samples\n\ttype tagged struct {{\n\t\tData string `first\n\t\t\tkeep\nlast`\n\t}}\n}}\n"
    );
    server.did_change(&generic_uri, 4, &local_source);
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
        let reply = server.hover(&generic_uri, &local_source, offset);
        assert_hover_contains(&reply, expected, "local type presentation/identity lost");
    }
}

fn check_format_and_key_highlights(
    root: &TempDir,
    server: &mut TestServer,
    semantic_types: &Value,
) {
    let format_source = concat!(
        "package client\nfunc emit(prefix, format string, args ...any) {}\n",
        "func misleadingf(format string, value any) {}\ntype Named func(string, ...any)\n",
        "func formats(named Named, direct func(string, ...any), value int) {\n",
        "emit(\"%d prefix\", \"%d emit\", value)\nmisleadingf(\"%d ordinary\", value)\n",
        "named(\"%d named\", value)\ndirect(\"%d direct\", value)\ndirect(\"\\x25d escaped\", value)\ndirect(\"\\u0025d unicode\", value)\n",
        "func(format string, args ...any) {}(\"%d literal\", value)\n",
        "{ direct := func(format string, value any) {}; direct(\"%d shadow ordinary\", value); direct(\"%d shadow repeat\", value) }\n",
        "direct(\"%d resumed\", value)\n",
        "emit(\"prefix\", \"%[2]*.[1]*[3]f dynamic\", value, value, 1.5)\n}\n",
        "type LiteralRow struct{ RowField int }\ntype LiteralKey struct{ KeyField int }\n",
        "func tables(index int) {\n_ = [][]LiteralRow{{{RowField: index}, {RowField: index}}, {{RowField: 1}}}\n",
        "_ = map[LiteralKey][]LiteralRow{{KeyField: 1}: {{RowField: 2}}, {KeyField: 3}: {{RowField: 4}}}\n",
        "_ = []map[int]int{{index: 1}, {index: 2}}\n}\n",
        "func anonymousKeys() { _ = struct{ RowField func(int) int }{RowField: func(value int) int { return value }} }\n",
    );
    let format_uri = root.uri("client/format.go");
    server.did_open(&format_uri, format_source);
    let reply = server.semantic_tokens_full(&format_uri);
    let decoded =
        decode_semantic_tokens(reply["result"]["data"].as_array().expect("format tokens"));
    let styles: Vec<(Value, Value)> = decoded
        .iter()
        .map(|&(line, character, _, kind, _)| {
            (
                json!({"line": line, "character": character}),
                semantic_types[kind as usize].clone(),
            )
        })
        .collect();
    let starts: Vec<Value> = styles.iter().map(|(start, _)| start.clone()).collect();

    for (literal, split) in [
        ("%d prefix", false),
        ("%d emit", true),
        ("%d ordinary", false),
        ("%d named", false),
        ("%d direct", true),
        ("\\x25d escaped", true),
        ("\\u0025d unicode", true),
        ("%d literal", true),
        ("%d shadow ordinary", false),
        ("%d shadow repeat", false),
        ("%d resumed", true),
    ] {
        let at = pos(format_source, format_source.find(literal).unwrap());
        assert_eq!(
            starts.contains(&at),
            split,
            "format parameter was guessed for {literal}: {reply}"
        );
    }
    for (needle, kind) in [
        ("RowField:", "property"),
        ("KeyField:", "property"),
        ("index:", "parameter"),
    ] {
        for (offset, _) in format_source.match_indices(needle) {
            let at = pos(format_source, offset);
            assert!(
                styles
                    .iter()
                    .any(|(start, style)| *start == at && style == kind),
                "elided literal mixed field and expression contexts for {needle}: {reply}"
            );
        }
    }
    for (literal, width) in [
        ("%d emit", 2),
        ("%d direct", 2),
        ("%d literal", 2),
        ("%d resumed", 2),
        ("\\x25d escaped", 5),
        ("\\u0025d unicode", 7),
    ] {
        let start = format_source.find(literal).unwrap();
        let line_end = start + format_source[start..].find('\n').unwrap();
        let argument = start + format_source[start..line_end].rfind("value").unwrap();
        let expected = json!([
            {"kind": 3, "range": range(format_source, start, start + width)},
            {"kind": 2, "range": range(format_source, argument, argument + 5)},
        ]);
        for offset in [start, start + width - 1] {
            let reply = server.document_highlight(&format_uri, format_source, offset);
            assert_eq!(
                reply["result"], expected,
                "format cursor lost its argument: {reply}"
            );
        }
        let reply = server.document_highlight(&format_uri, format_source, start + width);
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
        let reply = server.document_highlight(&format_uri, format_source, offset);
        assert_eq!(
            reply["result"],
            json!([
                {"kind": 3, "range": range(format_source, directive, directive + length)},
                {"kind": 2, "range": range(format_source, argument, argument + argument_length)},
            ]),
            "indexed width or argument end boundary was lost: {reply}"
        );
    }

    let key_source = concat!(
        "package client\nvar shared = 1\ntype KeyRow struct{ key int }\n",
        "func keyTables(key int) {\n_ = []map[int]int{{key: shared}, {key: shared}}\n",
        "_ = []KeyRow{{key: key}}\n{ key := 2; _ = []map[int]int{{key: 1}}; _ = []KeyRow{{key: key}} }\n",
        "_ = struct{ shared int }{shared: shared}\n}\n",
        "func switchKeys(input any) {\nswitch key := input.(type) {\ncase int:\n",
        "_ = []map[any]int{{key: 1}, {key: 2}}\n_ = []KeyRow{{key: key}}\n_ = key\n",
        "case string:\n_ = []map[any]int{{key: 3}}\n_ = key\n}\n}\n",
        "func another(input any) { switch key := input.(type) { case int: _ = key } }\n",
    );
    let key_uri = root.uri("client/keys.go");
    server.did_open(&key_uri, key_source);
    let outer = key_source.find("keyTables(key").unwrap() + "keyTables(".len();
    let first_key = key_source.find("{{key: shared}").unwrap() + 2;
    let second_key = key_source.find("{key: shared}}").unwrap() + 1;
    let field_value = key_source.find("{{key: key}}").unwrap() + "{{key: ".len();
    let first_case = key_source.find("{{key: 1}, {key: 2}}").unwrap() + 2;
    let second_case_key = key_source.find("{key: 2}}").unwrap() + 1;
    let case_value =
        first_case + key_source[first_case..].find("{{key: key}}").unwrap() + "{{key: ".len();
    let case_read = case_value + key_source[case_value..].find("_ = key").unwrap() + 4;
    let guards: Vec<_> = key_source
        .match_indices("key := input.(type)")
        .map(|(offset, _)| (offset, 3))
        .collect();
    for (cursor, expected) in [
        (
            outer,
            vec![
                (outer, 1),
                (first_key, 2),
                (second_key, 2),
                (field_value, 2),
            ],
        ),
        (
            first_key,
            vec![
                (outer, 1),
                (first_key, 2),
                (second_key, 2),
                (field_value, 2),
            ],
        ),
        (
            first_case,
            vec![
                (first_case, 2),
                (second_case_key, 2),
                (case_value, 2),
                (case_read, 2),
            ],
        ),
        (guards[0].0, guards.clone()),
    ] {
        let expected: Vec<_> = expected
            .into_iter()
            .map(
                |(start, kind)| json!({"kind": kind, "range": range(key_source, start, start + 3)}),
            )
            .collect();
        let reply = server.document_highlight(&key_uri, key_source, cursor);
        assert_eq!(
            reply["result"],
            json!(expected),
            "literal keys confused variables, fields or switch-clause identities: {reply}"
        );
    }

    let shared = key_source.find("var shared").unwrap() + 4;
    let reply = server.rename(&key_uri, key_source, shared, "renamedShared");
    let changes = reply["result"]["documentChanges"]
        .as_array()
        .expect("shared rename");
    assert_eq!(
        changes.len(),
        1,
        "global rename touched unrelated documents: {reply}"
    );
    assert_eq!(changes[0]["textDocument"]["uri"], key_uri.as_str());
    let mut edits = changes[0]["edits"].as_array().unwrap().clone();
    let mut offsets = vec![shared];
    offsets.extend(
        key_source
            .match_indices("key: shared")
            .map(|(offset, _)| offset + 5),
    );
    offsets.push(key_source.find("{shared: shared}").unwrap() + "{shared: ".len());
    let mut expected: Vec<_> = offsets
        .into_iter()
        .map(|start| json!({"newText": "renamedShared", "range": range(key_source, start, start + 6)}))
        .collect();
    edits.sort_by_key(Value::to_string);
    expected.sort_by_key(Value::to_string);
    assert_eq!(
        edits, expected,
        "anonymous field labels were renamed as package variables: {reply}"
    );
}

fn check_anonymous_fields_and_declarations(root: &TempDir, server: &mut TestServer) {
    let anonymous_source = concat!(
        "package client\nvar Anonymous = struct{ Field int; Other int }{Field: 1}\n",
        "func anonymousUse() {\nvalue := struct{ Field int }{Field: 2}\ncopied := value\n_ = copied.Field\n_ = Anonymous.Field\n}\n",
    );
    let anonymous_other = "package client\nfunc elsewhere() { _ = Anonymous.Field }\n";
    let anonymous_uri = root.uri("client/anonymous.go");
    let anonymous_other_uri = root.uri("client/anonymous_other.go");
    server.did_open(&anonymous_uri, anonymous_source);
    server.did_open(&anonymous_other_uri, anonymous_other);

    let global_field = anonymous_source.find("struct{ Field").unwrap() + "struct{ ".len();
    let local_field =
        anonymous_source.find("value := struct{ Field").unwrap() + "value := struct{ ".len();
    let copied = anonymous_source.find("copied.Field").unwrap() + "copied.".len();
    for (cursor, declaration, expected) in [
        (
            global_field,
            global_field,
            vec![
                (&anonymous_uri, anonymous_source, global_field),
                (
                    &anonymous_uri,
                    anonymous_source,
                    anonymous_source.find("{Field: 1}").unwrap() + 1,
                ),
                (
                    &anonymous_uri,
                    anonymous_source,
                    anonymous_source.find("Anonymous.Field").unwrap() + "Anonymous.".len(),
                ),
                (
                    &anonymous_other_uri,
                    anonymous_other,
                    anonymous_other.find("Anonymous.Field").unwrap() + "Anonymous.".len(),
                ),
            ],
        ),
        (
            copied,
            local_field,
            vec![
                (&anonymous_uri, anonymous_source, local_field),
                (
                    &anonymous_uri,
                    anonymous_source,
                    anonymous_source.find("{Field: 2}").unwrap() + 1,
                ),
                (&anonymous_uri, anonymous_source, copied),
            ],
        ),
    ] {
        for include_declaration in [false, true] {
            let mut expected_locations: Vec<_> = expected
                .iter()
                .filter(|(uri, _, start)| {
                    include_declaration || **uri != anonymous_uri || *start != declaration
                })
                .map(|(uri, source, start)| {
                    json!({"uri": uri, "range": range(source, *start, start + 5)})
                })
                .collect();
            let reply = server.references(
                &anonymous_uri,
                anonymous_source,
                cursor,
                include_declaration,
            );
            let mut actual = reply["result"]
                .as_array()
                .expect("anonymous-field references")
                .clone();
            actual.sort_by_key(Value::to_string);
            expected_locations.sort_by_key(Value::to_string);
            assert_eq!(
                actual, expected_locations,
                "references and rename disagreed on field identity: {reply}"
            );
        }
        let reply = server.rename(&anonymous_uri, anonymous_source, cursor, "RenamedField");
        let changes = reply["result"]["documentChanges"]
            .as_array()
            .expect("anonymous-field rename");
        let mut edits: Vec<_> = changes
            .iter()
            .flat_map(|change| {
                change["edits"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|edit| json!({"uri": change["textDocument"]["uri"], "edit": edit}))
            })
            .collect();
        let mut expected_edits: Vec<_> = expected
            .into_iter()
            .map(|(uri, source, start)| {
                json!({
                    "uri": uri,
                    "edit": {"newText": "RenamedField", "range": range(source, start, start + 5)},
                })
            })
            .collect();
        edits.sort_by_key(Value::to_string);
        expected_edits.sort_by_key(Value::to_string);
        assert_eq!(
            edits, expected_edits,
            "anonymous fields merged distinct declarations or lost references: {reply}"
        );
    }

    let other_field = anonymous_source.find("Other int").unwrap();
    for include_declaration in [false, true] {
        let reply = server.references(
            &anonymous_uri,
            anonymous_source,
            other_field,
            include_declaration,
        );
        let expected = if include_declaration {
            json!([{"uri": anonymous_uri, "range": range(anonymous_source, other_field, other_field + 5)}])
        } else {
            json!([])
        };
        assert_eq!(
            reply["result"], expected,
            "unused field did not preserve declaration/empty-list semantics: {reply}"
        );
    }
    let reply = server.rename(&anonymous_uri, anonymous_source, global_field, "Other");
    assert_eq!(
        reply["error"]["code"], 0,
        "field conflict produced edits: {reply}"
    );
    assert!(
        reply["error"]["message"]
            .as_str()
            .is_some_and(|text| text.contains("would conflict with this field")),
        "field conflict was not identified: {reply}"
    );

    let declarations = concat!(
        "package client\ntype InlineData struct{ LongName int; X string }\n",
        "type RawTag struct{ Value string `first\n// keep literal text\n\nlast` }\n",
        "type AliasData = InlineData\nvar _ InlineData\nvar _ RawTag\nvar _ AliasData\n",
        "func localDeclaration() { type LocalRaw struct{ Value string `first\n// keep local text\nlast` }; var _ LocalRaw }\n",
        "var LiteralValue struct { A, B string `struct { keep; } [0x10] func(a, b int)\ntwo  spaces` }\n",
        "func LiteralResult() struct { S string `interface { keep; }` } { panic(\"unused\") }\n",
        "var _ = LiteralValue\nvar _ = LiteralResult\n",
    );
    let declarations_uri = root.uri("client/declarations.go");
    server.did_open(&declarations_uri, declarations);
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
        (
            "LiteralValue",
            r#"var LiteralValue struct{A string "struct { keep; } [0x10] func(a, b int)\ntwo  spaces"; B string "struct { keep; } [0x10] func(a, b int)\ntwo  spaces"}"#,
        ),
        (
            "LiteralResult",
            r#"func LiteralResult() struct{S string "interface { keep; }"}"#,
        ),
    ] {
        let reply = server.hover(
            &declarations_uri,
            declarations,
            declarations.rfind(name).unwrap(),
        );
        assert_hover_contains(
            &reply,
            expected,
            "declaration formatting changed a name, literal or field layout",
        );
    }
}

fn check_nested_siblings_relations_and_aliases(root: &TempDir, server: &mut TestServer) {
    let nested_uri = root.uri("child/p/open.go");
    let nested_source = "package p\nvar Item Carrier\nvar Other CarrierAlias\n";
    server.did_open(&nested_uri, nested_source);
    let reply = server.hover(
        &nested_uri,
        nested_source,
        nested_source.find("Carrier").unwrap(),
    );
    let text = hover_text(&reply);
    assert!(
        text.contains("Exported") && text.contains("through Inner"),
        "missing sibling import promotion: {reply}"
    );
    assert!(
        text.contains("Hello()"),
        "exported method through private embedding was lost: {reply}"
    );
    assert!(
        !text.lines().any(|line| {
            let mut words = line.split_whitespace();
            words.next() == Some("Inner") && words.next() == Some("int")
        }),
        "direct embedding did not shadow the nested same-name field: {reply}"
    );
    let reply = server.hover(
        &nested_uri,
        nested_source,
        nested_source.find("CarrierAlias").unwrap(),
    );
    assert_hover_contains(&reply, "Hello()", "alias lost its underlying method set");

    let reply = server.workspace_symbol("Inner");
    assert!(
        !reply["result"]
            .as_array()
            .is_some_and(|items| items.iter().any(|item| item["name"] == "Inner")),
        "nested view leaked into main symbols: {reply}"
    );

    let relations = "package outer\ntype DataOnly interface { Data() any }\ntype WithError interface { error; DataOnly }\ntype PlainData struct{}\nfunc (PlainData) Data() any { return nil }\ntype ErrorData struct{}\nfunc (*ErrorData) Data() any { return nil }\nfunc (*ErrorData) Error() string { return \"\" }\ntype WrongData struct{}\nfunc (WrongData) Data() any { return nil }\nfunc (WrongData) Error() int { return 0 }\ntype Embedded struct { DataOnly }\ntype PointerEmbedded struct { *PlainData }\ntype GenericField[T any] struct{}\ntype GenericEmbedded struct { GenericField[int] }\nfunc useEmbedded(e Embedded) { _ = e.DataOnly }\n";
    let relations_uri = root.uri("relations.go");
    server.did_open(&relations_uri, relations);
    let reply = server.implementation(
        &relations_uri,
        relations,
        relations.find("WithError").unwrap(),
    );
    let rows = reply["result"]
        .as_array()
        .unwrap_or_else(|| panic!("implementation result: {reply}"));
    for (name, satisfies) in [
        ("PlainData", false),
        ("ErrorData", true),
        ("WrongData", false),
    ] {
        let required = pos(relations, relations.find(name).unwrap());
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
        let reply = server.document_highlight(
            &relations_uri,
            relations,
            relations.find(needle).unwrap() + relative,
        );
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
        let reply = server.implementation(
            &relations_uri,
            relations,
            relations.find(needle).unwrap() + relative,
        );
        let expected = pos(relations, relations.find(expected).unwrap());
        assert_has_location(
            &reply,
            &relations_uri,
            &expected,
            "implementation cursor should follow the embedded type",
        );
    }

    let arrays = "package p\nimport \"example.test/child/dep\"\nconst ( Slot = 2 + iota; NextSlot )\nvar Sparse = [...]int{dep.LastIndex: 10, 20, 1: 30}\nvar Grouped = [...]byte{NextSlot: 1}\nvar Empty = [...]int{}\nvar Fraction = [...]int{1.0: 1}\nvar Reverse = [...]int{7: 1, 2: 2, 3}\ntype SameLength [len(Sparse)]byte\nfunc locals() { const top = 5; values := [...]int{top: 1, 2}; _ = values; type item struct { N int }; local := [...]item{{}}; _ = local }\n";
    let arrays_uri = root.uri("child/p/arrays.go");
    server.did_open(&arrays_uri, arrays);
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
        let reply = server.hover(&arrays_uri, arrays, arrays.find(name).unwrap());
        assert_hover_contains(
            &reply,
            expected,
            "array extent or lexical identity was lost",
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
        server.did_change(&arrays_uri, version as i64 + 2, &source);
        let reply = server.hover(&arrays_uri, &source, source.find("Broken").unwrap());
        assert!(
            reply["result"].is_null(),
            "invalid/unavailable extent fell back to a fabricated type: {reply}"
        );
    }

    let aliases = "package p\ntype Pair[T comparable] = [2]T\ntype Box[T any] struct { Value T }\ntype BoxAlias[T comparable] = Box[T]\nfunc First[T comparable](p Pair[T]) T { return p[0] }\nvar IntPair Pair[int]\nvar FirstEl = IntPair[0]\nvar FirstInt = First(IntPair)\nfunc useAlias() {\n\tfirstEl := IntPair[0]\n\tvar b BoxAlias[string]\n\t_ = firstEl\n\t_ = b.Value\n\t_ = b\n\t_ = FirstInt\n\t_ = FirstEl\n}\n";
    let aliases_uri = root.uri("child/p/aliases.go");
    server.did_open(&aliases_uri, aliases);
    for (needle, expected) in [
        ("IntPair", "Pair[int]"),
        ("FirstInt", "var FirstInt int"),
        ("firstEl :=", "var firstEl int"),
        ("b.Value", "field Value string"),
    ] {
        let offset = aliases.rfind(needle).unwrap() + needle.rfind('.').map_or(0, |dot| dot + 1);
        let reply = server.hover(&aliases_uri, aliases, offset);
        assert_hover_contains(
            &reply,
            expected,
            &format!("generic type alias hover mismatch for {needle}"),
        );
    }
    let def_reply = server.definition(&aliases_uri, aliases, aliases.find("Pair[int]").unwrap());
    let expected_pair = pos(aliases, aliases.find("Pair[T comparable]").unwrap());
    assert_has_location(
        &def_reply,
        &aliases_uri,
        &expected_pair,
        "generic type alias definition mismatch",
    );
    let tdef_reply = server.type_definition(
        &aliases_uri,
        aliases,
        aliases.rfind("_ = b").unwrap() + "_ = ".len(),
    );
    let expected_box_alias = pos(aliases, aliases.find("BoxAlias[T comparable]").unwrap());
    assert_has_location(
        &tdef_reply,
        &aliases_uri,
        &expected_box_alias,
        "generic type alias typeDefinition mismatch",
    );
}

fn check_build_views_and_doc_links(
    root: &TempDir,
    server: &mut TestServer,
    semantic_types: &Value,
) {
    for (filename, test_methods, private_methods) in [
        ("view.go", false, false),
        ("view_test.go", true, true),
        ("external_view_test.go", true, false),
        ("client/view.go", false, false),
        ("dotclient/view.go", false, false),
    ] {
        let text = root.read(filename);
        let uri = root.uri(filename);
        server.did_open(&uri, &text);
        for name in ["ViewItem", "ViewWrapper"] {
            let reply = server.hover(&uri, &text, text.rfind(name).unwrap());
            let content = hover_text(&reply);
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
        let source = root.read(filename);
        let uri = root.uri(filename);
        let reply = server.semantic_tokens_full(&uri);
        let data = reply["result"]["data"]
            .as_array()
            .unwrap_or_else(|| panic!("missing semantic tokens: {reply}"));
        let reference = format!("[{needle}]");
        let offset =
            source.find(&reference).unwrap() + 1 + needle.rfind('.').map_or(0, |off| off + 1);
        let expected = pos(&source, offset);
        let mut found = None;
        for (line, character, _, token_kind, _) in decode_semantic_tokens(data) {
            if expected == json!({"line": line, "character": character}) {
                found = semantic_types[token_kind as usize].as_str();
            }
        }
        assert_eq!(
            found, kind,
            "wrong doc member token for {filename}/{needle}: {reply}"
        );
    }
}

fn check_watch_limits_and_invalid_regex() {
    // Invalid regex input must fail initialization rather than silently leave
    // the directory watched or exclude everything.
    let mut invalid_exclude = TestServer::from_command(goquery_command(), true);
    let reply = invalid_exclude.request(
        "initialize",
        json!({
            "capabilities": {},
            "initializationOptions": {"goroot": "", "fileWatchExcludeDirectories": ["["]},
        }),
    );
    assert_eq!(reply["error"]["code"], -32602, "{reply}");
    assert!(
        reply["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("fileWatchExcludeDirectories")),
        "{reply}"
    );
    invalid_exclude.exit_only();

    // File watch budgets: an explicit low limit logs an actionable error and
    // never falls back to polling. Raised/unlimited budgets cover both roots.
    let limits_root = TempDir::new("watch-limits");
    let folders: Vec<_> = ["first", "second"]
        .into_iter()
        .map(|name| {
            let directory = limits_root.join(name);
            limits_root.write(
                format!("{name}/go.mod"),
                format!("module example.test/{name}\n\ngo 1.27.3\n"),
            );
            json!({"uri": dir_uri(directory), "name": name})
        })
        .collect();
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
            limits_root.write(
                format!("{name}/file.go"),
                "package budget\nfunc BeforeBudgetEdit() {}\n",
            );
        }
        let mut cmd = goquery_command();
        cmd.stderr(Stdio::piped())
            .env("GOQUERY_POLL_MS", "100")
            .env("GOMODCACHE", limits_root.join("modcache"));
        let mut budgeted = TestServer::from_command(cmd, true);
        budgeted.initialize_ok(json!({
            "workspaceFolders": folders,
            "capabilities": {},
            "initializationOptions": {
                "goroot": "",
                "fileWatch": true,
                "pollIntervalMs": 100,
                "fileWatchMaxRoots": roots,
                "fileWatchMaxDirectories": directories,
                "fileWatchMaxEntries": entries,
            },
        }));
        // Complete initial reconciliation before testing a fresh event. Legacy
        // polling settings above must not enable periodic or on-demand scans.
        let reply = budgeted.workspace_symbol("BeforeBudgetEdit");
        assert_eq!(reply["result"].as_array().map(Vec::len), Some(2), "{reply}");
        std::thread::sleep(Duration::from_millis(250));
        let symbol = format!("AfterBudgetEdit{case}");
        limits_root.write(
            "second/file.go",
            format!("package budget\nfunc {symbol}() {{}}\n"),
        );
        if exceeded.is_some() {
            std::thread::sleep(Duration::from_millis(1200));
            let reply = budgeted.workspace_symbol(&symbol);
            assert!(
                !reply["result"]
                    .as_array()
                    .is_some_and(|items| items.iter().any(|item| item["name"] == symbol)),
                "unexpected polling fallback: {reply}"
            );
        } else {
            budgeted.wait_workspace_symbol(
                &symbol,
                Duration::from_secs(2),
                |reply| {
                    reply["result"]
                        .as_array()
                        .is_some_and(|items| items.iter().any(|item| item["name"] == symbol))
                },
                &format!("watch budget case {case} did not refresh"),
            );
        }
        let log = budgeted.shutdown_with_stderr();
        if let Some(key) = exceeded {
            assert!(log.contains(&format!("{key}=1 exceeded")), "{log}");
        } else {
            assert!(!log.contains("file watching unavailable"), "{log}");
        }
    }
}

fn check_pipelined_registration() {
    // Clients like Zed pipeline didOpen and document queries immediately after
    // `initialized` before replying to `client/registerCapability`. Dynamic
    // registration and folder re-registration must not hold the concurrency
    // slot while waiting on the client response.
    let pipelined_root = TempDir::from_fixture("pipelined-watch", "tests/fixtures/watched-files");
    let mut cmd = goquery_command();
    cmd.env(
        "GOMODCACHE",
        std::env::temp_dir().join("goquery-pipelined-modcache"),
    );
    let mut pipelined = TestServer::from_command(cmd, false);

    let pipelined_root_uri = pipelined_root.dir_uri();
    let reply = pipelined.request(
        "initialize",
        json!({
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
    let pipelined_main = pipelined_root.read("main.go");
    let pipelined_main_uri = pipelined_root.uri("main.go");
    pipelined.notify("initialized", json!({}));
    pipelined.did_open(&pipelined_main_uri, &pipelined_main);
    let reply = pipelined.document_symbol(&pipelined_main_uri);
    assert!(reply["result"].is_array(), "{reply}");
    let registration = pipelined.wait_request("client/registerCapability");
    pipelined.reply(&registration);

    pipelined.notify(
        "workspace/didChangeWorkspaceFolders",
        json!({"event": {"added": [{"uri": pipelined_root_uri, "name": "pipelined-again"}], "removed": []}}),
    );
    let reply = pipelined.document_symbol(&pipelined_main_uri);
    assert!(reply["result"].is_array(), "{reply}");
    let unregistration = pipelined.wait_request("client/unregisterCapability");
    pipelined.reply(&unregistration);
    let re_registration = pipelined.wait_request("client/registerCapability");
    pipelined.reply(&re_registration);

    pipelined.shutdown();
}
