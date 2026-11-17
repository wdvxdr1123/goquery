//! Event-owned orphan view admission and fixed-size build membership metadata.
use super::Workspace;
use std::collections::{HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};

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

pub(super) const MAX_VIEW_DIRECTORY_ENTRIES: usize = 2048;
const _: () = assert!(PREFERRED_PORTS.len() < 63);

/// Immutable request build context. The workspace owns both targets and paths.
#[derive(Clone, Copy)]
pub(crate) struct BuildView<'a> {
    root: Option<&'a Path>,
    target: &'a crate::buildconstraint::Target,
    bit: usize,
    detached: bool,
}

impl<'a> BuildView<'a> {
    pub(crate) fn visible(self, workspace: &Workspace, candidate: &Path) -> bool {
        if self.root == Some(candidate) {
            return true;
        }
        if !self.detached {
            return !workspace.is_detached(candidate);
        }
        workspace
            .files
            .get(candidate)
            .is_some_and(|stub| stub.build_targets & (1 << self.bit) != 0)
    }

    pub(crate) fn includes_detached(self) -> bool {
        self.detached
    }

    pub(crate) fn gc_sizes(self) -> Option<(i64, i64)> {
        super::configuration::gc_sizes(self.target.goarch())
    }

    pub(crate) fn integer_bits(self) -> Option<u32> {
        self.gc_sizes().map(|(word, _)| (word * 8) as u32)
    }

    pub(crate) fn origin_for(self, workspace: &Workspace, file: &Path) -> Option<&'a Path> {
        let file_view = workspace.build_view(Some(file));
        (self.bit != file_view.bit || self.detached != file_view.detached)
            .then_some(self.root)
            .flatten()
    }
}

impl Workspace {
    pub(crate) fn build_view(&self, file: Option<&Path>) -> BuildView<'_> {
        let root = file.and_then(|file| {
            self.files
                .get_key_value(file)
                .map(|(path, _)| path.as_path())
        });
        let index = root.and_then(|file| self.orphan_view_index(file));
        BuildView {
            root,
            target: index
                .and_then(|index| self.view_targets.get(index))
                .unwrap_or(&self.build_target),
            bit: index.map_or(0, |index| index + 1),
            detached: root.is_some_and(|file| self.is_detached(file)),
        }
    }

    /// A lone excluded file still has its own orphan package. An excluded file
    /// next to an admitted package needs a usable host or preferred-port view.
    pub(crate) fn orphan_metadata_available(&self, file: &Path) -> bool {
        if !self.is_detached(file)
            || self
                .files
                .get(file)
                .is_some_and(|stub| stub.build_targets != 0)
        {
            return true;
        }
        let Some(directory) = file.parent() else {
            return true;
        };
        !self
            .files
            .range::<Path, _>((
                std::ops::Bound::Included(directory),
                std::ops::Bound::Unbounded,
            ))
            .take_while(|(path, _)| path.starts_with(directory))
            .any(|(path, _)| path.as_path() != file && path.parent() == Some(directory))
    }
    pub(super) fn configure_view_targets(&mut self) {
        self.view_targets = PREFERRED_PORTS
            .iter()
            .map(|(goos, goarch)| self.view_target(goos, goarch))
            .collect();
    }

    pub(super) fn build_membership(&self, file: &Path, source: &str) -> u64 {
        let Some(name) = file.file_name().and_then(|name| name.to_str()) else {
            return 0;
        };
        crate::buildconstraint::matching_targets(
            name,
            source,
            std::iter::once(&self.build_target).chain(self.view_targets.iter()),
        )
        .unwrap_or(0)
    }

    /// A consumer's selected view is metadata, never a source query.
    fn orphan_view_index(&self, file: &Path) -> Option<usize> {
        if !self.is_detached(file) {
            return None;
        }
        let mask = self.files.get(file)?.build_targets;
        if mask & 1 != 0 {
            return None;
        }
        self.view_targets
            .iter()
            .enumerate()
            .find_map(|(index, _)| (mask & (1 << (index + 1)) != 0).then_some(index))
    }

    /// Visibility of admitted files in this consumer's selected build view.
    pub(crate) fn view_file_visible(&self, from: &Path, candidate: &Path) -> bool {
        self.build_view(Some(from)).visible(self, candidate)
    }

    /// Effective view target of an orphaned file, mirroring gopls' dynamic
    /// preferred-port views. Returns None when the host target already matches
    /// or the file is not detached, so ordinary lookups keep their behavior.
    pub(crate) fn orphan_view_target(&self, file: &Path) -> Option<crate::buildconstraint::Target> {
        self.view_targets
            .get(self.orphan_view_index(file)?)
            .cloned()
    }

    /// Whether a dependency file belongs to the target view selected for an
    /// orphaned consumer, using only precomputed target membership.
    pub(crate) fn file_matches_target(
        &self,
        file: &Path,
        target: &crate::buildconstraint::Target,
    ) -> bool {
        let index = if target == &self.build_target {
            0
        } else {
            let Some(index) = self
                .view_targets
                .iter()
                .position(|candidate| candidate == target)
            else {
                return false;
            };
            index + 1
        };
        self.files
            .get(file)
            .is_some_and(|stub| stub.build_targets & (1 << index) != 0)
    }

    /// Admit variant metadata only from directories selected by the offline plan.
    /// One open-buffer event shares 16 files / 4 MiB and 128 import edges / 16 KiB.
    pub(super) fn load_detached_import_variants(&mut self, file: &Path) {
        let Some(target) = self.orphan_view_target(file) else {
            return;
        };
        let mut pending = PendingImports::new();
        if let Some(stub) = self.files.get(file) {
            pending.extend(&stub.imports);
        }
        let mut seen = HashSet::new();
        let mut remaining_files = MAX_ORPHAN_IMPORT_FILES;
        let mut remaining_bytes = MAX_ORPHAN_IMPORT_BYTES;
        let sdk_root = self.sdk_source_root().map(Path::to_path_buf);
        while let Some(import) = pending.queue.pop_front() {
            if remaining_files == 0 || remaining_bytes == 0 {
                pending.limited = true;
                break;
            }
            let Some(directory) = self.variant_import_directory(&import) else {
                continue;
            };
            if !seen.insert(directory.clone()) {
                continue;
            }
            let is_sdk = sdk_root
                .as_ref()
                .is_some_and(|root| directory.starts_with(root));
            if is_sdk {
                if self.sdk_packages.len() >= 1024 && !self.sdk_packages.contains(&directory) {
                    pending.limited = true;
                    break;
                }
                self.sdk_packages.insert(directory.clone());
            } else if !self.roots.iter().any(|root| directory.starts_with(root)) {
                if self.dependency_packages.len() >= 4096
                    && !self.dependency_packages.contains(&directory)
                {
                    pending.limited = true;
                    break;
                }
                self.dependency_packages.insert(directory.clone());
            }
            for (_, stub) in self
                .files
                .range::<Path, _>((
                    std::ops::Bound::Included(directory.as_path()),
                    std::ops::Bound::Unbounded,
                ))
                .take_while(|(path, _)| path.starts_with(&directory))
                .filter(|(path, _)| {
                    path.parent() == Some(directory.as_path())
                        && super::production_file(path)
                        && self.file_matches_target(path, &target)
                })
            {
                pending.extend(&stub.imports);
            }
            let Ok(mut entries) = fs::read_dir(&directory) else {
                continue;
            };
            let mut candidates: Vec<PathBuf> = entries
                .by_ref()
                .take(MAX_VIEW_DIRECTORY_ENTRIES)
                .filter_map(Result::ok)
                .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
                .map(|entry| entry.path())
                .filter(|path| {
                    path.extension().is_some_and(|ext| ext == "go")
                        && super::production_file(path)
                        && !self.files.contains_key(path)
                })
                .collect();
            pending.limited |= entries.next().is_some();
            candidates.sort();
            for path in candidates {
                if remaining_files == 0 || remaining_bytes == 0 {
                    pending.limited = true;
                    break;
                }
                let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                    continue;
                };
                if !target.matches_filename(name) {
                    continue;
                }
                let Some(source) = self.source_with_limit(&path, remaining_bytes) else {
                    continue;
                };
                remaining_bytes -= source.len();
                let Some(stub) = self.orphan_file_from(&path, &source, &target) else {
                    continue;
                };
                remaining_files -= 1;
                pending.extend(&stub.imports);
                self.files.insert(path.clone(), stub);
                self.detached.insert(path);
            }
        }
        if pending.limited {
            eprintln!(
                "orphan view: dependency metadata incomplete for {} (limits: 16 files, 4 MiB, 128 import edges, 16 KiB import paths, 2048 entries/directory)",
                file.display()
            );
        }
    }

    fn variant_import_directory(&self, import: &str) -> Option<PathBuf> {
        let mut candidates = self.modules.iter().filter_map(|(root, module)| {
            let suffix = if module == "std" {
                import
            } else if import == module {
                ""
            } else {
                import.strip_prefix(module.as_str())?.strip_prefix('/')?
            };
            let directory = root.join(suffix);
            (self.directory_import_path(&directory).as_deref() == Some(import)).then_some(directory)
        });
        let first = candidates.next()?;
        candidates
            .all(|directory| directory == first)
            .then_some(first)
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
        stub.build_targets = self.build_membership(file, source);
        stub.stamp = crate::index::DiskStamp::of(file);
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

/// Fixed total queue admission, including already-visited import edges.
struct PendingImports {
    queue: VecDeque<String>,
    edges: usize,
    bytes: usize,
    limited: bool,
}

impl PendingImports {
    fn new() -> Self {
        Self {
            queue: VecDeque::new(),
            edges: 128,
            bytes: 16 * 1024,
            limited: false,
        }
    }
    fn extend(&mut self, imports: &[crate::syntax::Import]) {
        for import in imports {
            if self.edges == 0 || import.path.len() > self.bytes {
                self.limited = true;
                break;
            }
            self.edges -= 1;
            self.bytes -= import.path.len();
            self.queue.push_back(import.path.clone());
        }
    }
}
