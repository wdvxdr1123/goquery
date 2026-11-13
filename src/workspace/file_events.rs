//! Filesystem notifications are coalesced hints; bounded disk reconciliation owns
//! change detection and the usual typed workspace update path.
use super::{Workspace, discovery};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
const RECURSIVE_ROOTS: bool = cfg!(any(windows, target_os = "macos"));

#[derive(Default)]
pub(super) struct WatchExcludes(pub Option<regex::RegexSet>);

impl WatchExcludes {
    /// Match directory prefixes relative to the first covering workspace root.
    /// Roots themselves remain watched; filenames are never tested as directories.
    pub(super) fn matches(&self, roots: &[PathBuf], path: &Path, directory: bool) -> bool {
        let Some(directory) = (if directory { Some(path) } else { path.parent() }) else {
            return false;
        };
        let Some(relative) = roots
            .iter()
            .find_map(|root| directory.strip_prefix(root).ok())
        else {
            return false;
        };
        let mut prefix = String::new();
        for component in relative.components() {
            if discovery::ignored_directory(component.as_os_str()) {
                return true;
            }
            if self.0.is_some() {
                if !prefix.is_empty() {
                    prefix.push('/');
                }
                prefix.push_str(&component.as_os_str().to_string_lossy());
                if self
                    .0
                    .as_ref()
                    .is_some_and(|patterns| patterns.is_match(&prefix))
                {
                    return true;
                }
            }
        }
        false
    }
}

pub(super) struct WatchLimits {
    pub roots: usize,
    pub directories: usize,
    pub entries: usize,
}

impl Default for WatchLimits {
    fn default() -> Self {
        Self {
            roots: 128,
            directories: 4096,
            entries: 20000,
        }
    }
}

pub(super) struct FileWatch {
    _watcher: Option<RecommendedWatcher>,
    roots: Vec<PathBuf>,
    paths: Vec<PathBuf>,
    dirty: Arc<AtomicBool>,
    healthy: Arc<AtomicBool>,
}

impl Workspace {
    pub(crate) fn file_watch_enabled(&self) -> bool {
        self.file_watch_enabled
    }

    pub(crate) fn file_watch_signal(&self) -> Arc<tokio::sync::Notify> {
        Arc::clone(&self.watch_signal)
    }

    pub(super) fn file_watch_excluded(&self, path: &Path, directory: bool) -> bool {
        self.file_watch_excludes
            .matches(&self.roots, path, directory)
    }
    pub(crate) fn file_watch_active(&self) -> bool {
        self.file_watch
            .as_ref()
            .is_some_and(|watch| watch.healthy.load(Ordering::Relaxed))
    }

    pub(crate) fn take_file_changes(&self) -> bool {
        self.file_watch
            .as_ref()
            .is_some_and(|watch| watch.dirty.swap(false, Ordering::Relaxed))
    }

    /// Refresh roots after module/folder changes. On platforms with per-directory
    /// watches, also discover newly created directories after reconciliation.
    pub(crate) fn refresh_file_watch(&mut self, reconcile: bool) {
        if !self.file_watch_enabled {
            self.file_watch = None;
            return;
        }
        if let Some(watch) = &self.file_watch
            && watch.roots == self.roots
            && (!self.file_watch_active() || RECURSIVE_ROOTS || !reconcile)
        {
            return;
        }
        let paths = self.file_watch_paths();
        if self.file_watch_active()
            && let (Some(watch), Ok(paths)) = (&self.file_watch, &paths)
            && watch.roots == self.roots
            && watch.paths == *paths
        {
            return;
        }
        let dirty = Arc::new(AtomicBool::new(false));
        let healthy = Arc::new(AtomicBool::new(false));
        let callback_dirty = Arc::clone(&dirty);
        let callback_healthy = Arc::clone(&healthy);
        let signal = Arc::clone(&self.watch_signal);
        let excludes = Arc::clone(&self.file_watch_excludes);
        let roots = self.roots.clone();
        #[cfg(not(target_os = "macos"))]
        let callback_roots = roots.clone();
        // FSEvents resolves watched paths (notably /var -> /private/var).
        #[cfg(target_os = "macos")]
        let callback_roots: Vec<_> = roots
            .iter()
            .map(|root| root.canonicalize().unwrap_or_else(|_| root.clone()))
            .collect();
        let result = paths.and_then(|paths| {
            let mut watcher = RecommendedWatcher::new(
                move |result: notify::Result<notify::Event>| match result {
                    Ok(event) => {
                        if event.need_rescan() {
                            callback_dirty.store(true, Ordering::Relaxed);
                            signal.notify_one();
                            return;
                        }
                        if matches!(event.kind, EventKind::Access(_)) {
                            return;
                        }
                        if event.paths.is_empty()
                            || event.paths.len() > 4096
                            || event.paths.iter().any(|path| {
                                let directory = matches!(
                                    event.kind,
                                    EventKind::Create(notify::event::CreateKind::Folder)
                                        | EventKind::Remove(notify::event::RemoveKind::Folder)
                                ) || path.is_dir();
                                callback_roots.iter().any(|root| path.starts_with(root))
                                    && !excludes.matches(&callback_roots, path, directory)
                            })
                        {
                            callback_dirty.store(true, Ordering::Relaxed);
                            signal.notify_one();
                        }
                    }
                    Err(error) => {
                        if callback_healthy.swap(false, Ordering::Relaxed) {
                            eprintln!("file watching failed; disk polling is disabled: {error}");
                        }
                        callback_dirty.store(true, Ordering::Relaxed);
                        signal.notify_one();
                    }
                },
                notify::Config::default().with_follow_symlinks(false),
            )
            .map_err(|error| error.to_string())?;
            let mode = if RECURSIVE_ROOTS {
                RecursiveMode::Recursive
            } else {
                RecursiveMode::NonRecursive
            };
            // Set before registration so an asynchronous error cannot be lost.
            healthy.store(true, Ordering::Relaxed);
            for path in &paths {
                watcher
                    .watch(path, mode)
                    .map_err(|error| error.to_string())?;
            }
            Ok((watcher, paths))
        });
        let (watcher, paths) = match result {
            Ok((watcher, paths)) => {
                dirty.store(true, Ordering::Relaxed);
                self.watch_signal.notify_one();
                (Some(watcher), paths)
            }
            Err(error) => {
                healthy.store(false, Ordering::Relaxed);
                eprintln!("file watching unavailable; disk polling is disabled: {error}");
                (None, Vec::new())
            }
        };
        self.file_watch = Some(FileWatch {
            _watcher: watcher,
            roots,
            paths,
            dirty,
            healthy,
        });
    }

    fn file_watch_paths(&self) -> Result<Vec<PathBuf>, String> {
        // Module planning sorts roots. A recursive ancestor also covers nested
        // module roots; avoid duplicate watches and duplicate discovery budgets.
        let mut roots: Vec<PathBuf> = Vec::new();
        for root in &self.roots {
            if self.file_watch_excluded(root, true) {
                continue;
            }
            if roots.last().is_some_and(|parent| root.starts_with(parent)) {
                continue;
            }
            let limit = self.file_watch_limits.roots;
            if limit != 0 && roots.len() >= limit {
                return Err(format!(
                    "fileWatchMaxRoots={limit} exceeded; increase it or use 0 for unlimited"
                ));
            }
            roots.push(root.clone());
        }
        #[cfg(any(windows, target_os = "macos"))]
        {
            Ok(roots)
        }
        #[cfg(not(any(windows, target_os = "macos")))]
        {
            let mut paths = Vec::new();
            let mut entries = 0usize;
            for root in &roots {
                for entry in walkdir::WalkDir::new(root)
                    .follow_links(false)
                    .into_iter()
                    .filter_entry(|entry| {
                        entry.depth() == 0
                            || !entry.file_type().is_dir()
                            || !self.file_watch_excluded(entry.path(), true)
                    })
                {
                    let entry = entry.map_err(|error| error.to_string())?;
                    let limit = self.file_watch_limits.entries;
                    if limit != 0 && entries >= limit {
                        return Err(format!(
                            "fileWatchMaxEntries={limit} exceeded; increase it or use 0 for unlimited"
                        ));
                    }
                    entries = entries.saturating_add(1);
                    if entry.file_type().is_dir() {
                        let limit = self.file_watch_limits.directories;
                        if limit != 0 && paths.len() >= limit {
                            return Err(format!(
                                "fileWatchMaxDirectories={limit} exceeded; increase it or use 0 for unlimited"
                            ));
                        }
                        paths.push(entry.into_path());
                    }
                }
            }
            paths.sort();
            paths.dedup();
            Ok(paths)
        }
    }
}
