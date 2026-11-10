//! All document/watch mutations enter through a typed workspace event.
//! Dynamic workspace-folder changes are not file watch events: the backend
//! notification handler calls `Workspace::apply_folder_changes` directly.
use super::{Workspace, modules, packages};
use crate::index::{self, IndexedFile};
use std::path::{Path, PathBuf};

/// Bound for dynamically supplied workspace folders; keeps the module plan and
/// root traversal bounded when a client keeps adding folders.
const MAX_WORKSPACE_FOLDERS: usize = 128;
/// Orphan-package siblings admitted for one detached open buffer.
const MAX_DETACHED_SIBLINGS: usize = 64;
/// Total source bytes parsed while admitting orphan-package siblings.
const MAX_DETACHED_SIBLING_BYTES: usize = 8 << 20;

pub(crate) enum WorkspaceEvent<'a> {
    SetBuffer { file: PathBuf, source: &'a str },
    Close(PathBuf),
    Save(PathBuf),
    WatchedFiles(Vec<PathBuf>),
}

impl Workspace {
    pub(crate) fn apply(&mut self, event: WorkspaceEvent<'_>) -> Result<(), (i32, String)> {
        match event {
            WorkspaceEvent::SetBuffer { file, source } => {
                if source.len() as u64 > index::MAX_FILE_BYTES {
                    return Err((
                        crate::error::INVALID_PARAMS,
                        "file exceeds 15 MiB limit".into(),
                    ));
                }
                self.open.insert(file.clone(), source.to_owned());
                if file.file_name().is_none_or(|name| name != "go.mod") {
                    self.reindex(&file);
                }
            }
            WorkspaceEvent::Close(file) => {
                self.open.remove(&file);
                self.reindex(&file);
                self.prune_detached_siblings(file.parent());
                self.prune_orphan_dependencies();
            }
            WorkspaceEvent::Save(file) => self.reindex(&file),
            WorkspaceEvent::WatchedFiles(files) => {
                let mut refresh = false;
                for file in files {
                    if modules::configuration(&file) {
                        refresh = true;
                    } else {
                        self.index(&file);
                    }
                }
                if refresh {
                    self.refresh_modules();
                }
            }
        }
        Ok(())
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
            self.install_index(file, next);
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
        self.install_index(file, next);
    }

    fn install_index(&mut self, file: &Path, next: Option<IndexedFile>) {
        self.detached.remove(file);
        let discover_sdk = next.as_ref().is_none_or(|next| {
            self.files.get(file).is_none_or(|old| {
                old.imports.len() != next.imports.len()
                    || old
                        .imports
                        .iter()
                        .zip(&next.imports)
                        .any(|(old, next)| old.path != next.path || old.name != next.name)
            })
        });
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
        if discover_sdk {
            self.load_dependencies();
            self.load_sdk();
        }
    }

    pub(super) fn indexed_file(&self, file: &Path) -> Option<IndexedFile> {
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
        let source = self.source(file)?;
        self.indexed_file_from(file, &source)
    }

    fn indexed_file_from(&self, file: &Path, source: &str) -> Option<IndexedFile> {
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
        if source.len() as u64 > index::MAX_FILE_BYTES {
            return None;
        }
        let mut stub = index::index_source(source);
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
