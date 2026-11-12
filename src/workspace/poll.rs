//! Bounded disk polling for direct (agent/shell) edits without editor events.
//!
//! The LSP client may not send `didChangeWatchedFiles` (some agent harnesses and
//! clients never register for it). The poller compares the disk identity stored
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

/// Paths reported by one poll; the backend applies them as one bounded batch.
pub(super) const MAX_POLL_CHANGES: usize = 4096;
/// Directory entries visited by one structural walk.
const MAX_POLL_ENTRIES: usize = 20000;
/// Recently checked non-indexable files remembered between polls.
const MAX_UNINDEXED_STAMPS: usize = 2048;

impl Workspace {
    /// Detect direct disk edits without any client notification. Returns changed,
    /// created and deleted paths (Go files and module manifests) in a bounded
    /// batch; callers apply them as `PolledFiles`.
    pub(crate) fn poll_disk_changes(&mut self) -> Vec<PathBuf> {
        let poll_started = std::time::Instant::now();
        let mut changed: Vec<PathBuf> = Vec::new();
        let mut seen: HashSet<PathBuf> = HashSet::new();

        // Dirty open buffers are editor-owned; clean ones are reloaded below when
        // their disk identity moved.
        let mut reload: Vec<(PathBuf, DiskStamp)> = Vec::new();
        for (file, state) in &self.open_state {
            if state.dirty {
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

        let indexed_started = std::time::Instant::now();
        // Indexed stubs: compare the stored stamp. SDK and dependency mounts are
        // read-only corpora; agents edit workspace and local-replace files.
        let mut indexed: Vec<(PathBuf, Option<DiskStamp>)> = self
            .files
            .iter()
            .filter(|(file, _)| self.poll_scope(file))
            .map(|(file, stub)| (file.clone(), stub.stamp))
            .collect();
        indexed.sort_by(|left, right| left.0.cmp(&right.0));
        for (file, stamp) in indexed {
            if DiskStamp::of(&file) != stamp && seen.insert(file.clone()) {
                changed.push(file);
            }
        }

        let indexed_ms = indexed_started.elapsed().as_millis();
        let walk_started = std::time::Instant::now();
        // Structural walk: new Go files and module manifests. Nested unselected
        // modules are recorded but not descended into, mirroring discovery.
        let roots = self.roots.clone();
        let mut manifests: Vec<PathBuf> = Vec::new();
        let mut candidates: Vec<(PathBuf, DiskStamp)> = Vec::new();
        let manifest_log = std::cell::RefCell::new(Vec::new());
        let mut walked = 0usize;
        for root in roots {
            for entry in WalkDir::new(&root)
                .follow_links(false)
                .into_iter()
                .filter_entry(|entry| {
                    if entry.depth() == 0 || !entry.file_type().is_dir() {
                        return true;
                    }
                    if discovery::ignored_directory(entry.file_name()) {
                        return false;
                    }
                    if entry.path().join("go.mod").is_file()
                        && !self.modules.contains_key(entry.path())
                    {
                        manifest_log.borrow_mut().push(entry.path().join("go.mod"));
                        return false;
                    }
                    true
                })
                .filter_map(Result::ok)
            {
                walked += 1;
                if walked > MAX_POLL_ENTRIES || changed.len() >= MAX_POLL_CHANGES {
                    break;
                }
                if !entry.file_type().is_file() {
                    continue;
                }
                let path = entry.path();
                let name = path.file_name().and_then(|name| name.to_str());
                if name == Some("go.mod") || name == Some("go.work") {
                    manifests.push(path.to_path_buf());
                    continue;
                }
                if path.extension().is_some_and(|extension| extension == "go")
                    && !self.files.contains_key(path)
                    && self.discoverable(path)
                    && let Some(stamp) = DiskStamp::of(path)
                {
                    candidates.push((path.to_path_buf(), stamp));
                }
            }
            if walked > MAX_POLL_ENTRIES || changed.len() >= MAX_POLL_CHANGES {
                break;
            }
        }
        let walk_ms = walk_started.elapsed().as_millis();
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
        // The first poll records existing manifests as its baseline; later polls
        // treat new, changed or disappeared manifests as module-plan candidates.
        if !self.manifest_baseline {
            for manifest in manifests {
                if let Some(stamp) = DiskStamp::of(&manifest) {
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
        if self.trace.edits && poll_started.elapsed().as_millis() > 50 {
            eprintln!(
                "disk poll cost: {} ms (indexed {} walk {} manifests {} candidates {} changed {})",
                poll_started.elapsed().as_millis(),
                indexed_ms,
                walk_ms,
                manifest_ms,
                candidate_count,
                changed.len()
            );
        }
        changed.truncate(MAX_POLL_CHANGES);
        if self.trace.edits && !changed.is_empty() {
            eprintln!(
                "disk poll: {} changed path(s), first {:?}",
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

    /// Number of poll-scope (workspace) files currently indexed. On-demand
    /// request polling is only affordable for small/medium workspaces; large
    /// trees rely on the background interval and burst acceleration.
    pub(crate) fn poll_scope_file_count(&self) -> usize {
        self.files
            .keys()
            .filter(|file| self.poll_scope(file))
            .count()
    }

    /// Whether this indexed path belongs to the editable workspace rather than an
    /// SDK/dependency mount (which the agent corpus does not edit). A workspace
    /// whose root is the SDK source itself still polls its own folder.
    fn poll_scope(&self, file: &Path) -> bool {
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
