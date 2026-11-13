//! Bounded disk reconciliation triggered by filesystem notifications.
//!
//! The LSP client may not send `didChangeWatchedFiles` (some agent harnesses and
//! clients never register for it). Reconciliation compares the disk identity stored
//! with each indexed stub against the filesystem, detects new/deleted Go files
//! and manifest edits by walking the workspace roots, and reports the changed
//! paths; the backend applies them through the same typed event as watched
//! files. Open buffers are only reloaded from disk while they are clean (no
//! editor `didChange` since the last sync), so unsaved editor edits stay
//! authoritative.

use super::{Workspace, discovery};
use crate::index::DiskStamp;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// Paths reported by one reconciliation; the backend applies them as one bounded batch.
pub(super) const MAX_DISK_CHANGES: usize = 4096;
/// Directory entries visited by one structural walk.
const MAX_DISK_ENTRIES: usize = 20000;
/// Recently checked non-indexable files remembered between reconciliations.
const MAX_UNINDEXED_STAMPS: usize = 2048;

impl Workspace {
    /// Detect direct disk edits without any client notification. Returns changed,
    /// created and deleted paths (Go files and module manifests) in a bounded
    /// batch; callers apply them as `DiskChanges`.
    pub(crate) fn disk_changes(&mut self) -> Vec<PathBuf> {
        let scan_started = std::time::Instant::now();
        let mut changed: Vec<PathBuf> = Vec::new();
        let mut seen: HashSet<PathBuf> = HashSet::new();

        // Dirty open buffers are editor-owned; clean ones are reloaded below when
        // their disk identity moved.
        let mut reload: Vec<(PathBuf, DiskStamp)> = Vec::new();
        for (file, state) in &self.open_state {
            if state.dirty || self.file_watch_excluded(file, false) {
                continue;
            }
            let Some(stamp) = DiskStamp::of(file) else {
                continue; // deleted while open: keep the editor buffer
            };
            if Some(stamp) != state.stamp {
                reload.push((file.clone(), stamp));
            }
        }
        for (file, stamp) in reload {
            if let Some(source) = Self::read_source(&file) {
                self.open.insert(file.clone(), source);
                if let Some(state) = self.open_state.get_mut(&file) {
                    state.stamp = Some(stamp);
                    state.external_reload = true;
                }
                if seen.insert(file.clone()) {
                    changed.push(file);
                }
            }
        }

        // Reuse metadata from the structural walk for indexed files too. On
        // Windows directory entries retain enumeration metadata, so asking
        // each path for metadata separately would add a file-system call per
        // indexed Go file. Borrow index keys instead of cloning/sorting paths.
        // One reconciliation retains at most MAX_DISK_ENTRIES borrowed keys.
        let mut visited = HashSet::new();
        let walk_started = std::time::Instant::now();
        // Structural walk: new Go files and module manifests. Nested unselected
        // modules are recorded but not descended into, mirroring discovery.
        let mut manifests: Vec<PathBuf> = Vec::new();
        let mut candidates: Vec<(PathBuf, DiskStamp)> = Vec::new();
        let manifest_log = std::cell::RefCell::new(Vec::new());
        let mut walked = 0usize;
        for root in &self.roots {
            if self.file_watch_excluded(root, true) {
                continue;
            }
            for entry in WalkDir::new(root)
                .follow_links(false)
                .into_iter()
                .filter_entry(|entry| {
                    if entry.depth() == 0 || !entry.file_type().is_dir() {
                        return true;
                    }
                    if discovery::ignored_directory(entry.file_name())
                        || self.file_watch_excluded(entry.path(), true)
                    {
                        return false;
                    }
                    if !self.modules.contains_key(entry.path())
                        && entry.path().join("go.mod").is_file()
                    {
                        manifest_log.borrow_mut().push(entry.path().join("go.mod"));
                        return false;
                    }
                    true
                })
                .filter_map(Result::ok)
            {
                walked += 1;
                if walked > MAX_DISK_ENTRIES || changed.len() >= MAX_DISK_CHANGES {
                    break;
                }
                if !entry.file_type().is_file() {
                    continue;
                }
                let path = entry.path();
                if let Some((file, stub)) = self.files.get_key_value(path)
                    && self.disk_change_scope(file)
                    && let Ok(metadata) = entry.metadata()
                {
                    visited.insert(file);
                    if Some(DiskStamp::from_metadata(&metadata)) != stub.stamp
                        && seen.insert(file.clone())
                    {
                        changed.push(file.clone());
                    }
                }
                let name = path.file_name().and_then(|name| name.to_str());
                if name == Some("go.mod") || name == Some("go.work") {
                    manifests.push(path.to_path_buf());
                    continue;
                }
                if path.extension().is_some_and(|extension| extension == "go")
                    && !self.files.contains_key(path)
                    && self.discoverable(path)
                    && let Ok(metadata) = entry.metadata()
                {
                    candidates.push((path.to_path_buf(), DiskStamp::from_metadata(&metadata)));
                }
            }
            if walked > MAX_DISK_ENTRIES || changed.len() >= MAX_DISK_CHANGES {
                break;
            }
        }
        let walk_ms = walk_started.elapsed().as_millis();
        let indexed_started = std::time::Instant::now();
        // Missing entries may be deleted, outside the walk's discovery scope,
        // or simply beyond its entry budget. Stat them rather than treating an
        // incomplete walk as proof of deletion. SDK/cache mounts remain excluded.
        for (file, stub) in &self.files {
            if !visited.contains(file)
                && self.disk_change_scope(file)
                && DiskStamp::of(file) != stub.stamp
                && seen.insert(file.clone())
            {
                changed.push(file.clone());
            }
        }
        let indexed_ms = indexed_started.elapsed().as_millis();
        let manifest_started = std::time::Instant::now();
        manifests.extend(manifest_log.into_inner());
        let candidate_count = candidates.len();
        // Only files changed after startup can have become part of the build
        // (new files or tag edits); pre-existing excluded files are skipped
        // without parsing, and recently re-checked exclusions are remembered.
        for (path, stamp) in candidates {
            if self
                .started_at
                .zip(stamp.modified)
                .is_some_and(|(start, modified)| modified < start)
            {
                continue;
            }
            if self.unindexed_stamps.get(&path) == Some(&stamp) {
                continue;
            }
            if self.indexed_file(&path).is_some() {
                if seen.insert(path.clone()) {
                    changed.push(path);
                }
            } else {
                if self.unindexed_stamps.len() >= MAX_UNINDEXED_STAMPS {
                    self.unindexed_stamps.clear();
                }
                self.unindexed_stamps.insert(path, stamp);
            }
        }
        // The first reconciliation records existing manifests as its baseline; later reconciliations
        // treat new, changed or disappeared manifests as module-plan candidates.
        if !self.manifest_baseline {
            for manifest in manifests {
                if let Some(stamp) = DiskStamp::of(&manifest) {
                    if self
                        .started_at
                        .zip(stamp.modified)
                        .is_some_and(|(start, modified)| modified >= start)
                        && seen.insert(manifest.clone())
                    {
                        changed.push(manifest.clone());
                    }
                    self.manifest_stamps.insert(manifest, stamp);
                }
            }
            self.manifest_baseline = true;
        } else {
            for manifest in manifests {
                let current = DiskStamp::of(&manifest);
                if self.manifest_stamps.get(&manifest).copied() != current
                    && seen.insert(manifest.clone())
                {
                    changed.push(manifest);
                }
            }
            let deleted: Vec<PathBuf> = self
                .manifest_stamps
                .keys()
                .filter(|manifest| DiskStamp::of(manifest).is_none())
                .cloned()
                .collect();
            for manifest in deleted {
                if seen.insert(manifest.clone()) {
                    changed.push(manifest);
                }
            }
        }
        let manifest_ms = manifest_started.elapsed().as_millis();
        if self.trace.edits && scan_started.elapsed().as_millis() > 50 {
            eprintln!(
                "disk reconciliation cost: {} ms (indexed {} walk {} manifests {} candidates {} changed {})",
                scan_started.elapsed().as_millis(),
                indexed_ms,
                walk_ms,
                manifest_ms,
                candidate_count,
                changed.len()
            );
        }
        changed.truncate(MAX_DISK_CHANGES);
        if self.trace.edits && !changed.is_empty() {
            eprintln!(
                "disk reconciliation: {} changed path(s), first {:?}",
                changed.len(),
                changed.iter().take(5).collect::<Vec<_>>()
            );
        }
        changed
    }

    /// Record the post-apply disk identities so one change is not reported twice,
    /// and drop manifest stamps that left the workspace.
    pub(crate) fn confirm_disk_paths(&mut self, paths: &[PathBuf]) {
        for path in paths {
            let name = path.file_name().and_then(|name| name.to_str());
            if name != Some("go.mod") && name != Some("go.work") {
                continue;
            }
            match DiskStamp::of(path) {
                Some(stamp) => {
                    self.manifest_stamps.insert(path.clone(), stamp);
                }
                None => {
                    self.manifest_stamps.remove(path);
                }
            }
        }
        self.manifest_stamps.retain(|manifest, _| {
            self.roots.iter().any(|root| manifest.starts_with(root))
                || self
                    .folders
                    .iter()
                    .any(|folder| manifest.starts_with(folder))
        });
    }

    /// Whether this indexed path belongs to the editable workspace rather than an
    /// SDK/dependency mount (which the agent corpus does not edit). A workspace
    /// whose root is the SDK source itself still reconciles its own folder.
    fn disk_change_scope(&self, file: &Path) -> bool {
        if self.file_watch_excluded(file, false) {
            return false;
        }
        let in_workspace_folder = self.folders.iter().any(|folder| file.starts_with(folder));
        if self
            .module_cache
            .as_ref()
            .is_some_and(|cache| file.starts_with(cache))
            && !in_workspace_folder
        {
            return false;
        }
        if self
            .sdk_source_root()
            .is_some_and(|sdk| file.starts_with(sdk))
            && !in_workspace_folder
        {
            return false;
        }
        file.parent().is_some_and(|parent| {
            !self.sdk_packages.contains(parent) && !self.dependency_packages.contains(parent)
        })
    }
}
