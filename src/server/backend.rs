//! Typed tower-lsp endpoints delegate to the existing bounded semantic queries.
use super::capabilities;
use super::transport::ProcessStatus;
use crate::features::{self, CallDirection, DocumentQuery, Query, TypeDirection};
use crate::workspace::{Workspace, WorkspaceEvent};
use futures::lock::Mutex as AsyncMutex;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::path::Path;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tower_lsp::LanguageServer;
use tower_lsp::jsonrpc::{Error, ErrorCode, Result};
use tower_lsp::lsp_types as lsp;
use tower_lsp::lsp_types::request::{
    GotoImplementationParams, GotoImplementationResponse, GotoTypeDefinitionParams,
    GotoTypeDefinitionResponse,
};

const WATCHED_FILES_REGISTRATION_ID: &str = "goquery-watched-files";

pub(super) struct Backend {
    client: tower_lsp::Client,
    workspace: Arc<Mutex<Workspace>>,
    status: Arc<ProcessStatus>,
    /// Sampled once: GOQUERY_DISABLE_DIAG keeps all push diagnostics off.
    diagnostics_enabled: bool,
    /// Whether `workspace/didChangeWatchedFiles` was registered successfully.
    /// Guarded by an async mutex so background registration and folder-change
    /// re-registration run in order without holding the request concurrency slot.
    watched_files_registered: Arc<AsyncMutex<bool>>,
}

type DiagnosticUpdate = (url::Url, Vec<lsp::Diagnostic>, Option<i32>);

fn diagnostics_values(
    workspace: &mut Workspace,
    files: &[PathBuf],
    external_sync: bool,
) -> Vec<DiagnosticUpdate> {
    let mut updates = Vec::with_capacity(files.len());
    for file in files {
        let Ok(value) = features::execute(
            workspace,
            Query::Document {
                file,
                query: DocumentQuery::Diagnostics,
            },
        ) else {
            continue;
        };
        let Ok(diagnostics) = decode::<Vec<lsp::Diagnostic>>(value) else {
            continue;
        };
        let Ok(uri) = url::Url::from_file_path(file) else {
            continue;
        };
        updates.push((
            uri,
            diagnostics,
            workspace.publish_version(file, external_sync),
        ));
    }
    updates
}

fn watched_files_registration() -> lsp::Registration {
    let watchers = ["**/*.go", "**/go.mod", "**/go.sum", "**/go.work"]
        .into_iter()
        .map(|pattern| lsp::FileSystemWatcher {
            glob_pattern: lsp::GlobPattern::String(pattern.into()),
            kind: None,
        })
        .collect();
    lsp::Registration {
        id: WATCHED_FILES_REGISTRATION_ID.into(),
        method: "workspace/didChangeWatchedFiles".into(),
        register_options: serde_json::to_value(lsp::DidChangeWatchedFilesRegistrationOptions {
            watchers,
        })
        .ok(),
    }
}

async fn register_watched_files(client: &tower_lsp::Client) -> bool {
    if let Err(error) = client
        .register_capability(vec![watched_files_registration()])
        .await
    {
        eprintln!("file watch registration: {error}");
        return false;
    }
    true
}

fn error((code, message): (i32, String)) -> Error {
    Error {
        code: ErrorCode::from(i64::from(code)),
        message: message.into(),
        data: None,
    }
}

fn encode(params: impl Serialize) -> Result<Value> {
    serde_json::to_value(params).map_err(|_| Error::internal_error())
}

fn decode<R: DeserializeOwned>(value: Value) -> Result<R> {
    serde_json::from_value(value).map_err(|_| Error::internal_error())
}

impl Backend {
    pub(super) fn new(client: tower_lsp::Client, status: Arc<ProcessStatus>) -> Self {
        Self {
            client,
            workspace: Arc::new(Mutex::new(Workspace::default())),
            status,
            diagnostics_enabled: std::env::var_os("GOQUERY_DISABLE_DIAG").is_none(),
            watched_files_registered: Arc::new(AsyncMutex::new(false)),
        }
    }

    /// Reconcile only after a filesystem notification. Notify retains one permit;
    /// the callback retains one dirty flag, so bursts cannot build a path queue.
    fn start_file_watcher(&self) {
        let signal = match self.workspace.lock() {
            Ok(mut workspace) => {
                if !workspace.file_watch_enabled() {
                    return;
                }
                workspace.refresh_file_watch(false);
                workspace.file_watch_signal()
            }
            Err(_) => return,
        };
        let workspace = Arc::clone(&self.workspace);
        let client = self.client.clone();
        let diagnostics_enabled = self.diagnostics_enabled;
        tokio::spawn(async move {
            loop {
                signal.notified().await;
                // One debounce per event burst; idle workspaces have no timer.
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                let updates = match workspace.lock() {
                    Ok(mut workspace) => {
                        if !workspace.take_file_changes() {
                            continue;
                        }
                        let changed = workspace.disk_changes();
                        let affected = if changed.is_empty() {
                            Vec::new()
                        } else {
                            let confirm = changed.clone();
                            let affected =
                                match workspace.apply(WorkspaceEvent::DiskChanges(changed)) {
                                    Ok(outcome) => outcome.diagnostics,
                                    Err((_, message)) => {
                                        eprintln!("file reconciliation: {message}");
                                        Vec::new()
                                    }
                                };
                            workspace.confirm_disk_paths(&confirm);
                            affected
                        };
                        workspace.refresh_file_watch(true);
                        if diagnostics_enabled {
                            diagnostics_values(&mut workspace, &affected, true)
                        } else {
                            Vec::new()
                        }
                    }
                    Err(_) => break,
                };
                for (uri, diagnostics, version) in updates {
                    let _ = client.publish_diagnostics(uri, diagnostics, version).await;
                }
            }
        });
    }

    fn watched_files_dynamic(&self) -> bool {
        self.workspace
            .lock()
            .map(|workspace| workspace.watched_files_dynamic)
            .unwrap_or(false)
    }

    /// Queue initial file-watch registration off the notification handler.
    /// `ConcurrencyLimit(..., 1)` pauses stdin admission while a handler is in
    /// flight, so awaiting `client/registerCapability` inside `initialized`
    /// deadlocks clients (such as Zed) that send `didOpen` or queries before
    /// replying to the server request.
    fn spawn_register_watched_files(&self) {
        let client = self.client.clone();
        let registered = Arc::clone(&self.watched_files_registered);
        tokio::spawn(async move {
            let mut registered = registered.lock().await;
            if *registered {
                return;
            }
            *registered = register_watched_files(&client).await;
        });
    }

    /// Clients may scope a file-watch registration to the workspace folders that
    /// existed when it was created. After folders change, replace the
    /// registration off the notification handler so newly added folders are
    /// watched too without blocking stdin.
    fn spawn_re_register_watched_files(&self) {
        let client = self.client.clone();
        let registered = Arc::clone(&self.watched_files_registered);
        tokio::spawn(async move {
            let mut registered = registered.lock().await;
            if *registered {
                let unregistration = lsp::Unregistration {
                    id: WATCHED_FILES_REGISTRATION_ID.into(),
                    method: "workspace/didChangeWatchedFiles".into(),
                };
                if let Err(error) = client.unregister_capability(vec![unregistration]).await {
                    eprintln!("file watch unregistration: {error}");
                }
            }
            *registered = register_watched_files(&client).await;
        });
    }

    fn with_workspace<R>(
        &self,
        run: impl FnOnce(&mut Workspace) -> std::result::Result<R, (i32, String)>,
    ) -> Result<R> {
        let mut workspace = self.workspace.lock().map_err(|_| Error::internal_error())?;
        run(&mut workspace).map_err(error)
    }

    fn document<P: Serialize, R: DeserializeOwned>(
        &self,
        params: P,
        query: for<'a> fn(&'a Value) -> DocumentQuery<'a>,
    ) -> Result<R> {
        let params = encode(params)?;
        let file = crate::workspace::path(&params["textDocument"]["uri"])
            .ok_or_else(|| Error::invalid_params("expected file URI"))?;
        decode(self.with_workspace(|workspace| {
            features::execute(
                workspace,
                Query::Document {
                    file: &file,
                    query: query(&params),
                },
            )
        })?)
    }

    fn relation<P: Serialize, R: DeserializeOwned>(
        &self,
        params: P,
        query: for<'a> fn(&'a Value) -> Query<'a>,
    ) -> Result<R> {
        let params = encode(params)?;
        decode(self.with_workspace(|workspace| features::execute(workspace, query(&params)))?)
    }

    /// Apply one workspace mutation and return the open documents whose
    /// diagnostics may now be stale. The mutation owns index/module updates.
    fn update(&self, event: WorkspaceEvent<'_>) -> Vec<PathBuf> {
        let Ok(mut workspace) = self.workspace.lock() else {
            return Vec::new();
        };
        let outcome = workspace.apply(event);
        workspace.refresh_file_watch(false);
        match outcome {
            Ok(outcome) => outcome.diagnostics,
            Err((_, message)) => {
                eprintln!("workspace notification: {message}");
                Vec::new()
            }
        }
    }

    /// Publish query-local diagnostics for the affected open documents. Values
    /// are computed under the workspace lock, then published after the lock is
    /// released so a slow client notification never blocks the next request.
    fn refresh_diagnostics(&self, files: Vec<PathBuf>) {
        if !self.diagnostics_enabled || files.is_empty() {
            return;
        }
        let computed =
            self.with_workspace(|workspace| Ok(diagnostics_values(workspace, &files, false)));
        let Ok(updates) = computed else {
            return;
        };
        if updates.is_empty() {
            return;
        }
        let client = self.client.clone();
        tokio::spawn(async move {
            for (uri, diagnostics, version) in updates {
                let _ = client.publish_diagnostics(uri, diagnostics, version).await;
            }
        });
    }

    fn set_version(&self, file: &Path, version: Option<i32>) {
        let _ = self.with_workspace(|workspace| {
            workspace.set_document_version(file, version);
            Ok(())
        });
    }

    pub(super) async fn embedding_supertypes(&self, params: Value) -> Result<Value> {
        self.with_workspace(|workspace| {
            features::execute(
                workspace,
                Query::Embedding(TypeDirection::Supertypes, &params),
            )
        })
    }

    pub(super) async fn embedding_subtypes(&self, params: Value) -> Result<Value> {
        self.with_workspace(|workspace| {
            features::execute(
                workspace,
                Query::Embedding(TypeDirection::Subtypes, &params),
            )
        })
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, params: lsp::InitializeParams) -> Result<lsp::InitializeResult> {
        let params = encode(params)?;
        self.with_workspace(|workspace| {
            workspace.initialize(&params)?;
            Ok(capabilities::initialize_result(&workspace.semantic_config))
        })
    }

    async fn initialized(&self, _: lsp::InitializedParams) {
        // File watching observes direct edits independently of dynamic
        // client registration, without scheduled or request-triggered scans.
        self.start_file_watcher();
        if !self.watched_files_dynamic() {
            eprintln!(
                "file watching: client does not support dynamic registration; external edits are \
                 handled by file watching (fileWatch); disk polling is disabled"
            );
            return;
        }
        self.spawn_register_watched_files();
    }

    async fn shutdown(&self) -> Result<()> {
        self.status.shutdown();
        Ok(())
    }

    async fn did_open(&self, params: lsp::DidOpenTextDocumentParams) {
        if let Ok(file) = params.text_document.uri.to_file_path() {
            self.set_version(&file, Some(params.text_document.version));
            let affected = self.update(WorkspaceEvent::SetBuffer {
                file: file.clone(),
                source: &params.text_document.text,
                dirty: false,
            });
            self.refresh_diagnostics(affected);
        }
    }

    async fn did_change(&self, params: lsp::DidChangeTextDocumentParams) {
        if let Ok(file) = params.text_document.uri.to_file_path()
            && let Some(change) = params.content_changes.last()
        {
            self.set_version(&file, Some(params.text_document.version));
            let affected = self.update(WorkspaceEvent::SetBuffer {
                file: file.clone(),
                source: &change.text,
                dirty: true,
            });
            self.refresh_diagnostics(affected);
        }
    }

    async fn did_close(&self, params: lsp::DidCloseTextDocumentParams) {
        if let Ok(file) = params.text_document.uri.to_file_path() {
            self.set_version(&file, None);
            let affected = self.update(WorkspaceEvent::Close(file.clone()));
            // Clear diagnostics without awaiting the client notification inside
            // the notification handler; long open/close bursts otherwise kill
            // the process (observed as a silent -1 exit on Windows).
            let client = self.client.clone();
            let uri = params.text_document.uri;
            tokio::spawn(async move {
                let _ = client.publish_diagnostics(uri, Vec::new(), None).await;
            });
            self.refresh_diagnostics(affected);
        }
    }

    async fn did_save(&self, params: lsp::DidSaveTextDocumentParams) {
        if let Ok(file) = params.text_document.uri.to_file_path() {
            let affected = self.update(WorkspaceEvent::Save(file));
            self.refresh_diagnostics(affected);
        }
    }

    async fn did_change_workspace_folders(&self, params: lsp::DidChangeWorkspaceFoldersParams) {
        let added: Vec<_> = params
            .event
            .added
            .iter()
            .filter_map(|folder| folder.uri.to_file_path().ok())
            .collect();
        let removed: Vec<_> = params
            .event
            .removed
            .iter()
            .filter_map(|folder| folder.uri.to_file_path().ok())
            .collect();
        if let Err(error) = self.with_workspace(|workspace| {
            workspace.apply_folder_changes(added, removed)?;
            workspace.refresh_file_watch(false);
            Ok(())
        }) {
            eprintln!("workspace folders: {}", error.message);
            return;
        }
        if self.watched_files_dynamic() {
            self.spawn_re_register_watched_files();
        }
    }

    async fn did_change_watched_files(&self, params: lsp::DidChangeWatchedFilesParams) {
        let files = params
            .changes
            .into_iter()
            .filter_map(|change| change.uri.to_file_path().ok())
            .collect();
        let affected = self.update(WorkspaceEvent::WatchedFiles(files));
        self.refresh_diagnostics(affected);
    }

    async fn goto_definition(
        &self,
        params: lsp::GotoDefinitionParams,
    ) -> Result<Option<lsp::GotoDefinitionResponse>> {
        self.document(params, |params| DocumentQuery::Definition(params))
    }

    async fn goto_declaration(
        &self,
        params: lsp::request::GotoDeclarationParams,
    ) -> Result<Option<lsp::request::GotoDeclarationResponse>> {
        self.document(params, |params| DocumentQuery::Declaration(params))
    }

    async fn goto_implementation(
        &self,
        params: GotoImplementationParams,
    ) -> Result<Option<GotoImplementationResponse>> {
        self.document(params, |params| DocumentQuery::Implementation(params))
    }

    async fn hover(&self, params: lsp::HoverParams) -> Result<Option<lsp::Hover>> {
        self.document(params, |params| DocumentQuery::Hover(params))
    }

    async fn document_highlight(
        &self,
        params: lsp::DocumentHighlightParams,
    ) -> Result<Option<Vec<lsp::DocumentHighlight>>> {
        self.document(params, |params| DocumentQuery::DocumentHighlight(params))
    }

    async fn folding_range(
        &self,
        params: lsp::FoldingRangeParams,
    ) -> Result<Option<Vec<lsp::FoldingRange>>> {
        self.document(params, |_| DocumentQuery::FoldingRange)
    }

    async fn code_lens(&self, params: lsp::CodeLensParams) -> Result<Option<Vec<lsp::CodeLens>>> {
        self.document(params, |_| DocumentQuery::CodeLens)
    }

    async fn formatting(
        &self,
        params: lsp::DocumentFormattingParams,
    ) -> Result<Option<Vec<lsp::TextEdit>>> {
        self.document(params, |params| DocumentQuery::Formatting(params))
    }

    async fn document_link(
        &self,
        params: lsp::DocumentLinkParams,
    ) -> Result<Option<Vec<lsp::DocumentLink>>> {
        // tower-lsp fixes this return type to its own lsp-types `DocumentLink`.
        // Its upstream `target` is `Option<Url>`, which rejects gopls targets such
        // as `https://::1` and normalizes host case/IDN/trailing slashes; the
        // workspace patch keeps the exact wire string (`Option<String>`).
        self.document(params, |_| DocumentQuery::DocumentLink)
    }

    async fn code_action(
        &self,
        params: lsp::CodeActionParams,
    ) -> Result<Option<lsp::CodeActionResponse>> {
        self.document(params, |params| DocumentQuery::CodeAction(params))
    }

    async fn prepare_rename(
        &self,
        params: lsp::TextDocumentPositionParams,
    ) -> Result<Option<lsp::PrepareRenameResponse>> {
        self.document(params, |params| DocumentQuery::PrepareRename(params))
    }

    async fn rename(&self, params: lsp::RenameParams) -> Result<Option<lsp::WorkspaceEdit>> {
        self.document(params, |params| DocumentQuery::Rename(params))
    }

    async fn goto_type_definition(
        &self,
        params: GotoTypeDefinitionParams,
    ) -> Result<Option<GotoTypeDefinitionResponse>> {
        self.document(params, |params| DocumentQuery::TypeDefinition(params))
    }

    async fn references(&self, params: lsp::ReferenceParams) -> Result<Option<Vec<lsp::Location>>> {
        self.document(params, |params| DocumentQuery::References(params))
    }

    async fn completion(
        &self,
        params: lsp::CompletionParams,
    ) -> Result<Option<lsp::CompletionResponse>> {
        self.document(params, |params| DocumentQuery::Completion(params))
    }

    async fn signature_help(
        &self,
        params: lsp::SignatureHelpParams,
    ) -> Result<Option<lsp::SignatureHelp>> {
        self.document(params, |params| DocumentQuery::SignatureHelp(params))
    }

    async fn document_symbol(
        &self,
        params: lsp::DocumentSymbolParams,
    ) -> Result<Option<lsp::DocumentSymbolResponse>> {
        self.document(params, |_| DocumentQuery::Symbols)
    }

    async fn semantic_tokens_full(
        &self,
        params: lsp::SemanticTokensParams,
    ) -> Result<Option<lsp::SemanticTokensResult>> {
        // We only return complete tokens. Decoding the untagged result enum
        // first builds a second serde Content array for the entire token stream.
        let tokens: Option<lsp::SemanticTokens> =
            self.document(params, |_| DocumentQuery::SemanticTokens)?;
        Ok(tokens.map(lsp::SemanticTokensResult::Tokens))
    }

    async fn semantic_tokens_range(
        &self,
        params: lsp::SemanticTokensRangeParams,
    ) -> Result<Option<lsp::SemanticTokensRangeResult>> {
        let tokens: Option<lsp::SemanticTokens> =
            self.document(params, |params| DocumentQuery::SemanticTokensRange(params))?;
        Ok(tokens.map(lsp::SemanticTokensRangeResult::Tokens))
    }

    async fn symbol(
        &self,
        params: lsp::WorkspaceSymbolParams,
    ) -> Result<Option<Vec<lsp::SymbolInformation>>> {
        self.relation(params, |params| Query::WorkspaceSymbols(params))
    }

    async fn prepare_call_hierarchy(
        &self,
        params: lsp::CallHierarchyPrepareParams,
    ) -> Result<Option<Vec<lsp::CallHierarchyItem>>> {
        self.document(params, |params| DocumentQuery::PrepareCallHierarchy(params))
    }

    async fn incoming_calls(
        &self,
        params: lsp::CallHierarchyIncomingCallsParams,
    ) -> Result<Option<Vec<lsp::CallHierarchyIncomingCall>>> {
        self.relation(params, |params| {
            Query::Calls(CallDirection::Incoming, params)
        })
    }

    async fn outgoing_calls(
        &self,
        params: lsp::CallHierarchyOutgoingCallsParams,
    ) -> Result<Option<Vec<lsp::CallHierarchyOutgoingCall>>> {
        self.relation(params, |params| {
            Query::Calls(CallDirection::Outgoing, params)
        })
    }

    async fn prepare_type_hierarchy(
        &self,
        params: lsp::TypeHierarchyPrepareParams,
    ) -> Result<Option<Vec<lsp::TypeHierarchyItem>>> {
        self.document(params, |params| DocumentQuery::PrepareTypeHierarchy(params))
    }

    async fn supertypes(
        &self,
        params: lsp::TypeHierarchySupertypesParams,
    ) -> Result<Option<Vec<lsp::TypeHierarchyItem>>> {
        self.relation(params, |params| {
            Query::Types(TypeDirection::Supertypes, params)
        })
    }

    async fn subtypes(
        &self,
        params: lsp::TypeHierarchySubtypesParams,
    ) -> Result<Option<Vec<lsp::TypeHierarchyItem>>> {
        self.relation(params, |params| {
            Query::Types(TypeDirection::Subtypes, params)
        })
    }
}
