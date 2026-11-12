//! Consistent discovery rules for traversal, editor buffers and file events.
use super::Workspace;
use super::modules;
use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

pub(super) fn ignored_directory(name: &OsStr) -> bool {
    name.to_str().is_some_and(|name| {
        name.starts_with(['.', '_'])
            || matches!(name, "vendor" | "testdata" | "node_modules" | "target")
    })
}

/// How much of the previous index one plan installation may reuse.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum PlanInstall {
    /// Clear and re-walk every root. Used at initialization, for workspace-folder
    /// replacement and for watched batches that could not be applied file-by-file.
    Full,
    /// Reuse `IndexedFile` stubs under roots shared by the previous and the new
    /// plan and walk only added roots plus module directories whose selection
    /// changed. Used for manifest edits that are fully represented in a
    /// `modules::Plan` (main-module `go.mod`/`go.work` changes).
    Incremental,
}

/// Root transition between two module plans.
#[derive(Debug, Default, PartialEq, Eq)]
struct RootDelta {
    /// Roots walked by both plans and therefore reused by an incremental install.
    retained: Vec<PathBuf>,
    /// Roots only the new plan walks.
    added: Vec<PathBuf>,
    /// Roots only the previous plan walked.
    removed: Vec<PathBuf>,
}

impl RootDelta {
    fn new(old_roots: &[PathBuf], new_roots: &[PathBuf]) -> Self {
        let old: HashSet<&Path> = old_roots.iter().map(PathBuf::as_path).collect();
        let new: HashSet<&Path> = new_roots.iter().map(PathBuf::as_path).collect();
        let mut delta = Self::default();
        for root in new_roots {
            if old.contains(root.as_path()) {
                delta.retained.push(root.clone());
            } else {
                delta.added.push(root.clone());
            }
        }
        for root in old_roots {
            if !new.contains(root.as_path()) {
                delta.removed.push(root.clone());
            }
        }
        delta
    }

    /// Whether a newly added root already walks `path` (it is the root or below).
    fn covers(&self, path: &Path) -> bool {
        self.added.iter().any(|root| path.starts_with(root))
    }
}

/// Previous plan inputs that incremental installation classifies stubs by.
struct PreviousPlan {
    roots: Vec<PathBuf>,
    modules: HashMap<PathBuf, String>,
    excluded: Vec<PathBuf>,
    compiler_checkout: bool,
    sdk_vendor: HashSet<String>,
    sdk_cmd_vendor: HashSet<String>,
    /// Package directories that were loaded as offline dependency mounts.
    dependency_packages: HashSet<PathBuf>,
    /// Package directories that were loaded from the SDK.
    sdk_packages: HashSet<PathBuf>,
    module_cache: Option<PathBuf>,
    sdk_root: Option<PathBuf>,
    /// Resolved `<goroot>/src` (or checkout equivalent) at the previous install.
    sdk_source: Option<PathBuf>,
}

/// Result of one incremental index installation, consumed by the shared tail.
struct InstallOutcome {
    retained: usize,
    walked: usize,
    /// Mount inventories can be reconciled instead of cleared and reloaded.
    reuse_mounts: bool,
    previous_dependencies: HashSet<PathBuf>,
    previous_sdk_packages: HashSet<PathBuf>,
}

/// One file's retain decision for an incremental install. A retained stub is
/// exactly what a full walk of the shared roots would produce: same root, same
/// build target and same source (events keep stubs current between refreshes).
/// Stubs under previous mount package directories are retained provisionally and
/// pruned by the incremental dependency/SDK pass when no longer reachable.
/// Detached orphan and additional-view stubs are rebuilt by their owners below.
fn retain_file(
    file: &Path,
    retained_roots: &[PathBuf],
    excluded: &HashSet<PathBuf>,
    detached: &HashSet<PathBuf>,
    nested: &HashSet<PathBuf>,
    mounts: Option<(&HashSet<PathBuf>, &HashSet<PathBuf>)>,
) -> bool {
    if detached.contains(file) || nested.contains(file) {
        return false;
    }
    if retained_roots.iter().any(|root| file.starts_with(root)) {
        if !excluded.iter().any(|directory| file.starts_with(directory)) {
            return true;
        }
        // An explicit root nested inside an excluded module stays indexed: a
        // full scan walks every root after clearing the exclusion set for the walk.
        return retained_roots
            .iter()
            .filter(|root| file.starts_with(root))
            .any(|root| excluded.iter().any(|directory| root.starts_with(directory)));
    }
    let Some((dependencies, sdk_packages)) = mounts else {
        return false;
    };
    file.parent()
        .is_some_and(|parent| dependencies.contains(parent) || sdk_packages.contains(parent))
}

/// Exclusions to carry across a plan transition, plus directories that must be
/// walked even though they are not new roots: modules that became selected and
/// former excluded directories whose `go.mod` disappeared. Nested-module
/// membership can only change here for directories the previous walk already
/// discovered, because creating or deleting a `go.mod` forces a full install.
fn transition_exclusions(
    old_excluded: &[PathBuf],
    old_modules: &HashMap<PathBuf, String>,
    new_modules: &HashMap<PathBuf, String>,
    retained_roots: &[PathBuf],
    delta: &RootDelta,
    manifest: impl Fn(&Path) -> bool,
) -> (HashSet<PathBuf>, Vec<PathBuf>) {
    let under_retained = |directory: &Path| {
        retained_roots
            .iter()
            .any(|root| directory.starts_with(root))
    };
    let mut excluded = HashSet::new();
    let mut walk = Vec::new();
    for directory in old_excluded {
        if !under_retained(directory) {
            continue;
        }
        if new_modules.contains_key(directory) {
            continue;
        }
        if !manifest(directory) {
            // The nested manifest disappeared: the directory is ordinary again.
            // Its files are indexed by walking it unless an added root covers it.
            if !delta.covers(directory) {
                walk.push(directory.clone());
            }
            continue;
        }
        excluded.insert(directory.clone());
    }
    for directory in old_modules.keys() {
        if new_modules.contains_key(directory) || !under_retained(directory) {
            continue;
        }
        // A module root that left the plan becomes a nested unselected module.
        if manifest(directory) {
            excluded.insert(directory.clone());
        }
    }
    for directory in new_modules.keys() {
        if old_modules.contains_key(directory) || delta.covers(directory) {
            continue;
        }
        // A newly selected module below a retained root is not covered by an
        // added root here only when the root itself was already walked.
        if under_retained(directory) {
            walk.push(directory.clone());
        }
    }
    (excluded, walk)
}

/// Collect every Go file below `root` using the shared traversal rules, recording
/// unselected nested modules as exclusions.
fn root_files(
    root: &Path,
    modules: &HashMap<PathBuf, String>,
    excluded: &mut HashSet<PathBuf>,
) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            if entry.depth() == 0 || !entry.file_type().is_dir() {
                return true;
            }
            if ignored_directory(entry.file_name()) {
                return false;
            }
            if entry.path().join("go.mod").is_file() {
                if !modules.contains_key(entry.path()) {
                    excluded.insert(entry.path().to_path_buf());
                }
                return false;
            }
            true
        })
        .filter_map(Result::ok)
    {
        if entry.file_type().is_file() && entry.path().extension().is_some_and(|ext| ext == "go") {
            files.push(entry.into_path());
        }
    }
    files
}

impl Workspace {
    pub(super) fn discoverable(&self, file: &Path) -> bool {
        if self
            .excluded_modules
            .iter()
            .any(|root| file.starts_with(root))
        {
            return false;
        }
        if self.sdk_packages.contains(file.parent().unwrap_or(file))
            || self
                .dependency_packages
                .contains(file.parent().unwrap_or(file))
        {
            return !file
                .file_name()
                .is_some_and(|name| name.to_string_lossy().ends_with("_test.go"));
        }
        let mut inside_workspace = false;
        for root in &self.roots {
            if let Ok(relative) = file.strip_prefix(root) {
                inside_workspace = true;
                if relative.parent().is_none_or(|parent| {
                    parent
                        .components()
                        .all(|part| !ignored_directory(part.as_os_str()))
                }) {
                    return true;
                }
            }
        }
        // Preserve explicitly opened scratch files outside the workspace. An
        // explicit nested workspace folder overrides exclusions in a parent.
        !inside_workspace && self.open.contains_key(file)
    }

    pub(super) fn scan_roots(&mut self, folders: Vec<PathBuf>) -> Result<(), String> {
        let started = std::time::Instant::now();
        // Planning is transactional: incomplete manifest edits keep the old index.
        let plan = self.module_plan(&folders)?;
        if self.trace.modules {
            eprintln!("modules: plan in {} ms", started.elapsed().as_millis());
        }
        self.install_plan(folders, plan, PlanInstall::Full)
    }

    /// Install a plan accepted from a main-module manifest edit, reusing stubs
    /// under shared roots. Falls back to a full rebuild when the retained index
    /// cannot be proven consistent (first plan, SDK identity change).
    pub(super) fn install_plan_incremental(
        &mut self,
        folders: Vec<PathBuf>,
        plan: modules::Plan,
    ) -> Result<(), String> {
        self.install_plan(folders, plan, PlanInstall::Incremental)
    }

    /// Install an accepted plan: rebuild the bounded root index, mounts and
    /// derived package metadata. The caller owns planning (and its transaction).
    pub(super) fn install_plan(
        &mut self,
        folders: Vec<PathBuf>,
        plan: modules::Plan,
        mode: PlanInstall,
    ) -> Result<(), String> {
        let started = std::time::Instant::now();
        let planned = std::time::Instant::now();
        let previous = (mode == PlanInstall::Incremental).then(|| self.take_previous_plan());
        self.plan_digest = Some(modules::plan_digest(&plan));
        self.folders = folders;
        self.roots = plan.roots;
        self.modules = plan.modules;
        self.main_modules = plan.mains;
        self.compiler_checkout = plan.compiler_checkout;
        self.configure_sdk_vendor();
        // SDK import identities are baked into retained stubs: a changed std/cmd
        // source root or vendor inventory needs a full re-index.
        let previous = previous.filter(|previous| self.reusable(previous));
        let outcome = match previous {
            Some(previous) => self.install_incremental(previous),
            None => InstallOutcome {
                retained: 0,
                walked: self.install_full(),
                reuse_mounts: false,
                previous_dependencies: HashSet::new(),
                previous_sdk_packages: HashSet::new(),
            },
        };
        let InstallOutcome {
            retained,
            walked,
            reuse_mounts,
            previous_dependencies,
            previous_sdk_packages,
        } = outcome;
        if self.trace.modules {
            if walked != 0 && mode == PlanInstall::Incremental {
                eprintln!(
                    "modules: incremental plan reused {retained} stub(s), walked {walked} root(s)"
                );
            }
            eprintln!(
                "modules: {} files indexed in {} ms",
                self.files.len(),
                planned.elapsed().as_millis()
            );
        }
        // Open buffers keep scratch-file behavior when a module leaves the graph.
        let open: Vec<_> = self
            .open
            .keys()
            .filter(|file| file.extension().is_some_and(|ext| ext == "go"))
            .cloned()
            .collect();
        for file in open {
            if let Some(stub) = self.indexed_file(&file) {
                self.files.insert(file, stub);
            }
        }
        let dependencies = std::time::Instant::now();
        if reuse_mounts {
            let dependency_started = std::time::Instant::now();
            let (dependency_retained, dependency_loaded, dependency_pruned) =
                self.load_dependencies_incremental(previous_dependencies);
            let dependency_ms = dependency_started.elapsed();
            let sdk_started = std::time::Instant::now();
            let (sdk_retained, sdk_loaded, sdk_pruned) =
                self.load_sdk_incremental(previous_sdk_packages);
            let sdk_ms = sdk_started.elapsed();
            if self.trace.modules {
                eprintln!(
                    "modules: mounts dependency packages {} files {dependency_retained}/{dependency_loaded}/{dependency_pruned}; sdk packages {} files {sdk_retained}/{sdk_loaded}/{sdk_pruned}",
                    self.dependency_packages.len(),
                    self.sdk_packages.len(),
                );
                eprintln!(
                    "modules: dependency graph load in {} ms; sdk graph load in {} ms",
                    dependency_ms.as_millis(),
                    sdk_ms.as_millis()
                );
            }
        } else {
            self.dependency_packages.clear();
            self.sdk_packages.clear();
            self.load_dependencies();
            self.load_sdk();
            if self.trace.modules {
                eprintln!(
                    "modules: mounts dependency packages {}; sdk packages {} (full reload)",
                    self.dependency_packages.len(),
                    self.sdk_packages.len(),
                );
            }
        }
        if self.trace.modules {
            eprintln!(
                "modules: dependency/SDK load in {} ms",
                dependencies.elapsed().as_millis()
            );
        }
        self.restore_nested_views();
        self.rebuild_package_names();
        if self.trace.modules {
            eprintln!(
                "modules: scan complete in {} ms",
                started.elapsed().as_millis()
            );
        }
        Ok(())
    }

    fn take_previous_plan(&mut self) -> PreviousPlan {
        let sdk_source = self.sdk_source_root().map(Path::to_path_buf);
        PreviousPlan {
            roots: std::mem::take(&mut self.roots),
            modules: std::mem::take(&mut self.modules),
            excluded: std::mem::take(&mut self.excluded_modules),
            compiler_checkout: self.compiler_checkout,
            sdk_vendor: std::mem::take(&mut self.sdk_vendor),
            sdk_cmd_vendor: std::mem::take(&mut self.sdk_cmd_vendor),
            dependency_packages: std::mem::take(&mut self.dependency_packages),
            sdk_packages: std::mem::take(&mut self.sdk_packages),
            module_cache: self.module_cache.clone(),
            sdk_root: self.sdk_root.clone(),
            sdk_source,
        }
    }

    /// Whether the previous index can be partially reused against the plan that
    /// was just installed. `previous` was taken before the new plan fields and
    /// SDK vendor inventory were recomputed.
    fn reusable(&self, previous: &PreviousPlan) -> bool {
        !previous.roots.is_empty()
            && previous.compiler_checkout == self.compiler_checkout
            && previous.sdk_vendor == self.sdk_vendor
            && previous.sdk_cmd_vendor == self.sdk_cmd_vendor
    }

    /// Whether previously loaded dependency/SDK package directories can be
    /// reconciled incrementally: mount origins must be explainable by the module
    /// graph, the cache/SDK roots must be unchanged, and compiler-checkout views
    /// keep their full reload (std/cmd packages are workspace roots there).
    fn mounts_reusable(&self, previous: &PreviousPlan) -> bool {
        !self.compiler_checkout
            && previous.module_cache == self.module_cache
            && previous.sdk_root == self.sdk_root
            && previous.sdk_source == self.sdk_source_root().map(Path::to_path_buf)
            && previous.dependency_packages.iter().all(|directory| {
                previous
                    .modules
                    .keys()
                    .any(|root| directory.starts_with(root))
            })
            && previous.sdk_packages.iter().all(|directory| {
                previous
                    .sdk_source
                    .as_ref()
                    .is_some_and(|root| directory.starts_with(root))
            })
    }

    /// Clear the index and walk every root. Returns the number of walked roots.
    fn install_full(&mut self) -> usize {
        self.files.clear();
        self.excluded_modules.clear();
        self.sdk_packages.clear();
        self.dependency_packages.clear();
        let mut excluded = HashSet::new();
        let mut files = Vec::new();
        for root in &self.roots {
            files.extend(root_files(root, &self.modules, &mut excluded));
        }
        for file in files {
            if let Some(stub) = self.indexed_file(&file) {
                self.files.insert(file, stub);
            }
        }
        self.excluded_modules = excluded.into_iter().collect();
        self.excluded_modules.sort();
        self.roots.len()
    }

    /// Reuse stubs under shared roots, walk added roots and module directories
    /// whose selection changed, and drop everything that left the plan. Mount
    /// stubs survive provisionally when the inventories are reconciliable; the
    /// dependency/SDK pass in the shared tail then prunes what is unreachable.
    /// Returns the retained stubs, walked roots and previous mount inventories.
    fn install_incremental(&mut self, previous: PreviousPlan) -> InstallOutcome {
        let reuse_mounts = self.mounts_reusable(&previous);
        let delta = RootDelta::new(&previous.roots, &self.roots);
        let (mut excluded, rescan) = transition_exclusions(
            &previous.excluded,
            &previous.modules,
            &self.modules,
            &delta.retained,
            &delta,
            |directory| directory.join("go.mod").is_file(),
        );
        let mut walk = delta.added.clone();
        walk.extend(rescan);
        walk.sort();
        walk.dedup();
        // Walking an ancestor covers descendants; keep only the outermost.
        let mut roots_to_walk: Vec<PathBuf> = Vec::new();
        for directory in walk {
            if !roots_to_walk.iter().any(|root| directory.starts_with(root)) {
                roots_to_walk.push(directory);
            }
        }
        // Detached orphan and additional-view stubs are rebuilt in the shared
        // tail; the sets themselves are owned by their admission paths.
        let detached = std::mem::take(&mut self.detached);
        let nested = std::mem::take(&mut self.nested_files);
        let mounts =
            reuse_mounts.then_some((&previous.dependency_packages, &previous.sdk_packages));
        self.files.retain(|file, _| {
            retain_file(file, &delta.retained, &excluded, &detached, &nested, mounts)
        });
        let retained = self.files.len();
        self.detached = detached;
        self.nested_files = nested;
        // A full install clears the exclusion set while walking roots; mirror
        // that so an explicit root below an excluded module still gets indexed.
        for root in &roots_to_walk {
            let files = root_files(root, &self.modules, &mut excluded);
            for file in files {
                if let Some(stub) = self.indexed_file(&file) {
                    self.files.insert(file, stub);
                }
            }
        }
        self.excluded_modules = excluded.into_iter().collect();
        self.excluded_modules.sort();
        InstallOutcome {
            retained,
            walked: roots_to_walk.len(),
            reuse_mounts,
            previous_dependencies: previous.dependency_packages,
            previous_sdk_packages: previous.sdk_packages,
        }
    }
}
