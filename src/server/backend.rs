//! Typed tower-lsp endpoints delegate to the existing bounded semantic queries.
use super::capabilities;
use super::scheduling::CANCELLATION;
use super::transport::ProcessStatus;
use super::worker::Worker;
use crate::features::{self, CallDirection, DocumentQuery, Query, TypeDirection};
use crate::workspace::{Workspace, WorkspaceEvent};
use futures::lock::Mutex as AsyncMutex;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Semaphore;
use tokio::time::Instant;
use tower_lsp::LanguageServer;
use tower_lsp::jsonrpc::{Error, ErrorCode, Result};
use tower_lsp::lsp_types as lsp;
use tower_lsp::lsp_types::request::{
    GotoImplementationParams, GotoImplementationResponse, GotoTypeDefinitionParams,
    GotoTypeDefinitionResponse,
};

const WATCHED_FILES_REGISTRATION_ID: &str = "goquery-watched-files";
const MAX_PENDING_DEPENDENCY_DIAGNOSTICS: usize = 8;
const DEPENDENCY_OPEN_DIAGNOSTIC_DELAY: Duration = Duration::from_secs(1);

pub(super) struct Backend {
    client: tower_lsp::Client,
    workspace: Worker,
    status: Arc<ProcessStatus>,
    /// Sampled once: GOQUERY_DISABLE_DIAG keeps all push diagnostics off.
    diagnostics_enabled: bool,
    dependency_diagnostic_slots: Arc<Semaphore>,
    last_query_finished: Arc<Mutex<Instant>>,
    /// Whether `workspace/didChangeWatchedFiles` was registered successfully.
    /// Serialize background registration and folder-change re-registration.
    watched_files_registered: Arc<AsyncMutex<bool>>,
}

type DiagnosticUpdate = (url::Url, Vec<lsp::Diagnostic>, Option<i32>);

#[derive(Clone, Copy, PartialEq, Eq)]
enum DiagnosticTrigger {
    Change,
    Save,
    DiskChange,
}

fn diagnostics_values(
    workspace: &mut Workspace,
    files: &[PathBuf],
    trigger: DiagnosticTrigger,
) -> Vec<DiagnosticUpdate> {
    if workspace.diagnostics_on_save && trigger != DiagnosticTrigger::Save {
        return Vec::new();
    }
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
            workspace.publish_version(file, trigger == DiagnosticTrigger::DiskChange),
        ));
    }
    updates
}

async fn publish_diagnostics(client: tower_lsp::Client, updates: Vec<DiagnosticUpdate>) {
    for (uri, diagnostics, version) in updates {
        let _ = client.publish_diagnostics(uri, diagnostics, version).await;
    }
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
    pub(super) fn new(
        client: tower_lsp::Client,
        status: Arc<ProcessStatus>,
        worker: Worker,
    ) -> Self {
        Self {
            client,
            workspace: worker,
            status,
            diagnostics_enabled: std::env::var_os("GOQUERY_DISABLE_DIAG").is_none(),
            dependency_diagnostic_slots: Arc::new(Semaphore::new(
                MAX_PENDING_DEPENDENCY_DIAGNOSTICS,
            )),
            last_query_finished: Arc::new(Mutex::new(Instant::now())),
            watched_files_registered: Arc::new(AsyncMutex::new(false)),
        }
    }

    /// Reconcile only after a filesystem notification. Notify retains one permit;
    /// the callback retains one dirty flag, so bursts cannot build a path queue.
    async fn start_file_watcher(&self) {
        let Ok(Some(signal)) = self
            .workspace
            .run(|workspace| {
                if !workspace.file_watch_enabled() {
                    return None;
                }
                workspace.refresh_file_watch(false);
                Some(workspace.file_watch_signal())
            })
            .await
        else {
            return;
        };
        let workspace = self.workspace.clone();
        let client = self.client.clone();
        let diagnostics_enabled = self.diagnostics_enabled;
        tokio::spawn(async move {
            loop {
                signal.notified().await;
                // One debounce per event burst; idle workspaces have no timer.
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                let updates = workspace
                    .run(move |workspace| {
                        if !workspace.take_file_changes() {
                            return Vec::new();
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
                            diagnostics_values(workspace, &affected, DiagnosticTrigger::DiskChange)
                        } else {
                            Vec::new()
                        }
                    })
                    .await;
                let Ok(updates) = updates else {
                    break;
                };
                publish_diagnostics(client.clone(), updates).await;
            }
        });
    }

    async fn watched_files_dynamic(&self) -> bool {
        self.workspace
            .run(|workspace| workspace.watched_files_dynamic)
            .await
            .unwrap_or(false)
    }

    /// Queue initial file-watch registration off the notification handler.
    /// A client can send requests before replying to `client/registerCapability`.
    /// Keep the registration off the FIFO handler to avoid blocking that burst.
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

    async fn with_workspace<R: Send + 'static>(
        &self,
        run: impl FnOnce(&mut Workspace) -> std::result::Result<R, (i32, String)> + Send + 'static,
    ) -> Result<R> {
        self.workspace.run(run).await?.map_err(error)
    }

    async fn document<P: Serialize, R: DeserializeOwned + Send + 'static>(
        &self,
        params: P,
        query: for<'a> fn(&'a Value) -> DocumentQuery<'a>,
    ) -> Result<R> {
        let params = encode(params)?;
        let file = crate::workspace::path(&params["textDocument"]["uri"])
            .ok_or_else(|| Error::invalid_params("expected file URI"))?;
        self.execute_query(move |workspace, cancellation| {
            features::execute_cancellable(
                workspace,
                Query::Document {
                    file: &file,
                    query: query(&params),
                },
                cancellation,
            )
        })
        .await
    }

    async fn relation<P: Serialize, R: DeserializeOwned + Send + 'static>(
        &self,
        params: P,
        query: for<'a> fn(&'a Value) -> Query<'a>,
    ) -> Result<R> {
        let params = encode(params)?;
        self.execute_query(move |workspace, cancellation| {
            features::execute_cancellable(workspace, query(&params), cancellation)
        })
        .await
    }

    async fn execute_query<R: DeserializeOwned + Send + 'static>(
        &self,
        query: impl FnOnce(
            &mut Workspace,
            Option<&AtomicBool>,
        ) -> std::result::Result<Value, (i32, String)>
        + Send
        + 'static,
    ) -> Result<R> {
        let cancellation = CANCELLATION.try_with(Clone::clone).ok().flatten();
        crate::error::check_cancelled(cancellation.as_deref()).map_err(error)?;
        let worker_flag = cancellation.clone();
        let finished = self.last_query_finished.clone();
        let reply = self
            .workspace
            .run(move |workspace| {
                crate::error::check_cancelled(worker_flag.as_deref()).map_err(error)?;
                let reply = query(workspace, worker_flag.as_deref())
                    .map_err(error)
                    .and_then(decode);
                if let Ok(mut finished) = finished.lock() {
                    *finished = Instant::now();
                }
                reply
            })
            .await?;
        crate::error::check_cancelled(cancellation.as_deref()).map_err(error)?;
        reply
    }

    /// Apply one workspace mutation and return the open documents whose
    /// diagnostics may now be stale. The mutation owns index/module updates.
    fn update(workspace: &mut Workspace, event: WorkspaceEvent) -> Vec<PathBuf> {
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

    /// Compute diagnostics on the worker; publish without holding its queue.
    async fn refresh_diagnostics(&self, files: Vec<PathBuf>, trigger: DiagnosticTrigger) {
        if !self.diagnostics_enabled || files.is_empty() {
            return;
        }
        let computed = self
            .workspace
            .run(move |workspace| diagnostics_values(workspace, &files, trigger))
            .await;
        let Ok(updates) = computed else {
            return;
        };
        if updates.is_empty() {
            return;
        }
        tokio::spawn(publish_diagnostics(self.client.clone(), updates));
    }

    async fn refresh_open_diagnostics(&self, file: PathBuf, version: i32, affected: Vec<PathBuf>) {
        if !self.diagnostics_enabled || affected.is_empty() {
            return;
        }
        let check_file = file.clone();
        let Ok((on_save, mounted_dependency)) = self
            .workspace
            .run(move |workspace| {
                (
                    workspace.diagnostics_on_save,
                    workspace.is_mounted_dependency_file(&check_file),
                )
            })
            .await
        else {
            return;
        };
        if on_save {
            return;
        }
        // A jump into a mounted dependency should admit the source immediately.
        // Run its initial diagnostics after follow-up requests have had a chance
        // to enter the sequential server. Multi-file invalidations are immediate.
        if affected.as_slice() == [file.as_path()]
            && mounted_dependency
            && let Ok(slot) = self.dependency_diagnostic_slots.clone().try_acquire_owned()
        {
            let workspace = self.workspace.clone();
            let client = self.client.clone();
            let last_query_finished = Arc::clone(&self.last_query_finished);
            tokio::spawn(async move {
                let _slot = slot;
                let mut deadline = Instant::now() + DEPENDENCY_OPEN_DIAGNOSTIC_DELAY;
                let updates = loop {
                    tokio::time::sleep_until(deadline).await;
                    let file = file.clone();
                    let last_query_finished = last_query_finished.clone();
                    let next = workspace
                        .run(move |workspace| {
                            if workspace.document_version(&file) != Some(version)
                                || workspace.open_source(&file).is_none()
                            {
                                return None;
                            }
                            let idle_at = *last_query_finished.lock().ok()?
                                + DEPENDENCY_OPEN_DIAGNOSTIC_DELAY;
                            if idle_at > Instant::now() {
                                return Some(Err(idle_at));
                            }
                            Some(Ok(diagnostics_values(
                                workspace,
                                &[file],
                                DiagnosticTrigger::Change,
                            )))
                        })
                        .await;
                    match next {
                        Ok(Some(Ok(updates))) => break updates,
                        Ok(Some(Err(idle_at))) => deadline = idle_at,
                        _ => return,
                    }
                };
                publish_diagnostics(client, updates).await;
            });
        } else {
            self.refresh_diagnostics(affected, DiagnosticTrigger::Change)
                .await;
        }
    }

    pub(super) async fn embedding_supertypes(&self, params: Value) -> Result<Value> {
        self.relation(params, |params| {
            Query::Embedding(TypeDirection::Supertypes, params)
        })
        .await
    }

    pub(super) async fn embedding_subtypes(&self, params: Value) -> Result<Value> {
        self.relation(params, |params| {
            Query::Embedding(TypeDirection::Subtypes, params)
        })
        .await
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, params: lsp::InitializeParams) -> Result<lsp::InitializeResult> {
        let params = encode(params)?;
        self.with_workspace(move |workspace| {
            workspace.initialize(&params)?;
            Ok(capabilities::initialize_result(
                &workspace.semantic_config,
                workspace.document_highlight_enabled,
                workspace.diagnostics_on_save,
            ))
        })
        .await
    }

    async fn initialized(&self, _: lsp::InitializedParams) {
        // File watching observes direct edits independently of dynamic
        // client registration, without scheduled or request-triggered scans.
        self.start_file_watcher().await;
        if !self.watched_files_dynamic().await {
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
            let version = params.text_document.version;
            let opened = file.clone();
            let affected = self
                .workspace
                .run(move |workspace| {
                    workspace.set_document_version(&opened, Some(version));
                    Self::update(
                        workspace,
                        WorkspaceEvent::SetBuffer {
                            file: opened,
                            source: params.text_document.text,
                            dirty: false,
                        },
                    )
                })
                .await
                .unwrap_or_default();
            self.refresh_open_diagnostics(file, version, affected).await;
        }
    }

    async fn did_change(&self, params: lsp::DidChangeTextDocumentParams) {
        if let Ok(file) = params.text_document.uri.to_file_path()
            && let Some(change) = params.content_changes.into_iter().last()
        {
            let version = params.text_document.version;
            let affected = self
                .workspace
                .run(move |workspace| {
                    workspace.set_document_version(&file, Some(version));
                    Self::update(
                        workspace,
                        WorkspaceEvent::SetBuffer {
                            file,
                            source: change.text,
                            dirty: true,
                        },
                    )
                })
                .await
                .unwrap_or_default();
            self.refresh_diagnostics(affected, DiagnosticTrigger::Change)
                .await;
        }
    }

    async fn did_close(&self, params: lsp::DidCloseTextDocumentParams) {
        if let Ok(file) = params.text_document.uri.to_file_path() {
            let affected = self
                .workspace
                .run(move |workspace| {
                    workspace.set_document_version(&file, None);
                    Self::update(workspace, WorkspaceEvent::Close(file))
                })
                .await
                .unwrap_or_default();
            // Clear diagnostics without awaiting the client notification inside
            // the notification handler; long open/close bursts otherwise kill
            // the process (observed as a silent -1 exit on Windows).
            let client = self.client.clone();
            let uri = params.text_document.uri;
            tokio::spawn(async move {
                let _ = client.publish_diagnostics(uri, Vec::new(), None).await;
            });
            self.refresh_diagnostics(affected, DiagnosticTrigger::Change)
                .await;
        }
    }

    async fn did_save(&self, params: lsp::DidSaveTextDocumentParams) {
        if let Ok(file) = params.text_document.uri.to_file_path() {
            let affected = self
                .workspace
                .run(move |workspace| Self::update(workspace, WorkspaceEvent::Save(file)))
                .await
                .unwrap_or_default();
            self.refresh_diagnostics(affected, DiagnosticTrigger::Save)
                .await;
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
        if let Err(error) = self
            .with_workspace(move |workspace| {
                workspace.apply_folder_changes(added, removed)?;
                workspace.refresh_file_watch(false);
                Ok(())
            })
            .await
        {
            eprintln!("workspace folders: {}", error.message);
            return;
        }
        if self.watched_files_dynamic().await {
            self.spawn_re_register_watched_files();
        }
    }

    async fn did_change_watched_files(&self, params: lsp::DidChangeWatchedFilesParams) {
        let files = params
            .changes
            .into_iter()
            .filter_map(|change| change.uri.to_file_path().ok())
            .collect();
        let affected = self
            .workspace
            .run(move |workspace| Self::update(workspace, WorkspaceEvent::WatchedFiles(files)))
            .await
            .unwrap_or_default();
        self.refresh_diagnostics(affected, DiagnosticTrigger::Change)
            .await;
    }

    async fn goto_definition(
        &self,
        params: lsp::GotoDefinitionParams,
    ) -> Result<Option<lsp::GotoDefinitionResponse>> {
        self.document(params, |params| DocumentQuery::Definition(params))
            .await
    }

    async fn goto_declaration(
        &self,
        params: lsp::request::GotoDeclarationParams,
    ) -> Result<Option<lsp::request::GotoDeclarationResponse>> {
        self.document(params, |params| DocumentQuery::Declaration(params))
            .await
    }

    async fn goto_implementation(
        &self,
        params: GotoImplementationParams,
    ) -> Result<Option<GotoImplementationResponse>> {
        self.document(params, |params| DocumentQuery::Implementation(params))
            .await
    }

    async fn hover(&self, params: lsp::HoverParams) -> Result<Option<lsp::Hover>> {
        self.document(params, |params| DocumentQuery::Hover(params))
            .await
    }

    async fn document_highlight(
        &self,
        params: lsp::DocumentHighlightParams,
    ) -> Result<Option<Vec<lsp::DocumentHighlight>>> {
        self.document(params, |params| DocumentQuery::DocumentHighlight(params))
            .await
    }

    async fn folding_range(
        &self,
        params: lsp::FoldingRangeParams,
    ) -> Result<Option<Vec<lsp::FoldingRange>>> {
        self.document(params, |_| DocumentQuery::FoldingRange).await
    }

    async fn code_lens(&self, params: lsp::CodeLensParams) -> Result<Option<Vec<lsp::CodeLens>>> {
        self.document(params, |_| DocumentQuery::CodeLens).await
    }

    async fn formatting(
        &self,
        params: lsp::DocumentFormattingParams,
    ) -> Result<Option<Vec<lsp::TextEdit>>> {
        self.document(params, |params| DocumentQuery::Formatting(params))
            .await
    }

    async fn document_link(
        &self,
        params: lsp::DocumentLinkParams,
    ) -> Result<Option<Vec<lsp::DocumentLink>>> {
        // tower-lsp fixes this return type to its own lsp-types `DocumentLink`.
        // Its upstream `target` is `Option<Url>`, which rejects gopls targets such
        // as `https://::1` and normalizes host case/IDN/trailing slashes; the
        // workspace patch keeps the exact wire string (`Option<String>`).
        self.document(params, |_| DocumentQuery::DocumentLink).await
    }

    async fn code_action(
        &self,
        params: lsp::CodeActionParams,
    ) -> Result<Option<lsp::CodeActionResponse>> {
        self.document(params, |params| DocumentQuery::CodeAction(params))
            .await
    }

    async fn prepare_rename(
        &self,
        params: lsp::TextDocumentPositionParams,
    ) -> Result<Option<lsp::PrepareRenameResponse>> {
        self.document(params, |params| DocumentQuery::PrepareRename(params))
            .await
    }

    async fn rename(&self, params: lsp::RenameParams) -> Result<Option<lsp::WorkspaceEdit>> {
        self.document(params, |params| DocumentQuery::Rename(params))
            .await
    }

    async fn goto_type_definition(
        &self,
        params: GotoTypeDefinitionParams,
    ) -> Result<Option<GotoTypeDefinitionResponse>> {
        self.document(params, |params| DocumentQuery::TypeDefinition(params))
            .await
    }

    async fn references(&self, params: lsp::ReferenceParams) -> Result<Option<Vec<lsp::Location>>> {
        self.document(params, |params| DocumentQuery::References(params))
            .await
    }

    async fn completion(
        &self,
        params: lsp::CompletionParams,
    ) -> Result<Option<lsp::CompletionResponse>> {
        self.document(params, |params| DocumentQuery::Completion(params))
            .await
    }

    async fn signature_help(
        &self,
        params: lsp::SignatureHelpParams,
    ) -> Result<Option<lsp::SignatureHelp>> {
        self.document(params, |params| DocumentQuery::SignatureHelp(params))
            .await
    }

    async fn document_symbol(
        &self,
        params: lsp::DocumentSymbolParams,
    ) -> Result<Option<lsp::DocumentSymbolResponse>> {
        self.document(params, |_| DocumentQuery::Symbols).await
    }

    async fn semantic_tokens_full(
        &self,
        params: lsp::SemanticTokensParams,
    ) -> Result<Option<lsp::SemanticTokensResult>> {
        // We only return complete tokens. Decoding the untagged result enum
        // first builds a second serde Content array for the entire token stream.
        let tokens: Option<lsp::SemanticTokens> = self
            .document(params, |_| DocumentQuery::SemanticTokens)
            .await?;
        Ok(tokens.map(lsp::SemanticTokensResult::Tokens))
    }

    async fn semantic_tokens_range(
        &self,
        params: lsp::SemanticTokensRangeParams,
    ) -> Result<Option<lsp::SemanticTokensRangeResult>> {
        let tokens: Option<lsp::SemanticTokens> = self
            .document(params, |params| DocumentQuery::SemanticTokensRange(params))
            .await?;
        Ok(tokens.map(lsp::SemanticTokensRangeResult::Tokens))
    }

    async fn symbol(
        &self,
        params: lsp::WorkspaceSymbolParams,
    ) -> Result<Option<Vec<lsp::SymbolInformation>>> {
        self.relation(params, |params| Query::WorkspaceSymbols(params))
            .await
    }

    async fn prepare_call_hierarchy(
        &self,
        params: lsp::CallHierarchyPrepareParams,
    ) -> Result<Option<Vec<lsp::CallHierarchyItem>>> {
        self.document(params, |params| DocumentQuery::PrepareCallHierarchy(params))
            .await
    }

    async fn incoming_calls(
        &self,
        params: lsp::CallHierarchyIncomingCallsParams,
    ) -> Result<Option<Vec<lsp::CallHierarchyIncomingCall>>> {
        self.relation(params, |params| {
            Query::Calls(CallDirection::Incoming, params)
        })
        .await
    }

    async fn outgoing_calls(
        &self,
        params: lsp::CallHierarchyOutgoingCallsParams,
    ) -> Result<Option<Vec<lsp::CallHierarchyOutgoingCall>>> {
        self.relation(params, |params| {
            Query::Calls(CallDirection::Outgoing, params)
        })
        .await
    }

    async fn prepare_type_hierarchy(
        &self,
        params: lsp::TypeHierarchyPrepareParams,
    ) -> Result<Option<Vec<lsp::TypeHierarchyItem>>> {
        self.document(params, |params| DocumentQuery::PrepareTypeHierarchy(params))
            .await
    }

    async fn supertypes(
        &self,
        params: lsp::TypeHierarchySupertypesParams,
    ) -> Result<Option<Vec<lsp::TypeHierarchyItem>>> {
        self.relation(params, |params| {
            Query::Types(TypeDirection::Supertypes, params)
        })
        .await
    }

    async fn subtypes(
        &self,
        params: lsp::TypeHierarchySubtypesParams,
    ) -> Result<Option<Vec<lsp::TypeHierarchyItem>>> {
        self.relation(params, |params| {
            Query::Types(TypeDirection::Subtypes, params)
        })
        .await
    }
}
