//! SDK discovery and import-driven, declaration-only standard-library loading.
use super::Workspace;
use super::goenv::GoEnv;
use serde_json::Value;
use std::collections::{HashSet, VecDeque};
use std::fs;
use std::path::{Component, Path, PathBuf};
use walkdir::WalkDir;

/// gopls' preferred ports for a file whose constraints do not match the folder
/// environment. First matching port selects the orphaned file's view, so the
/// order is significant.
const PREFERRED_PORTS: &[(&str, &str)] = &[
    // First class ports.
    ("darwin", "amd64"),
    ("darwin", "arm64"),
    ("linux", "amd64"),
    ("linux", "arm64"),
    ("windows", "amd64"),
    ("linux", "arm"),
    ("linux", "386"),
    ("windows", "386"),
    // Secondary ports.
    ("aix", "ppc64"),
    ("dragonfly", "amd64"),
    ("freebsd", "386"),
    ("freebsd", "amd64"),
    ("freebsd", "arm"),
    ("freebsd", "arm64"),
    ("illumos", "amd64"),
    ("linux", "ppc64"),
    ("linux", "ppc64le"),
    ("linux", "mips"),
    ("linux", "mipsle"),
    ("linux", "mips64"),
    ("linux", "mips64le"),
    ("linux", "riscv64"),
    ("linux", "s390x"),
    ("android", "386"),
    ("android", "amd64"),
    ("android", "arm"),
    ("android", "arm64"),
    ("ios", "arm64"),
    ("ios", "amd64"),
    ("js", "wasm"),
    ("netbsd", "386"),
    ("netbsd", "amd64"),
    ("netbsd", "arm"),
    ("netbsd", "arm64"),
    ("openbsd", "386"),
    ("openbsd", "amd64"),
    ("openbsd", "arm"),
    ("openbsd", "arm64"),
    ("openbsd", "mips64"),
    ("plan9", "386"),
    ("plan9", "amd64"),
    ("plan9", "arm"),
    ("solaris", "amd64"),
    ("windows", "arm"),
    ("windows", "arm64"),
    ("freebsd", "riscv64"),
    ("linux", "loong64"),
    ("linux", "sparc64"),
    ("openbsd", "ppc64"),
    ("openbsd", "riscv64"),
    ("wasip1", "wasm"),
];

const MAX_ORPHAN_IMPORT_FILES: usize = 16;
const MAX_ORPHAN_IMPORT_BYTES: usize = 4 * 1024 * 1024;

/// Resolves the SDK source root (`<goroot>/src`), in priority order:
/// `initializationOptions.goroot` (explicit, including `""` to disable), the
/// toolchain's `go env GOROOT`, the `GOROOT` environment variable and finally the
/// installation layout of `go` on `PATH`.
pub(super) fn discover(options: &Value, go_env: &GoEnv) -> Result<Option<PathBuf>, String> {
    if let Some(value) = options.get("goroot").filter(|value| !value.is_null()) {
        let value = value.as_str().ok_or("goroot must be a string")?;
        if value.is_empty() {
            return Ok(None);
        }
        return source_root(Path::new(value))
            .map(Some)
            .ok_or_else(|| "goroot must name an absolute Go SDK directory".into());
    }
    let candidates = [
        go_env.goroot.clone(),
        std::env::var_os("GOROOT")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from),
        installation_on_path(),
    ];
    Ok(candidates
        .into_iter()
        .flatten()
        .find_map(|root| source_root(&root)))
}

/// The source directory of an absolute SDK root that contains the standard library.
fn source_root(root: &Path) -> Option<PathBuf> {
    let source = root.join("src");
    (root.is_absolute() && source.join("builtin/builtin.go").is_file()).then_some(source)
}

/// The Go installation containing the `go` executable on `PATH`.
fn installation_on_path() -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths).find_map(|directory| {
        let binary = directory.join(if cfg!(windows) { "go.exe" } else { "go" });
        binary
            .is_file()
            .then(|| directory.parent().map(Path::to_path_buf))
            .flatten()
    })
}

fn package_path(root: &Path, import: &str) -> Option<PathBuf> {
    // Module paths have a dot in their first element; never interpret traversal,
    // rooted paths or Windows separators as standard-library imports.
    if import.is_empty() || import.split('/').next()?.contains('.') || import.contains('\\') {
        return None;
    }
    let relative = Path::new(import);
    if !relative
        .components()
        .all(|part| matches!(part, Component::Normal(_)))
        || import
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return None;
    }
    Some(root.join(relative))
}

fn vendor_packages(vendor: &Path) -> HashSet<String> {
    let mut packages = HashSet::new();
    for entry in WalkDir::new(vendor)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            !entry.file_type().is_dir()
                || !entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.starts_with(['.', '_']) || name == "testdata")
        })
        .filter_map(Result::ok)
    {
        if entry.file_type().is_file() && entry.path().extension().is_some_and(|ext| ext == "go") {
            let directory = entry.path().parent().expect("source directory");
            if let Ok(relative) = directory.strip_prefix(vendor) {
                let import = relative
                    .components()
                    .map(|part| part.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/");
                if packages.len() == 1024 && !packages.contains(&import) {
                    eprintln!("SDK vendor inventory exceeds 1024 packages");
                    break;
                }
                packages.insert(import);
            }
        }
    }
    packages
}

impl Workspace {
    pub(crate) fn builtin_file(&self) -> Option<std::path::PathBuf> {
        Some(self.sdk_source_root()?.join("builtin/builtin.go"))
    }

    fn sdk_source_root(&self) -> Option<&Path> {
        self.modules
            .iter()
            .find_map(|(root, module)| (module == "std").then_some(root.as_path()))
            .or_else(|| {
                self.modules.iter().find_map(|(root, module)| {
                    let source = root.parent()?;
                    (module == "cmd" && source.join("builtin/builtin.go").is_file())
                        .then_some(source)
                })
            })
            .or(self.sdk_root.as_deref())
    }

    pub(super) fn configure_sdk_vendor(&mut self) {
        self.sdk_vendor.clear();
        self.sdk_cmd_vendor.clear();
        let Some(root) = self.sdk_source_root().map(Path::to_path_buf) else {
            return;
        };
        self.sdk_vendor = vendor_packages(&root.join("vendor"));
        if self
            .modules
            .get(&root.join("cmd"))
            .is_some_and(|module| module == "cmd")
        {
            self.sdk_cmd_vendor = vendor_packages(&root.join("cmd/vendor"));
        }
    }

    pub(super) fn sdk_owner(&self, file: &Path) -> Option<&str> {
        let source = self.sdk_source_root()?;
        self.modules
            .iter()
            .filter(|(root, _)| file.starts_with(root))
            .max_by_key(|(root, _)| root.components().count())
            .and_then(|(root, module)| {
                ((module == "std" && root == source)
                    || (module == "cmd" && *root == source.join("cmd")))
                .then_some(module.as_str())
            })
    }

    fn sdk_file(&self, file: &Path) -> bool {
        self.sdk_owner(file).is_some()
    }

    /// Canonicalize only SDK-owned imports. std and cmd have separate vendor
    /// copies; ordinary modules retain independent module identities.
    pub(crate) fn sdk_import_identity(&self, file: &Path, import: &mut String) {
        match self.sdk_owner(file) {
            Some("cmd") if self.sdk_cmd_vendor.contains(import) => {
                import.insert_str(0, "cmd/vendor/")
            }
            Some("std") if self.sdk_vendor.contains(import) => import.insert_str(0, "vendor/"),
            _ => {}
        }
    }

    fn sdk_dependency(&self, root: &Path, file: &Path, import: &str) -> Option<PathBuf> {
        if (import.starts_with("vendor/") || import.starts_with("cmd/vendor/"))
            && !self.sdk_file(file)
        {
            return None;
        }
        package_path(root, import)
    }

    pub(super) fn load_sdk(&mut self) {
        let Some(root) = self.sdk_source_root().map(Path::to_path_buf) else {
            return;
        };
        self.modules.insert(root.clone(), "std".into());
        let existing: HashSet<_> = self
            .files
            .keys()
            .filter(|file| !self.detached.contains(*file))
            .filter_map(|file| file.parent())
            .collect();
        let mut pending: VecDeque<_> = self
            .files
            .iter()
            .filter(|(file, _)| !self.detached.contains(*file))
            .flat_map(|(file, stub)| {
                stub.imports
                    .iter()
                    .filter_map(|import| self.sdk_dependency(&root, file, &import.path))
            })
            .filter(|directory| !existing.contains(directory.as_path()))
            .collect();
        let mut seen = HashSet::new();
        while let Some(directory) = pending.pop_front() {
            if !seen.insert(directory.clone())
                || self.sdk_packages.contains(&directory)
                || self
                    .files
                    .keys()
                    .filter(|file| !self.detached.contains(*file))
                    .any(|file| file.parent() == Some(directory.as_path()))
            {
                continue;
            }
            // Fixed SDK, bounded metadata; no source text or body arenas survive.
            if self.sdk_packages.len() >= 1024 {
                eprintln!("standard-library dependency limit exceeded (1024 packages)");
                break;
            }
            let Ok(entries) = fs::read_dir(&directory) else {
                continue;
            };
            if directory
                .ancestors()
                .take_while(|path| *path != root)
                .any(|path| {
                    path.join("go.mod").is_file()
                        && !(path == root.join("cmd")
                            && self.modules.get(path).is_some_and(|module| module == "cmd"))
                })
            {
                continue;
            }
            self.sdk_packages.insert(directory.clone());
            for entry in entries.filter_map(Result::ok) {
                let file = entry.path();
                if !entry.file_type().is_ok_and(|kind| kind.is_file())
                    || file.extension().is_none_or(|extension| extension != "go")
                {
                    continue;
                }
                if let Some(stub) = self.indexed_file(&file) {
                    pending.extend(
                        stub.imports
                            .iter()
                            .filter_map(|import| self.sdk_dependency(&root, &file, &import.path)),
                    );
                    self.files.insert(file, stub);
                }
            }
        }
        self.rebuild_package_names();
    }

    /// Effective view target of an orphaned file, mirroring gopls' dynamic
    /// preferred-port views. Returns None when the host target already matches
    /// or the file is not detached, so ordinary lookups keep their behavior.
    pub(crate) fn orphan_view_target(&self, file: &Path) -> Option<crate::buildconstraint::Target> {
        if !self.is_detached(file) {
            return None;
        }
        let name = file.file_name().and_then(|name| name.to_str())?;
        let source = self.source(file)?;
        if self.build_target.matches_file(name, &source) == Ok(true) {
            return None;
        }
        PREFERRED_PORTS.iter().find_map(|(goos, goarch)| {
            let target = self.view_target(goos, goarch);
            (target.matches_file(name, &source) == Ok(true)).then_some(target)
        })
    }

    /// Whether a dependency file belongs to the target view selected for an
    /// orphaned consumer. Sources are read through the existing bounded reader.
    pub(crate) fn file_matches_target(
        &self,
        file: &Path,
        target: &crate::buildconstraint::Target,
    ) -> bool {
        let Some(name) = file.file_name().and_then(|name| name.to_str()) else {
            return false;
        };
        if !target.matches_filename(name) {
            return false;
        }
        self.source(file)
            .is_some_and(|source| target.matches_file(name, &source) == Ok(true))
    }

    /// Load the imported SDK package variants selected by an orphaned file's own
    /// build constraints. Already indexed host variants stay untouched; missing
    /// target-compatible files are read and indexed as detached metadata so they
    /// never participate in main-view packages, references or workspace symbols.
    /// Bounded to 16 files / 4 MiB per detached buffer.
    pub(super) fn load_detached_import_variants(&mut self, file: &Path) {
        let Some(target) = self.orphan_view_target(file) else {
            return;
        };
        let Some(root) = self.sdk_source_root().map(Path::to_path_buf) else {
            return;
        };
        let imports: Vec<String> = self
            .files
            .get(file)
            .map(|stub| {
                stub.imports
                    .iter()
                    .map(|import| import.path.clone())
                    .collect()
            })
            .unwrap_or_default();
        let mut remaining_files = MAX_ORPHAN_IMPORT_FILES;
        let mut remaining_bytes = MAX_ORPHAN_IMPORT_BYTES;
        for import in imports {
            if remaining_files == 0 || remaining_bytes == 0 {
                break;
            }
            let Some(directory) = self.sdk_dependency(&root, file, &import) else {
                continue;
            };
            let Ok(entries) = fs::read_dir(&directory) else {
                continue;
            };
            if self.sdk_packages.len() >= 1024 {
                eprintln!("standard-library dependency limit exceeded (1024 packages)");
                break;
            }
            self.sdk_packages.insert(directory.clone());
            let mut candidates: Vec<PathBuf> = entries
                .filter_map(Result::ok)
                .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
                .map(|entry| entry.path())
                .filter(|path| path.extension().is_some_and(|ext| ext == "go"))
                .filter(|path| !self.files.contains_key(path))
                .collect();
            candidates.sort();
            for path in candidates {
                if remaining_files == 0 || remaining_bytes == 0 {
                    break;
                }
                let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                    continue;
                };
                if !target.matches_filename(name) {
                    continue;
                }
                let Ok(source) = fs::read_to_string(&path) else {
                    continue;
                };
                if source.len() > remaining_bytes {
                    continue;
                }
                let Some(stub) = self.orphan_file_from(&path, &source, &target) else {
                    continue;
                };
                remaining_files -= 1;
                remaining_bytes -= source.len();
                self.files.insert(path.clone(), stub);
                self.detached.insert(path);
            }
        }
    }

    /// Index a dependency variant that only an orphaned file's view selects.
    /// Uses the same import identity/cgo rules as ordinary indexing.
    fn orphan_file_from(
        &self,
        file: &Path,
        source: &str,
        target: &crate::buildconstraint::Target,
    ) -> Option<crate::index::IndexedFile> {
        if source.len() as u64 > crate::index::MAX_FILE_BYTES {
            return None;
        }
        let name = file.file_name().and_then(|name| name.to_str())?;
        if target.matches_file(name, source) != Ok(true) {
            return None;
        }
        let mut stub = crate::index::index_source(source);
        if self.cgo_disabled && stub.imports.iter().any(|import| import.path == "C") {
            return None;
        }
        for import in &mut stub.imports {
            self.sdk_import_identity(file, &mut import.path);
        }
        stub.rebuild_import_filter();
        Some(stub)
    }
}
