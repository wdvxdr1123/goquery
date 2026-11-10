//! Compact workspace ownership and package-directory metadata lookup.
use super::packages::PackageNames;

use crate::index::{IndexedFile, IndexedSymbol as Symbol};
use std::collections::{BTreeMap, HashMap};
use std::ops::Bound;
use std::path::{Path, PathBuf};

/// Debug switches are sampled once when the server workspace is constructed.
/// Queries and module refreshes only read these flags, never the environment.
pub(crate) struct TraceConfig {
    pub(crate) queries: bool,
    pub(crate) modules: bool,
}

impl Default for TraceConfig {
    fn default() -> Self {
        Self {
            queries: std::env::var_os("GOQUERY_TRACE_QUERIES").is_some(),
            modules: std::env::var_os("GOQUERY_TRACE_MODULES").is_some(),
        }
    }
}

#[derive(Default)]
pub struct Workspace {
    pub(crate) trace: TraceConfig,
    pub(crate) files: BTreeMap<PathBuf, IndexedFile>,
    pub(crate) modules: HashMap<PathBuf, String>,
    pub(crate) package_names: PackageNames,
    pub(super) open: HashMap<PathBuf, String>,
    pub(super) roots: Vec<PathBuf>,
    pub(super) folders: Vec<PathBuf>,
    pub(super) main_modules: std::collections::HashSet<PathBuf>,
    pub(super) compiler_checkout: bool,
    pub(super) module_cache: Option<PathBuf>,
    pub(super) dependency_packages: std::collections::HashSet<PathBuf>,
    pub(super) sdk_root: Option<PathBuf>,
    pub(super) sdk_packages: std::collections::HashSet<PathBuf>,
    pub(super) sdk_vendor: std::collections::HashSet<String>,
    pub(super) sdk_cmd_vendor: std::collections::HashSet<String>,
    pub(super) excluded_modules: Vec<PathBuf>,
    pub(super) build_target: crate::buildconstraint::Target,
    /// Sampled once from CGO_ENABLED at initialization: cgo-disabled builds
    /// exclude files importing "C", mirroring the Go build context.
    pub(super) cgo_disabled: bool,
    /// Open buffers admitted outside the build/discovery scope (gopls' orphaned
    /// files). They answer semantic queries against themselves but never join a
    /// real package view or workspace-wide scans.
    pub(super) detached: std::collections::HashSet<PathBuf>,
    /// Additional views for nested modules under a workspace root, keyed by the
    /// nested module root. Their files never become main-view sources.
    pub(super) nested_modules: HashMap<PathBuf, String>,
    /// Package directories loaded for the additional views (bounded).
    pub(super) nested_directories: std::collections::HashSet<PathBuf>,
    /// Files indexed only for an additional view. They are marked detached so
    /// workspace-wide scans, symbols and package views keep excluding them.
    pub(super) nested_files: std::collections::HashSet<PathBuf>,
    pub(super) nested_bytes: usize,
    pub(crate) completion_config: crate::features::CompletionConfig,
    pub(crate) semantic_config: crate::features::SemanticConfig,
    /// Client announced hierarchicalDocumentSymbolSupport at initialization.
    pub(crate) hierarchical_document_symbols: bool,
    /// Client document versions for open buffers; closed files stay absent.
    pub(super) versions: HashMap<PathBuf, i32>,
}
impl Workspace {
    pub(crate) fn document_version(&self, file: &Path) -> Option<i32> {
        self.versions.get(file).copied()
    }

    /// Configured SDK source root (`<goroot>/src`), from the `goroot` option,
    /// `go env GOROOT`, `GOROOT` or the installation layout of `go` on `PATH`.
    pub(crate) fn sdk_root_path(&self) -> Option<&Path> {
        self.sdk_root.as_deref()
    }

    pub(crate) fn set_document_version(&mut self, file: &Path, version: Option<i32>) {
        if let Some(version) = version {
            self.versions.insert(file.to_owned(), version);
        } else {
            self.versions.remove(file);
        }
    }

    pub(crate) fn is_detached(&self, file: &Path) -> bool {
        self.detached.contains(file)
    }

    pub(crate) fn symbols(&self, file: &Path) -> Vec<(&PathBuf, &Symbol)> {
        self.package_symbols(file).collect()
    }

    pub(crate) fn package_symbols<'a>(
        &'a self,
        file: &Path,
    ) -> impl Iterator<Item = (&'a PathBuf, &'a Symbol)> + use<'a> {
        self.package_files(file)
            .flat_map(|(path, stub)| stub.symbols.iter().map(move |symbol| (path, symbol)))
    }

    pub(crate) fn package_symbol_candidates<'a>(
        &'a self,
        file: &Path,
        probe: crate::bloom::Fingerprint,
    ) -> impl Iterator<Item = (&'a PathBuf, &'a Symbol)> + use<'a> {
        self.package_files(file)
            .filter(move |(_, stub)| stub.declarations.may_contain_fingerprint(probe))
            .flat_map(|(path, stub)| stub.symbols.iter().map(move |symbol| (path, symbol)))
    }

    /// Method names are absent from the top-level declaration filter. The
    /// source identifier filter can reject files without false negatives.
    pub(crate) fn package_method_candidates<'a>(
        &'a self,
        file: &Path,
        probe: crate::bloom::Fingerprint,
    ) -> impl Iterator<Item = (&'a PathBuf, &'a Symbol)> + use<'a> {
        self.package_files(file)
            .filter(move |(_, stub)| stub.identifiers.may_contain_fingerprint(probe))
            .flat_map(|(path, stub)| {
                stub.symbols
                    .iter()
                    .filter(|symbol| symbol.is_method())
                    .map(move |symbol| (path, symbol))
            })
    }

    fn package_files<'a>(
        &'a self,
        file: &Path,
    ) -> impl Iterator<Item = (&'a PathBuf, &'a IndexedFile)> + use<'a> {
        let scope = self
            .files
            .get_key_value(file)
            .map(|(path, stub)| (path.parent(), stub.package.as_str()));
        // Path ordering keeps a package subtree contiguous. Narrow by directory
        // before checking the package clause, without a duplicate file index.
        let directory = scope
            .and_then(|(directory, _)| directory)
            .unwrap_or(Path::new(""));
        // Orphaned open buffers only ever see themselves; they must not merge
        // into the real package view of their directory.
        let detached = self.detached.contains(file);
        self.files
            .range::<Path, _>((Bound::Included(directory), Bound::Unbounded))
            .take_while(move |(path, _)| path.starts_with(directory))
            .filter(move |(path, stub)| {
                if detached {
                    return self.detached.contains(*path)
                        && scope.is_some_and(|(directory, package)| {
                            path.parent() == directory && stub.package == package
                        });
                }
                !self.detached.contains(*path)
                    && scope.is_some_and(|(directory, package)| {
                        path.parent() == directory && stub.package == package
                    })
            })
    }
}
