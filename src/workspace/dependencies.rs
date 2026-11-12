//! Offline dependency mounts and import-directed declaration loading.
use super::Workspace;
use super::goenv::GoEnv;
use crate::index::IndexedFile;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// Module cache root, preferring the toolchain's own configuration. There is no
/// initialization option: `go env GOMODCACHE`, then the environment, then the
/// first `GOPATH` entry and finally the per-user default. Relative candidates are
/// ignored because cache mounts must be absolute.
pub(super) fn cache_root(go_env: &GoEnv) -> Option<PathBuf> {
    [
        go_env.gomodcache.clone(),
        std::env::var_os("GOMODCACHE")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from),
        std::env::var_os("GOPATH")
            .and_then(|value| std::env::split_paths(&value).next())
            .map(|path| path.join("pkg/mod")),
        std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
            .map(|home| PathBuf::from(home).join("go/pkg/mod")),
    ]
    .into_iter()
    .flatten()
    .find(|path| path.is_absolute())
}

pub(super) fn cached_directory(cache: &Path, name: &str, version: &str) -> Option<PathBuf> {
    crate::gomod::compare_versions(version, version)?;
    Some(cache.join(format!(
        "{}@{}",
        crate::gomod::escape_cache_path(name)?,
        crate::gomod::escape_cache_path(version)?
    )))
}

/// Indexed files grouped by package directory for one incremental mount pass.
/// Only previously loaded mount directories are collected: root files are handled
/// by the seed pass and do not need a directory snapshot.
fn mount_files_by_directory(
    files: &BTreeMap<PathBuf, IndexedFile>,
    previous: &HashSet<PathBuf>,
) -> HashMap<PathBuf, Vec<PathBuf>> {
    let mut directories: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
    for file in files.keys() {
        let Some(directory) = file.parent() else {
            continue;
        };
        if previous.contains(directory) {
            directories
                .entry(directory.to_path_buf())
                .or_default()
                .push(file.clone());
        }
    }
    directories
}

/// Bound on the refresh-scoped import→directory memo. Reached only by
/// pathological import counts; beyond the bound resolution stays correct but
/// uncached. The map is dropped at the end of the pass and is never retained.
pub(super) const MAX_IMPORT_MEMO: usize = 65_536;

/// Refresh-scoped resolver from a decoded import path to the package directory
/// served by an offline module mount. Built from the installed `modules`/`roots`
/// state; a longest module-path prefix is found by walking `/` boundaries and
/// successful/failed lookups are memoized for the pass.
struct MountIndex<'a> {
    by_path: HashMap<&'a str, (&'a Path, &'a str)>,
    roots: &'a [PathBuf],
    resolved: HashMap<String, Option<Rc<PathBuf>>>,
    memo_bytes: usize,
}

/// Queue one resolved package directory exactly once per pass. Shared directory
/// handles are reference-counted so repeated imports do not allocate a `PathBuf`
/// for every occurrence.
fn queue_directory(
    pending: &mut VecDeque<Rc<PathBuf>>,
    queued: &mut HashSet<Rc<PathBuf>>,
    directory: Option<Rc<PathBuf>>,
) {
    if let Some(directory) = directory
        && queued.insert(directory.clone())
    {
        pending.push_back(directory);
    }
}

impl<'a> MountIndex<'a> {
    fn new(modules: &'a HashMap<PathBuf, String>, roots: &'a [PathBuf]) -> Self {
        let mut by_path = HashMap::with_capacity(modules.len().min(4096));
        for (root, module) in modules {
            // Module paths are unique across an installed plan: duplicate
            // workspace/main modules are rejected during planning and dependency
            // selection keys modules by name.
            by_path
                .entry(module.as_str())
                .or_insert((root.as_path(), module.as_str()));
        }
        Self {
            by_path,
            roots,
            resolved: HashMap::new(),
            memo_bytes: 0,
        }
    }

    fn resolve(&mut self, import: &str) -> Option<Rc<PathBuf>> {
        if let Some(directory) = self.resolved.get(import) {
            return directory.clone();
        }
        let directory = self.lookup(import).map(Rc::new);
        if self.resolved.len() < MAX_IMPORT_MEMO {
            self.memo_bytes += import.len()
                + directory
                    .as_ref()
                    .map_or(0, |directory| directory.as_os_str().len());
            self.resolved.insert(import.to_owned(), directory.clone());
        }
        directory
    }

    fn lookup(&self, import: &str) -> Option<PathBuf> {
        let mut end = import.len();
        loop {
            let prefix = &import[..end];
            if let Some((root, module)) = self.by_path.get(prefix) {
                if *module == "std" || self.roots.iter().any(|candidate| candidate == *root) {
                    return None;
                }
                let suffix = import[prefix.len()..].trim_start_matches('/');
                if suffix
                    .split('/')
                    .any(|part| part == ".." || part == "." || part.contains(['\\', ':']))
                {
                    return None;
                }
                let directory = root.join(suffix);
                if directory
                    .ancestors()
                    .take_while(|path| *path != *root)
                    .any(|path| path.join("go.mod").is_file())
                {
                    return None;
                }
                return Some(directory);
            }
            match import[..end].rfind('/') {
                Some(position) => end = position,
                None => return None,
            }
        }
    }

    fn entries(&self) -> usize {
        self.resolved.len()
    }

    fn bytes(&self) -> usize {
        self.memo_bytes
    }
}

/// Whether a retained stub is owned by a mount package directory (never a root
/// file): pruning must not remove explicitly scanned roots.
fn mount_file(workspace: &Workspace, file: &Path) -> bool {
    !workspace.nested_files.contains(file)
        && !workspace.roots.iter().any(|root| file.starts_with(root))
}

impl Workspace {
    fn dependency_directory(&self, import: &str) -> Option<PathBuf> {
        let (root, module) = self
            .modules
            .iter()
            .filter(|(_, module)| {
                import == module.as_str()
                    || import
                        .strip_prefix(module.as_str())
                        .is_some_and(|suffix| suffix.starts_with('/'))
            })
            .max_by_key(|(_, module)| module.len())?;
        if self.roots.contains(root) || module == "std" {
            return None;
        }
        let suffix = import.strip_prefix(module)?.trim_start_matches('/');
        if suffix
            .split('/')
            .any(|part| part == ".." || part == "." || part.contains(['\\', ':']))
        {
            return None;
        }
        let directory = root.join(suffix);
        if directory
            .ancestors()
            .take_while(|path| *path != root)
            .any(|path| path.join("go.mod").is_file())
        {
            return None;
        }
        Some(directory)
    }

    pub(super) fn load_dependencies(&mut self) {
        let mut pending: VecDeque<_> = self
            .files
            .values()
            .flat_map(|stub| {
                stub.imports
                    .iter()
                    .filter_map(|import| self.dependency_directory(&import.path))
            })
            .collect();
        let mut seen = HashSet::new();
        while let Some(directory) = pending.pop_front() {
            if !seen.insert(directory.clone()) || self.dependency_packages.contains(&directory) {
                continue;
            }
            if self.dependency_packages.len() >= 4096 {
                eprintln!("module dependency package limit exceeded (4096)");
                break;
            }
            let Ok(entries) = fs::read_dir(&directory) else {
                continue;
            };
            self.dependency_packages.insert(directory);
            for entry in entries.filter_map(Result::ok) {
                let file = entry.path();
                if !entry.file_type().is_ok_and(|kind| kind.is_file())
                    || file.extension().is_none_or(|ext| ext != "go")
                {
                    continue;
                }
                if let Some(stub) = self.indexed_file(&file) {
                    pending.extend(
                        stub.imports
                            .iter()
                            .filter_map(|import| self.dependency_directory(&import.path)),
                    );
                    self.files.insert(file, stub);
                }
            }
        }
        self.rebuild_package_names();
    }

    /// Reconcile dependency mounts after a plan transition. Stubs under package
    /// directories that remain import-reachable are retained, newly reachable
    /// directories are read once, and every package that is no longer reachable
    /// (removed requirement, changed module version, changed replace target) is
    /// pruned so repeated remounts do not grow the index.
    ///
    /// `previous` is the inventory of the installed plan. Returns
    /// `(retained files, loaded files, pruned files)` for tracing and tests.
    pub(super) fn load_dependencies_incremental(
        &mut self,
        previous: HashSet<PathBuf>,
    ) -> (usize, usize, usize) {
        let started = std::time::Instant::now();
        let directories = mount_files_by_directory(&self.files, &previous);
        let snapshot_ms = started.elapsed();
        let mut mount = MountIndex::new(&self.modules, &self.roots);
        let mut pending: VecDeque<Rc<PathBuf>> = VecDeque::new();
        let mut queued: HashSet<Rc<PathBuf>> = HashSet::new();
        let seeds_started = std::time::Instant::now();
        for (file, stub) in &self.files {
            // Stale mount stubs must not seed reachability; root and open files
            // (including files of a root that replaced a mount) always seed.
            let stale = file
                .parent()
                .is_some_and(|parent| previous.contains(parent))
                && !self.roots.iter().any(|root| file.starts_with(root));
            if stale {
                continue;
            }
            for import in &stub.imports {
                queue_directory(&mut pending, &mut queued, mount.resolve(&import.path));
            }
        }
        let seeds_ms = seeds_started.elapsed();
        let graph_started = std::time::Instant::now();
        let mut expansion = std::time::Duration::ZERO;
        let mut expanded_files = 0usize;
        let mut expanded_imports = 0usize;
        let mut reachable: HashSet<Rc<PathBuf>> = HashSet::new();
        let mut loaded = 0usize;
        while let Some(directory) = pending.pop_front() {
            if reachable.len() >= 4096 {
                eprintln!("module dependency package limit exceeded (4096)");
                break;
            }
            if let Some(files) = directories.get(directory.as_path()) {
                // A retained package is trusted: its stubs were current when the
                // previous plan was installed and the event contract keeps them
                // current. Avoid a stat storm over every reachable mount.
                reachable.insert(directory.clone());
                let expansion_started = std::time::Instant::now();
                for file in files {
                    expanded_files += 1;
                    if self.nested_files.contains(file) {
                        continue;
                    }
                    if let Some(stub) = self.files.get(file) {
                        expanded_imports += stub.imports.len();
                        for import in &stub.imports {
                            queue_directory(&mut pending, &mut queued, mount.resolve(&import.path));
                        }
                    }
                }
                expansion += expansion_started.elapsed();
                continue;
            }
            if previous.contains(directory.as_path()) {
                // Loaded before with no indexable source: keep the empty package.
                reachable.insert(directory.clone());
                continue;
            }
            let Ok(entries) = fs::read_dir(directory.as_path()) else {
                continue;
            };
            reachable.insert(directory.clone());
            // `discoverable` admits production files in a known mount directory.
            self.dependency_packages.insert((*directory).clone());
            for entry in entries.filter_map(Result::ok) {
                let file = entry.path();
                if !entry.file_type().is_ok_and(|kind| kind.is_file())
                    || file.extension().is_none_or(|ext| ext != "go")
                    || !super::packages::production_file(&file)
                {
                    continue;
                }
                if let Some(stub) = self.indexed_file(&file) {
                    for import in &stub.imports {
                        queue_directory(&mut pending, &mut queued, mount.resolve(&import.path));
                    }
                    self.files.insert(file, stub);
                    loaded += 1;
                }
            }
        }
        let reachable: HashSet<PathBuf> = reachable
            .into_iter()
            .map(|directory| (*directory).clone())
            .collect();
        let graph_ms = graph_started.elapsed();
        let prune_started = std::time::Instant::now();
        if self.trace.modules {
            eprintln!(
                "modules: dependency import map {} entries ({} bytes)",
                mount.entries(),
                mount.bytes()
            );
        }
        drop(mount);
        let stale: Vec<PathBuf> = self
            .files
            .keys()
            .filter(|file| {
                file.parent()
                    .is_some_and(|parent| previous.contains(parent) && !reachable.contains(parent))
                    && mount_file(self, file)
            })
            .cloned()
            .collect();
        let pruned = stale.len();
        for file in stale {
            self.files.remove(&file);
        }
        let retained = self
            .files
            .keys()
            .filter(|file| {
                file.parent()
                    .is_some_and(|parent| previous.contains(parent) && reachable.contains(parent))
            })
            .count();
        let prune_ms = prune_started.elapsed();
        if self.trace.modules {
            eprintln!(
                "modules: dependency phases snapshot {} ms seeds {} ms graph {} ms prune {} ms (expanded {} files/{} imports in {} us)",
                snapshot_ms.as_millis(),
                seeds_ms.as_millis(),
                graph_ms.as_millis(),
                prune_ms.as_millis(),
                expanded_files,
                expanded_imports,
                expansion.as_micros()
            );
        }
        self.dependency_packages = reachable;
        // Derived package names are rebuilt once by the plan-install tail (after
        // nested views are restored); rebuilding here as well was redundant.
        (retained, loaded, pruned)
    }
}
