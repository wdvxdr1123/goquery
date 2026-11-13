//! All document/watch mutations enter through a typed workspace event.
//! Dynamic workspace-folder changes are not file watch events: the backend
//! notification handler calls `Workspace::apply_folder_changes` directly.
use super::{Workspace, modules, packages};
use crate::bloom::Fingerprint;
use crate::index::{self, IndexedFile};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Bound for dynamically supplied workspace folders; keeps the module plan and
/// root traversal bounded when a client keeps adding folders.
const MAX_WORKSPACE_FOLDERS: usize = 128;
/// Orphan-package siblings admitted for one detached open buffer.
const MAX_DETACHED_SIBLINGS: usize = 64;
/// Total source bytes parsed while admitting orphan-package siblings.
const MAX_DETACHED_SIBLING_BYTES: usize = 8 << 20;
/// Open documents whose diagnostics are recomputed after one mutation. A batch
/// edit can touch many packages, but diagnostic computation stays interactive;
/// documents beyond this bound refresh on their next edit, save or watch event.
const MAX_REFRESH_DOCUMENTS: usize = 64;
/// Direct importers refreshed by one live `didChange`/`didClose` declaration
/// edit. Typing must stay responsive even in sessions with many open files;
/// importers beyond this bound refresh on their next save or watched batch.
const MAX_CHANGE_IMPORTERS: usize = 16;
/// Watched paths applied from one client notification. The transport already
/// bounds the JSON payload; this keeps reindex work interactive if a client
/// reports a whole-tree scan as a single batch.
const MAX_WATCHED_FILES: usize = 4096;
/// Declaration names collected while deciding which importing documents can be
/// affected. Above this bound the conservative answer is "all importers".
const MAX_SCOPE_PROBES: usize = 256;

/// Declaration identity of one indexed file, captured before/after a mutation.
/// `signatures` is `None` when the file has too many declarations to diff by
/// name; such a change conservatively refreshes every importer.
struct DeclarationSnapshot {
    package: String,
    fingerprint: u64,
    signatures: Option<Vec<(String, u64)>>,
}

/// Files above this declaration count keep only the aggregate fingerprint.
const MAX_SNAPSHOT_SYMBOLS: usize = 2048;

fn declaration_snapshot(stub: &IndexedFile) -> DeclarationSnapshot {
    let signatures = (stub.symbols.len() <= MAX_SNAPSHOT_SYMBOLS).then(|| {
        let mut signatures: Vec<(String, u64)> = stub
            .symbols
            .iter()
            .map(|symbol| (symbol.name.clone(), symbol.digest))
            .collect();
        signatures.sort_unstable();
        signatures
    });
    DeclarationSnapshot {
        package: stub.package.clone(),
        fingerprint: stub_declarations_fingerprint(stub),
        signatures,
    }
}

/// Importers that may reference any changed declaration. `None` means every
/// importer must be refreshed (package clause changed or probe set too large);
/// `Some(empty)` means no importer can be affected.
fn changed_declaration_probes(
    previous: Option<&DeclarationSnapshot>,
    current: Option<&DeclarationSnapshot>,
) -> Option<Vec<Fingerprint>> {
    if previous
        .zip(current)
        .is_some_and(|(previous, current)| previous.package != current.package)
    {
        return None;
    }
    let previous = previous.and_then(|snapshot| snapshot.signatures.as_deref());
    let current = current.and_then(|snapshot| snapshot.signatures.as_deref());
    if previous.is_none() && current.is_none() {
        return Some(Vec::new());
    }
    let mut probes = Vec::new();
    if !diff_declaration_names(previous, current, &mut probes) {
        return None;
    }
    Some(probes)
}

/// Merge two name/digest sorted lists and append every name whose digest block
/// differs. Returns false when more than [`MAX_SCOPE_PROBES`] names changed.
fn diff_declaration_names(
    previous: Option<&[(String, u64)]>,
    current: Option<&[(String, u64)]>,
    probes: &mut Vec<Fingerprint>,
) -> bool {
    let (mut left, mut right) = (0usize, 0usize);
    loop {
        let left_name = previous.and_then(|entries| entries.get(left));
        let right_name = current.and_then(|entries| entries.get(right));
        let name = match (left_name, right_name) {
            (None, None) => return true,
            (Some((left, _)), Some((right, _))) => left.min(right).as_str(),
            (Some((left, _)), None) => left.as_str(),
            (None, Some((right, _))) => right.as_str(),
        };
        let left_start = left;
        while previous
            .and_then(|entries| entries.get(left))
            .is_some_and(|(entry, _)| entry == name)
        {
            left += 1;
        }
        let right_start = right;
        while current
            .and_then(|entries| entries.get(right))
            .is_some_and(|(entry, _)| entry == name)
        {
            right += 1;
        }
        let previous_block = previous.map_or(&[][..], |entries| &entries[left_start..left]);
        let current_block = current.map_or(&[][..], |entries| &entries[right_start..right]);
        if previous_block != current_block {
            probes.push(Fingerprint::new(name.as_bytes()));
            if probes.len() > MAX_SCOPE_PROBES {
                return false;
            }
        }
    }
}

/// Open documents whose published diagnostics may be stale after a mutation.
pub(crate) struct ChangeOutcome {
    pub(crate) diagnostics: Vec<PathBuf>,
}

/// Package-level declaration identity of one indexed stub. Byte spans are
/// deliberately excluded: inserting a line above declarations must not look
/// like a declaration change. `declaration_digest` covers the declaration header
/// text (signature, type body, initializer), so a signature-only edit widens the
/// refresh scope while body-only typing stays query-locally cheap. Imports are
/// excluded because one file's imports cannot change another file's diagnostics;
/// an import-only edit therefore only refreshes the edited buffer.
fn stub_declarations_fingerprint(stub: &IndexedFile) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    stub.package.hash(&mut hasher);
    stub.declaration_digest.hash(&mut hasher);
    for symbol in &stub.symbols {
        symbol.name.hash(&mut hasher);
        (symbol.kind as u8).hash(&mut hasher);
        symbol.generic.hash(&mut hasher);
        symbol.alias.hash(&mut hasher);
        symbol.leaf.hash(&mut hasher);
        (symbol.constructor as u8).hash(&mut hasher);
        symbol.receiver.hash(&mut hasher);
    }
    hasher.finish()
}

pub(crate) enum WorkspaceEvent<'a> {
    /// `dirty` distinguishes editor edits from the initial `didOpen` snapshot;
    /// the disk poller never overwrites a dirty buffer.
    SetBuffer {
        file: PathBuf,
        source: &'a str,
        dirty: bool,
    },
    Close(PathBuf),
    Save(PathBuf),
    WatchedFiles(Vec<PathBuf>),
    /// Internal poller batch: same application as watched files.
    PolledFiles(Vec<PathBuf>),
}

/// Compact mutation label for `GOQUERY_TRACE_EDITS`; built only when enabled.
fn event_description(event: &WorkspaceEvent<'_>) -> String {
    match event {
        WorkspaceEvent::SetBuffer { file, .. } => format!("buffer {}", file.display()),
        WorkspaceEvent::Close(file) => format!("close {}", file.display()),
        WorkspaceEvent::Save(file) => format!("save {}", file.display()),
        WorkspaceEvent::WatchedFiles(files) => format!("watched {} path(s)", files.len()),
        WorkspaceEvent::PolledFiles(files) => format!("polled {} path(s)", files.len()),
    }
}

impl Workspace {
    /// Apply one mutation and return the open documents whose diagnostics may
    /// have become stale. A watched batch is indexed with at most one dependency
    /// and SDK discovery pass instead of one pass per file.
    pub(crate) fn apply(
        &mut self,
        event: WorkspaceEvent<'_>,
    ) -> Result<ChangeOutcome, (i32, String)> {
        let trace = self
            .trace
            .edits
            .then(|| (std::time::Instant::now(), event_description(&event)));
        let diagnostics = match event {
            WorkspaceEvent::SetBuffer {
                file,
                source,
                dirty,
            } => {
                if source.len() as u64 > index::MAX_FILE_BYTES {
                    return Err((
                        crate::error::INVALID_PARAMS,
                        "file exceeds 15 MiB limit".into(),
                    ));
                }
                let previous = self.files.get(&file).map(declaration_snapshot);
                // Full-sync edits usually have similar sizes. Reuse an overlay
                // allocation instead of replacing it on every keystroke, but
                // release it when a large deletion leaves more than 2x slack.
                if let Some(buffer) = self.open.get_mut(&file)
                    && buffer.capacity() >= source.len()
                    && buffer.capacity() <= source.len().saturating_mul(2)
                {
                    buffer.clear();
                    buffer.push_str(source);
                } else {
                    self.open.insert(file.clone(), source.to_owned());
                }
                let disk_stamp = index::DiskStamp::of(&file);
                let state = self.open_state.entry(file.clone()).or_default();
                if !dirty {
                    state.dirty = false;
                    state.stamp = disk_stamp;
                    state.external_reload = false;
                } else {
                    state.dirty = true;
                    state.external_reload = false;
                    if state.stamp.is_none() {
                        state.stamp = disk_stamp;
                    }
                }
                if modules::configuration(&file) {
                    // go.work planning reads the open buffer, so an unsaved edit
                    // updates workspace membership transactionally. go.mod is
                    // planned from disk: re-running the plan on every keystroke
                    // would only rescan; saves and watched events refresh it.
                    if file.file_name().is_some_and(|name| name == "go.work") {
                        self.refresh_modules(std::slice::from_ref(&file));
                        self.all_open_documents()
                    } else {
                        Vec::new()
                    }
                } else {
                    self.reindex(&file);
                    // Body-only typing and import-only edits cannot invalidate
                    // another file's diagnostics. A declaration-identity/header
                    // change refreshes the changed-importers set immediately
                    // (both the edited package and direct importers), without
                    // waiting for a save. Importers are narrowed by the changed
                    // declaration names; saves and watched batches use the same
                    // bounded rule.
                    if previous.as_ref().map(|previous| previous.fingerprint)
                        == self.files.get(&file).map(stub_declarations_fingerprint)
                    {
                        self.file_documents(&file)
                    } else {
                        let current = self.files.get(&file).map(declaration_snapshot);
                        let probes =
                            changed_declaration_probes(previous.as_ref(), current.as_ref());
                        self.changed_documents(
                            std::slice::from_ref(&file),
                            probes.as_deref(),
                            MAX_CHANGE_IMPORTERS,
                        )
                    }
                }
            }
            WorkspaceEvent::Close(file) => {
                let previous = self.files.get(&file).map(declaration_snapshot);
                self.open.remove(&file);
                self.open_state.remove(&file);
                let diagnostics = if modules::configuration(&file) {
                    self.refresh_modules(std::slice::from_ref(&file));
                    self.all_open_documents()
                } else {
                    self.reindex(&file);
                    // Closing an unmodified or body/import-only edited buffer
                    // restores equivalent package declarations; only the closed
                    // document's diagnostics must be cleared (the backend does
                    // that).
                    if previous.as_ref().map(|previous| previous.fingerprint)
                        == self.files.get(&file).map(stub_declarations_fingerprint)
                    {
                        Vec::new()
                    } else {
                        let current = self.files.get(&file).map(declaration_snapshot);
                        let probes =
                            changed_declaration_probes(previous.as_ref(), current.as_ref());
                        self.changed_documents(
                            std::slice::from_ref(&file),
                            probes.as_deref(),
                            MAX_CHANGE_IMPORTERS,
                        )
                    }
                };
                self.prune_detached_siblings(file.parent());
                self.prune_orphan_dependencies();
                diagnostics
            }
            WorkspaceEvent::Save(file) => {
                if let Some(state) = self.open_state.get_mut(&file) {
                    state.dirty = false;
                    state.stamp = index::DiskStamp::of(&file);
                    state.external_reload = false;
                }
                if modules::configuration(&file) {
                    self.refresh_modules(std::slice::from_ref(&file));
                    self.all_open_documents()
                } else {
                    let previous = self.files.get(&file).map(declaration_snapshot);
                    self.reindex(&file);
                    let current = self.files.get(&file).map(declaration_snapshot);
                    let probes = if previous.as_ref().map(|previous| previous.fingerprint)
                        == current.as_ref().map(|current| current.fingerprint)
                    {
                        Some(Vec::new())
                    } else {
                        changed_declaration_probes(previous.as_ref(), current.as_ref())
                    };
                    self.changed_documents(
                        std::slice::from_ref(&file),
                        probes.as_deref(),
                        MAX_REFRESH_DOCUMENTS,
                    )
                }
            }
            WorkspaceEvent::WatchedFiles(files) | WorkspaceEvent::PolledFiles(files) => {
                if files.len() > MAX_WATCHED_FILES {
                    // A whole-tree batch cannot be applied partially without
                    // leaving stale files, so rebuild the bounded workspace
                    // plan/index once instead of truncating the event.
                    eprintln!(
                        "watched files: {} paths exceed the {MAX_WATCHED_FILES} per-event limit; \
                         rebuilding the workspace index",
                        files.len()
                    );
                    // The batch was not applied file-by-file, so force the full
                    // walk: a plan digest alone cannot prove the index is current.
                    let folders = self.folders.clone();
                    if let Err(error) = self.scan_roots(folders) {
                        eprintln!("workspace modules: {error}");
                    }
                    self.all_open_documents()
                } else {
                    let mut configuration_files: Vec<PathBuf> = Vec::new();
                    let mut go_files: Vec<PathBuf> = Vec::new();
                    let mut seen = HashSet::new();
                    for file in files {
                        if !seen.insert(file.clone()) {
                            continue;
                        }
                        if modules::configuration(&file) {
                            configuration_files.push(file);
                        } else if file.extension().is_some_and(|extension| extension == "go") {
                            go_files.push(file);
                        }
                    }
                    let mut dependencies_changed = false;
                    let mut probes = Vec::new();
                    let mut probes_unbounded = false;
                    for file in &go_files {
                        let previous = self.files.get(file).map(declaration_snapshot);
                        let next = self.indexed_file(file);
                        let declarations_changed =
                            previous.as_ref().map(|previous| previous.fingerprint)
                                != next.as_ref().map(stub_declarations_fingerprint);
                        dependencies_changed |= self.install_index(file, next);
                        if !declarations_changed {
                            continue;
                        }
                        let current = self.files.get(file).map(declaration_snapshot);
                        match changed_declaration_probes(previous.as_ref(), current.as_ref()) {
                            None => probes_unbounded = true,
                            Some(items) => {
                                probes.extend(items);
                                if probes.len() > MAX_SCOPE_PROBES {
                                    probes_unbounded = true;
                                }
                            }
                        }
                        if probes_unbounded {
                            break;
                        }
                    }
                    if dependencies_changed {
                        self.load_dependencies();
                        self.load_sdk();
                    }
                    if !configuration_files.is_empty() {
                        self.refresh_modules(&configuration_files);
                        self.all_open_documents()
                    } else if probes_unbounded {
                        self.changed_documents(&go_files, None, MAX_REFRESH_DOCUMENTS)
                    } else {
                        self.changed_documents(&go_files, Some(&probes), MAX_REFRESH_DOCUMENTS)
                    }
                }
            }
        };
        if let Some((started, description)) = trace {
            eprintln!(
                "workspace edit: {description} -> {} diagnostic document(s), {} ms",
                diagnostics.len(),
                started.elapsed().as_millis()
            );
        }
        Ok(ChangeOutcome { diagnostics })
    }

    /// Just `file` when it is open.
    fn file_documents(&self, file: &Path) -> Vec<PathBuf> {
        self.refresh_documents(|open, _| open == file)
    }

    /// Open documents affected by changed files: the changed/same-package set
    /// first, then direct importers. Preference order is deterministic so a
    /// capped batch never drops the edited package before its consumers.
    /// `probes` narrows importers to files whose identifier filter may contain a
    /// changed declaration name; `None` refreshes every importer and
    /// `Some(empty)` refreshes none. `importer_limit` bounds the importer part
    /// independently so live edits stay interactive.
    fn changed_documents(
        &self,
        changed: &[PathBuf],
        probes: Option<&[Fingerprint]>,
        importer_limit: usize,
    ) -> Vec<PathBuf> {
        let directories: HashSet<&Path> = changed.iter().filter_map(|file| file.parent()).collect();
        let imports: HashSet<String> = changed
            .iter()
            .filter_map(|file| self.import_path(file))
            .collect();
        let mut primary = Vec::new();
        let mut secondary = Vec::new();
        let mut open: Vec<&PathBuf> = self.open.keys().collect();
        open.sort();
        for file in open {
            if file.extension().is_none_or(|extension| extension != "go") {
                continue;
            }
            if changed.iter().any(|changed| changed == file)
                || file
                    .parent()
                    .is_some_and(|parent| directories.contains(parent))
            {
                primary.push(file.clone());
            } else if self.files.get(file).is_some_and(|stub| {
                probes.is_none_or(|probes| {
                    probes
                        .iter()
                        .any(|probe| stub.identifiers.may_contain_fingerprint(*probe))
                }) && stub
                    .imports
                    .iter()
                    .any(|import| imports.contains(&import.path))
            }) {
                secondary.push(file.clone());
            }
        }
        primary.truncate(MAX_REFRESH_DOCUMENTS);
        let remaining = MAX_REFRESH_DOCUMENTS.saturating_sub(primary.len());
        primary.extend(secondary.into_iter().take(remaining.min(importer_limit)));
        primary
    }

    /// Every open `.go` document, used when the module plan itself changed.
    fn all_open_documents(&self) -> Vec<PathBuf> {
        self.refresh_documents(|_, _| true)
    }

    fn refresh_documents(
        &self,
        include: impl Fn(&Path, Option<&IndexedFile>) -> bool,
    ) -> Vec<PathBuf> {
        let mut documents: Vec<PathBuf> = self
            .open
            .iter()
            .filter(|(file, _)| file.extension().is_some_and(|extension| extension == "go"))
            .filter(|(file, _)| include(file, self.files.get(*file)))
            .map(|(file, _)| file.clone())
            .collect();
        documents.sort();
        documents.truncate(MAX_REFRESH_DOCUMENTS);
        documents
    }

    /// Replace the configured workspace folders. Re-planning is transactional:
    /// a failed module plan keeps the previous folders, roots and index intact.
    /// A successful refresh rebuilds every derived index from the new roots, so
    /// files under removed folders leave the index; open buffers are re-admitted
    /// as editor-owned scratch files, exactly as after initialization.
    pub(crate) fn set_folders(&mut self, folders: Vec<PathBuf>) -> Result<(), String> {
        let folders = normalized_folders(folders);
        if folders == self.folders {
            return Ok(());
        }
        self.scan_roots(folders)
    }

    /// Apply one LSP `didChangeWorkspaceFolders` change in client order: removed
    /// folders leave the set, added folders append, duplicates collapse. The
    /// returned protocol error is for the notification handler to log; on failure
    /// the previous folders, module plan and index remain intact.
    pub(crate) fn apply_folder_changes(
        &mut self,
        added: Vec<PathBuf>,
        removed: Vec<PathBuf>,
    ) -> Result<(), (i32, String)> {
        let removed: Vec<PathBuf> = removed.into_iter().map(normalize_folder).collect();
        let mut folders: Vec<PathBuf> = self
            .folders
            .iter()
            .filter(|folder| !removed.contains(folder))
            .cloned()
            .collect();
        folders.extend(added);
        self.set_folders(folders)
            .map_err(|error| (-32603, format!("workspace folders: {error}")))
    }
}

/// Absolute, duplicate-free folder list in client order, capped for bounded
/// module/graph work.
fn normalized_folders(folders: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut result: Vec<PathBuf> = Vec::new();
    for folder in folders {
        let folder = normalize_folder(folder);
        if folder.as_os_str().is_empty() || result.contains(&folder) {
            continue;
        }
        result.push(folder);
        if result.len() == MAX_WORKSPACE_FOLDERS {
            break;
        }
    }
    result
}

fn normalize_folder(folder: PathBuf) -> PathBuf {
    if folder.as_os_str().is_empty() {
        return folder;
    }
    let base = std::env::current_dir().unwrap_or_default();
    modules::absolute(&base, &folder)
}

impl Workspace {
    /// Admit a document using the same scope/build rules as indexing, then return
    /// one query-local source snapshot. No closed source is retained here.
    pub(crate) fn prepare_document(
        &mut self,
        file: &Path,
    ) -> Result<Option<String>, (i32, String)> {
        if !self.files.contains_key(file)
            && !self.open.contains_key(file)
            && (!self.roots.iter().any(|root| file.starts_with(root))
                || self
                    .excluded_modules
                    .iter()
                    .any(|root| file.starts_with(root)))
        {
            return Err((
                0,
                format!("no package metadata for file {}", super::uri(file)),
            ));
        }
        let source = if self.files.contains_key(file) && self.open.contains_key(file) {
            None
        } else {
            Some(self.source(file).ok_or((
                crate::error::INVALID_PARAMS,
                "file unavailable or too large".into(),
            ))?)
        };
        if !self.files.contains_key(file) {
            let next = source
                .as_deref()
                .and_then(|source| self.indexed_file_from(file, source));
            if self.install_index(file, next) {
                self.load_dependencies();
                self.load_sdk();
            }
        }
        if !self.files.contains_key(file)
            && self.open.contains_key(file)
            && self.roots.iter().any(|root| file.starts_with(root))
            && let Some(source) = source
                .as_deref()
                .or_else(|| self.open.get(file).map(String::as_str))
            && let Some(stub) = self.detached_file_from(file, source)
        {
            self.files.insert(file.to_path_buf(), stub);
            self.detached.insert(file.to_path_buf());
            self.admit_detached_siblings(file);
            // A file under an unselected nested module mounts that module as an
            // additional view so its module-local imports resolve.
            self.load_nested_view(file);
            // Orphaned files use a view selected by their own build constraints;
            // load only the missing imported SDK variants for that view.
            self.load_detached_import_variants(file);
        }
        if !self.files.contains_key(file) {
            let reason = if self.discoverable(file) {
                "file excluded by build constraints"
            } else {
                "file excluded by workspace discovery rules"
            };
            return Err((crate::error::INVALID_PARAMS, reason.into()));
        }
        Ok(source)
    }
    pub(super) fn index(&mut self, file: &Path) {
        let next = self.indexed_file(file);
        if self.install_index(file, next) {
            self.load_dependencies();
            self.load_sdk();
        }
    }

    /// Install one indexed stub (or remove it) and report whether dependency or
    /// SDK discovery is now stale because the import set changed.
    fn install_index(&mut self, file: &Path, next: Option<IndexedFile>) -> bool {
        self.detached.remove(file);
        let discover_sdk = match (self.files.get(file), next.as_ref()) {
            (None, None) => false,
            (Some(old), Some(next)) => {
                old.imports.len() != next.imports.len()
                    || old
                        .imports
                        .iter()
                        .zip(&next.imports)
                        .any(|(old, next)| old.path != next.path || old.name != next.name)
            }
            _ => true,
        };
        if self.files.get(file).map(|stub| &stub.package) != next.as_ref().map(|stub| &stub.package)
            && packages::production_file(file)
            && let Some(import) = self.import_path(file)
        {
            if let Some(previous) = self.files.get(file) {
                self.package_names.remove(&import, &previous.package);
            }
            if let Some(stub) = &next {
                self.package_names.add(import, &stub.package);
            }
        }
        if let Some(stub) = next {
            self.files.insert(file.to_path_buf(), stub);
        } else {
            self.files.remove(file);
        }
        discover_sdk
    }

    pub(super) fn indexed_file(&self, file: &Path) -> Option<IndexedFile> {
        if file.extension().is_none_or(|extension| extension != "go") {
            return None;
        }
        if !self.discoverable(file) {
            return None;
        }
        let name = file
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        if !self.build_target.matches_filename(name) {
            return None;
        }
        if let Some(source) = self.open_source(file) {
            self.indexed_file_from(file, source)
        } else {
            let source = Self::read_source(file)?;
            self.indexed_file_from(file, &source)
        }
    }

    fn indexed_file_from(&self, file: &Path, source: &str) -> Option<IndexedFile> {
        if file.extension().is_none_or(|extension| extension != "go") {
            return None;
        }
        if !self.discoverable(file) {
            return None;
        }
        let name = file
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        if !self.build_target.matches_filename(name)
            || self.build_target.matches_file(name, source) != Ok(true)
        {
            return None;
        }
        let mut stub = index::index_source(source);
        stub.stamp = index::DiskStamp::of(file);
        // A cgo-disabled build context excludes files importing "C", exactly as
        // the Go build system does; an enabled context contributes the `cgo` tag.
        if self.cgo_disabled && stub.imports.iter().any(|import| import.path == "C") {
            return None;
        }
        for import in &mut stub.imports {
            self.sdk_import_identity(file, &mut import.path);
        }
        stub.rebuild_import_filter();
        Some(stub)
    }

    /// Index an open buffer outside the build/discovery scope (gopls' orphaned
    /// file). The file still gets import identities so local selectors
    /// can resolve, but it never joins package-wide scans.
    fn detached_file_from(&self, file: &Path, source: &str) -> Option<IndexedFile> {
        if file.extension().is_none_or(|extension| extension != "go") {
            return None;
        }
        if source.len() as u64 > index::MAX_FILE_BYTES {
            return None;
        }
        let mut stub = index::index_source(source);
        stub.stamp = index::DiskStamp::of(file);
        if self.cgo_disabled && stub.imports.iter().any(|import| import.path == "C") {
            return None;
        }
        for import in &mut stub.imports {
            self.sdk_import_identity(file, &mut import.path);
        }
        stub.rebuild_import_filter();
        Some(stub)
    }

    /// Load the orphan package view for a detached open buffer: active sibling
    /// files with the same package clause. They are marked detached so they
    /// answer only together with the orphaned file, never for real packages.
    fn admit_detached_siblings(&mut self, file: &Path) {
        let Some((directory, package)) = self.files.get(file).and_then(|stub| {
            file.parent()
                .map(|directory| (directory.to_path_buf(), stub.package.clone()))
        }) else {
            return;
        };
        let Ok(entries) = std::fs::read_dir(&directory) else {
            return;
        };
        // A detached file may be inactive only because the host view differs
        // (e.g. a unix-only file opened on Windows). Its orphan package uses
        // the file's own dynamic view target, so build-variant siblings are
        // admitted instead of host-only siblings.
        let target = self.orphan_view_target(file);
        let mut bytes = 0usize;
        let mut admitted = 0usize;
        for entry in entries.flatten() {
            if admitted >= MAX_DETACHED_SIBLINGS || bytes >= MAX_DETACHED_SIBLING_BYTES {
                break;
            }
            let path = entry.path();
            if path == file || self.files.contains_key(&path) || self.open.contains_key(&path) {
                continue;
            }
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if !name.ends_with(".go") || name.ends_with("_test.go") {
                continue;
            }
            if !self.roots.iter().any(|root| path.starts_with(root)) {
                continue;
            }
            let Some(source) = self.source(&path) else {
                continue;
            };
            let matches = match &target {
                Some(target) => {
                    target.matches_filename(name) && target.matches_file(name, &source) == Ok(true)
                }
                None => {
                    self.build_target.matches_filename(name)
                        && self.build_target.matches_file(name, &source) == Ok(true)
                }
            };
            if !matches {
                continue;
            }
            let Some(stub) = self.detached_file_from(&path, &source) else {
                continue;
            };
            if stub.package != package {
                continue;
            }
            bytes += source.len();
            admitted += 1;
            self.files.insert(path.clone(), stub);
            self.detached.insert(path);
        }
    }

    /// Drop sibling stubs admitted only for a detached open buffer once that
    /// directory has no remaining detached open document.
    fn prune_detached_siblings(&mut self, directory: Option<&Path>) {
        let Some(directory) = directory else {
            return;
        };
        let removable: Vec<PathBuf> = self
            .detached
            .iter()
            .filter(|path| path.parent() == Some(directory) && !self.open.contains_key(*path))
            .cloned()
            .collect();
        for path in removable {
            if self.nested_files.contains(&path) {
                continue;
            }
            self.files.remove(&path);
            self.detached.remove(&path);
        }
    }

    /// Drop SDK variants loaded only for an orphaned buffer once no detached
    /// document remains open. Nested-module files keep their additional view.
    fn prune_orphan_dependencies(&mut self) {
        if self
            .detached
            .iter()
            .any(|path| self.open.contains_key(path))
        {
            return;
        }
        let removable: Vec<PathBuf> = self
            .detached
            .iter()
            .filter(|path| !self.open.contains_key(*path) && !self.nested_files.contains(*path))
            .cloned()
            .collect();
        for path in removable {
            self.files.remove(&path);
            self.detached.remove(&path);
        }
    }
}
