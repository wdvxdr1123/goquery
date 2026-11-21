use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

/// Bounded Content-Length framing for the test client (the server itself uses
/// tower-lsp's codec). Keeping it local removes a standalone workspace crate.
pub mod jsonrpc2 {
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

    pub fn frame(value: Value) -> Vec<u8> {
        let mut bytes = Vec::new();
        send(&mut bytes, value).expect("frame LSP message");
        bytes
    }
}

pub fn file_uri(path: impl AsRef<Path>) -> String {
    url::Url::from_file_path(path.as_ref())
        .unwrap_or_else(|_| panic!("invalid file URI path: {}", path.as_ref().display()))
        .to_string()
}

pub fn dir_uri(path: impl AsRef<Path>) -> String {
    url::Url::from_directory_path(path.as_ref())
        .unwrap_or_else(|_| panic!("invalid dir URI path: {}", path.as_ref().display()))
        .to_string()
}

pub fn pos(source: &str, offset: usize) -> Value {
    let prefix = source.get(..offset).expect("valid utf-8 offset");
    json!({
        "line": prefix.matches('\n').count(),
        "character": prefix.rsplit('\n').next().unwrap_or_default().encode_utf16().count(),
    })
}

pub fn range(source: &str, start: usize, end: usize) -> Value {
    json!({"start": pos(source, start), "end": pos(source, end)})
}

pub fn range_of(source: &str, needle: &str) -> Value {
    let start = source
        .find(needle)
        .unwrap_or_else(|| panic!("needle {needle:?} not found"));
    range(source, start, start + needle.len())
}

pub fn location(uri: &str, source: &str, start: usize, len: usize) -> Value {
    json!({"uri": uri, "range": range(source, start, start + len)})
}

pub fn locations(uri: &str, source: &str, start: usize, len: usize) -> Value {
    json!([location(uri, source, start, len)])
}

pub fn copy_tree(from: &Path, to: &Path) {
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

pub fn write_files(root: &Path, files: &[(&str, &str)]) {
    std::fs::create_dir_all(root).expect("create workspace root");
    for (rel, content) in files {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create parent dir");
        }
        std::fs::write(&path, content).expect("write fixture file");
    }
}

pub fn decode_semantic_tokens(data: &[Value]) -> Vec<(u32, u32, u32, u32, u32)> {
    assert_eq!(data.len() % 5, 0, "encoded tokens must come in 5-tuples");
    let mut out = Vec::with_capacity(data.len() / 5);
    let (mut line, mut start) = (0u32, 0u32);
    for chunk in data.as_chunks::<5>().0 {
        let d_line = chunk[0].as_u64().expect("deltaLine") as u32;
        let d_start = chunk[1].as_u64().expect("deltaStart") as u32;
        let length = chunk[2].as_u64().expect("length") as u32;
        let kind = chunk[3].as_u64().expect("tokenType") as u32;
        let mods = chunk[4].as_u64().expect("tokenModifiers") as u32;
        line += d_line;
        start = if d_line == 0 {
            start + d_start
        } else {
            d_start
        };
        out.push((line, start, length, kind, mods));
    }
    out
}

pub fn hover_text(reply: &Value) -> &str {
    reply["result"]["contents"]["value"]
        .as_str()
        .unwrap_or_default()
}

pub fn assert_hover_contains(reply: &Value, expected: &str, context: &str) {
    assert!(
        hover_text(reply).contains(expected),
        "{context}: expected {expected:?} in hover reply: {reply}"
    );
}

pub fn assert_first_location_uri(reply: &Value, expected_uri: &str) {
    let result = reply["result"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        result.first().and_then(|item| item["uri"].as_str()),
        Some(expected_uri),
        "{reply}"
    );
}

pub fn assert_has_location(reply: &Value, uri: &str, start: &Value, context: &str) {
    assert!(
        reply["result"].as_array().is_some_and(|rows| {
            rows.iter()
                .any(|row| row["uri"].as_str() == Some(uri) && row["range"]["start"] == *start)
        }),
        "{context}: {reply}"
    );
}

pub fn assert_diagnostics_contain(items: &[Value], needle: &str, context: &str) {
    assert!(
        items.iter().any(|item| {
            item["message"]
                .as_str()
                .is_some_and(|message| message.contains(needle))
        }),
        "{context}: missing {needle:?} in {items:?}"
    );
}

pub fn completion_item(
    label: &str,
    kind: u64,
    detail: Option<&str>,
    insert_format: u64,
    new_text: &str,
    edit_range: Value,
    rank: usize,
) -> Value {
    let mut item = json!({
        "label": label,
        "kind": kind,
        "filterText": label,
        "insertTextFormat": insert_format,
        "textEdit": {"range": edit_range, "newText": new_text},
        "sortText": format!("{rank:05}"),
    });
    if rank == 0 {
        item["preselect"] = true.into();
    }
    if let Some(detail) = detail {
        item["detail"] = detail.into();
    }
    item
}

pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    pub fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("goquery-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create temp workspace");
        Self { path }
    }

    pub fn from_fixture(name: &str, fixture_rel: &str) -> Self {
        let dir = Self::new(name);
        let base = Path::new(env!("CARGO_MANIFEST_DIR"));
        copy_tree(&base.join(fixture_rel), &dir.path);
        dir
    }

    pub fn join(&self, rel: impl AsRef<Path>) -> PathBuf {
        self.path.join(rel)
    }

    pub fn uri(&self, rel: impl AsRef<Path>) -> String {
        file_uri(self.path.join(rel))
    }

    pub fn dir_uri(&self) -> String {
        dir_uri(&self.path)
    }

    pub fn write(&self, rel: impl AsRef<Path>, content: impl AsRef<[u8]>) -> PathBuf {
        let path = self.path.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create parent dir");
        }
        std::fs::write(&path, content).expect("write temp file");
        path
    }

    pub fn write_files(&self, files: &[(&str, &str)]) {
        write_files(&self.path, files);
    }

    pub fn read(&self, rel: impl AsRef<Path>) -> String {
        std::fs::read_to_string(self.path.join(rel)).expect("read temp file")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

pub fn goquery_command() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_goquery"));
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .env("GOWORK", "off")
        .env_remove("GOQUERY_DISABLE_DIAG")
        .env_remove("GOQUERY_TRACE_QUERIES");
    cmd
}

pub struct TestServer {
    root: Option<PathBuf>,
    owns_root: bool,
    child: Option<Child>,
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    messages: mpsc::Receiver<Value>,
    requests: Arc<Mutex<Vec<Value>>>,
    next_id: i64,
}

impl TestServer {
    pub fn spawn(mut child: Child, auto_reply: bool) -> Self {
        let stdin = Arc::new(Mutex::new(Some(child.stdin.take().expect("server stdin"))));
        let stdout = child.stdout.take().expect("server stdout");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let (sender, messages) = mpsc::channel();
        let reader_stdin = stdin.clone();
        let reader_requests = requests.clone();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
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
                        let response = json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": Value::Null,
                        });
                        if let Some(input) = reader_stdin.lock().expect("stdin lock").as_mut() {
                            let _ = jsonrpc2::send(input, response);
                        }
                    }
                } else {
                    let _ = sender.send(message);
                }
            }
        });
        Self {
            root: None,
            owns_root: false,
            child: Some(child),
            stdin,
            messages,
            requests,
            next_id: 0,
        }
    }

    pub fn from_command(mut cmd: Command, auto_reply: bool) -> Self {
        let child = cmd.spawn().expect("spawn goquery");
        Self::spawn(child, auto_reply)
    }

    pub fn start(name: &str, files: &[(&str, &str)]) -> Self {
        Self::start_in_target(name, files, None)
    }

    pub fn start_in_target(
        name: &str,
        files: &[(&str, &str)],
        target: Option<(&str, &str)>,
    ) -> Self {
        Self::start_configured(name, files, target, false)
    }

    pub fn start_configured(
        name: &str,
        files: &[(&str, &str)],
        target: Option<(&str, &str)>,
        snippets: bool,
    ) -> Self {
        Self::start_with_sdk(name, files, target, snippets, None, ".")
    }

    pub fn start_with_sdk(
        name: &str,
        files: &[(&str, &str)],
        target: Option<(&str, &str)>,
        snippets: bool,
        sdk: Option<&str>,
        workspace: &str,
    ) -> Self {
        Self::start_session(name, files, workspace, snippets, |root| {
            json!({
                "goroot": sdk.map_or_else(PathBuf::new, |sdk| root.join(sdk)),
                "fileWatch": false,
                "diagnostics": {"onSave": false},
                "usePlaceholders": snippets,
                "goos": target.map(|(goos, _)| goos),
                "goarch": target.map(|(_, goarch)| goarch),
            })
        })
    }

    pub fn start_session(
        name: &str,
        files: &[(&str, &str)],
        workspace: &str,
        snippets: bool,
        options: impl FnOnce(&Path) -> Value,
    ) -> Self {
        let root = std::env::temp_dir().join(format!("goquery-sema-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        write_files(&root, files);
        let mut cmd = goquery_command();
        cmd.env("GOMODCACHE", root.join("_gomodcache"));
        let mut server = Self::from_command(cmd, true);
        server.root = Some(root.clone());
        server.owns_root = true;
        let root_uri = dir_uri(root.join(workspace));
        let initialization_options = options(&root);
        let capabilities = if snippets {
            json!({"textDocument": {"completion": {"completionItem": {"snippetSupport": true}}}})
        } else {
            json!({})
        };
        let init = server.request(
            "initialize",
            json!({
                "processId": std::process::id(),
                "rootUri": root_uri,
                "capabilities": capabilities,
                "initializationOptions": initialization_options,
            }),
        );
        assert!(init.get("result").is_some(), "initialize failed: {init}");
        server.notify("initialized", json!({}));
        server
    }

    pub fn root(&self) -> &Path {
        self.root.as_deref().expect("server has a workspace root")
    }

    pub fn uri(&self, rel: &str) -> String {
        file_uri(self.root().join(rel))
    }

    pub fn send(&self, message: &Value) {
        let mut guard = self.stdin.lock().expect("stdin lock");
        let input = guard.as_mut().expect("server stdin active");
        jsonrpc2::send(input, message.clone()).expect("send LSP message");
    }

    pub fn send_raw(&self, bytes: &[u8]) {
        let mut guard = self.stdin.lock().expect("stdin lock");
        let input = guard.as_mut().expect("server stdin active");
        input.write_all(bytes).expect("write raw LSP burst");
        input.flush().expect("flush raw LSP burst");
    }

    pub fn recv_message(&self) -> Value {
        self.messages
            .recv_timeout(Duration::from_secs(30))
            .expect("timeout waiting for LSP message")
    }

    pub fn reply(&self, request: &Value) {
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": request["id"],
            "result": Value::Null,
        }));
    }

    pub fn notify(&self, method: &str, params: Value) {
        self.send(&json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }));
    }

    pub fn request(&mut self, method: &str, params: Value) -> Value {
        self.request_observing(method, params, |_| {})
    }

    pub fn request_before_diagnostics(&mut self, method: &str, params: Value, uri: &str) -> Value {
        self.request_observing(method, params, |message| {
            assert!(
                message["method"] != "textDocument/publishDiagnostics"
                    || message["params"]["uri"] != uri,
                "dependency diagnostics arrived before the follow-up request: {message}"
            );
        })
    }

    pub fn request_observing(
        &mut self,
        method: &str,
        params: Value,
        mut observe: impl FnMut(&Value),
    ) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        let mut message = json!({
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
            let incoming = self
                .messages
                .recv_timeout(remaining)
                .unwrap_or_else(|_| panic!("timeout waiting for {method}"));
            observe(&incoming);
            if incoming.get("id") == Some(&json!(id)) && incoming.get("method").is_none() {
                return incoming;
            }
        }
    }

    pub fn initialize_ok(&mut self, params: Value) -> Value {
        let reply = self.request("initialize", params);
        assert!(reply.get("result").is_some(), "initialize failed: {reply}");
        self.notify("initialized", json!({}));
        reply
    }

    pub fn did_open(&self, uri: &str, text: &str) {
        self.notify(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": "go",
                    "version": 1,
                    "text": text,
                },
            }),
        );
    }

    pub fn did_change(&self, uri: &str, version: i64, text: &str) {
        self.notify(
            "textDocument/didChange",
            json!({
                "textDocument": {"uri": uri, "version": version},
                "contentChanges": [{"text": text}],
            }),
        );
    }

    pub fn did_save(&self, uri: &str) {
        self.notify(
            "textDocument/didSave",
            json!({"textDocument": {"uri": uri}}),
        );
    }

    pub fn did_close(&self, uri: &str) {
        self.notify(
            "textDocument/didClose",
            json!({"textDocument": {"uri": uri}}),
        );
    }

    pub fn did_change_watched_files(&self, changes: &[(&str, u32)]) {
        let changes: Vec<_> = changes
            .iter()
            .map(|(uri, kind)| json!({"uri": uri, "type": kind}))
            .collect();
        self.notify(
            "workspace/didChangeWatchedFiles",
            json!({"changes": changes}),
        );
    }

    pub fn text_doc_pos(&mut self, method: &str, uri: &str, source: &str, offset: usize) -> Value {
        self.request(
            method,
            json!({"textDocument": {"uri": uri}, "position": pos(source, offset)}),
        )
    }

    pub fn hover(&mut self, uri: &str, source: &str, offset: usize) -> Value {
        self.text_doc_pos("textDocument/hover", uri, source, offset)
    }

    pub fn definition(&mut self, uri: &str, source: &str, offset: usize) -> Value {
        self.text_doc_pos("textDocument/definition", uri, source, offset)
    }

    pub fn type_definition(&mut self, uri: &str, source: &str, offset: usize) -> Value {
        self.text_doc_pos("textDocument/typeDefinition", uri, source, offset)
    }

    pub fn implementation(&mut self, uri: &str, source: &str, offset: usize) -> Value {
        self.text_doc_pos("textDocument/implementation", uri, source, offset)
    }

    pub fn document_highlight(&mut self, uri: &str, source: &str, offset: usize) -> Value {
        self.text_doc_pos("textDocument/documentHighlight", uri, source, offset)
    }

    pub fn completion(&mut self, uri: &str, source: &str, offset: usize) -> Value {
        self.text_doc_pos("textDocument/completion", uri, source, offset)
    }

    pub fn signature_help(&mut self, uri: &str, source: &str, offset: usize) -> Value {
        self.text_doc_pos("textDocument/signatureHelp", uri, source, offset)
    }

    pub fn signature_help_invoked(&mut self, uri: &str, source: &str, offset: usize) -> Value {
        self.request(
            "textDocument/signatureHelp",
            json!({
                "textDocument": {"uri": uri},
                "position": pos(source, offset),
                "context": {"triggerKind": 1, "isRetrigger": false},
            }),
        )
    }

    pub fn references(
        &mut self,
        uri: &str,
        source: &str,
        offset: usize,
        include_declaration: bool,
    ) -> Value {
        self.request(
            "textDocument/references",
            json!({
                "textDocument": {"uri": uri},
                "position": pos(source, offset),
                "context": {"includeDeclaration": include_declaration},
            }),
        )
    }

    pub fn rename(&mut self, uri: &str, source: &str, offset: usize, new_name: &str) -> Value {
        self.request(
            "textDocument/rename",
            json!({
                "textDocument": {"uri": uri},
                "position": pos(source, offset),
                "newName": new_name,
            }),
        )
    }

    pub fn document_symbol(&mut self, uri: &str) -> Value {
        self.request(
            "textDocument/documentSymbol",
            json!({"textDocument": {"uri": uri}}),
        )
    }

    pub fn code_lens(&mut self, uri: &str) -> Value {
        self.request(
            "textDocument/codeLens",
            json!({"textDocument": {"uri": uri}}),
        )
    }

    pub fn semantic_tokens_full(&mut self, uri: &str) -> Value {
        self.request(
            "textDocument/semanticTokens/full",
            json!({"textDocument": {"uri": uri}}),
        )
    }

    pub fn semantic_tokens_range(
        &mut self,
        uri: &str,
        source: &str,
        start: usize,
        end: usize,
    ) -> Value {
        self.request(
            "textDocument/semanticTokens/range",
            json!({"textDocument": {"uri": uri}, "range": range(source, start, end)}),
        )
    }

    pub fn workspace_symbol(&mut self, query: &str) -> Value {
        self.request("workspace/symbol", json!({"query": query}))
    }

    pub fn wait_workspace_symbol(
        &mut self,
        query: &str,
        timeout: Duration,
        predicate: impl Fn(&Value) -> bool,
        context: &str,
    ) {
        let deadline = Instant::now() + timeout;
        loop {
            let reply = self.workspace_symbol(query);
            if predicate(&reply) {
                return;
            }
            assert!(Instant::now() < deadline, "{context}: {reply}");
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    pub fn wait_request(&self, method: &str) -> Value {
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

    pub fn wait_registration(&self) -> Value {
        self.wait_request("client/registerCapability")["params"].clone()
    }

    pub fn wait_diagnostics(&self, expected: &[&str]) -> HashMap<String, Vec<Value>> {
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut found = HashMap::new();
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
    pub fn wait_diagnostic_containing(&self, uri: &str, needle: &str) -> Vec<Value> {
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

    pub fn published_diagnostics(&self, uri: &str, version: i64) -> Vec<Value> {
        let deadline = Instant::now() + Duration::from_secs(15);
        for _ in 0..64 {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let message = self
                .messages
                .recv_timeout(remaining)
                .expect("server closed or timed out before diagnostics");
            if message["method"] == "textDocument/publishDiagnostics"
                && message["params"]["uri"] == uri
                && message["params"]["version"] == version
            {
                return message["params"]["diagnostics"]
                    .as_array()
                    .expect("diagnostics array")
                    .clone();
            }
        }
        panic!("diagnostic notification budget exceeded");
    }

    fn wait_exit(&mut self) -> Option<std::process::ChildStderr> {
        let _ = self.stdin.lock().expect("stdin lock").take();
        let mut child = self.child.take()?;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if child.try_wait().expect("wait goquery").is_some() {
                let stderr = child.stderr.take();
                let _ = child.wait();
                return stderr;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("test server did not exit");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    pub fn shutdown(mut self) {
        let reply = self.request("shutdown", Value::Null);
        assert!(reply.get("result").is_some(), "shutdown failed: {reply}");
        self.notify("exit", Value::Null);
        self.wait_exit();
    }

    pub fn shutdown_with_stderr(mut self) -> String {
        let reply = self.request("shutdown", Value::Null);
        assert!(reply.get("result").is_some(), "shutdown failed: {reply}");
        self.notify("exit", Value::Null);
        let mut log = String::new();
        self.wait_exit()
            .expect("piped stderr")
            .read_to_string(&mut log)
            .expect("read stderr");
        log
    }

    pub fn exit_only(mut self) {
        self.notify("exit", Value::Null);
        self.wait_exit();
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.stdin.lock() {
            let _ = guard.take();
        }
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if self.owns_root
            && let Some(root) = self.root.take()
        {
            let _ = std::fs::remove_dir_all(&root);
        }
    }
}
