//! Bounded additional module views for nested modules under a workspace root.
//!
//! Opening a file that belongs to a nested module the main plan did not select
//! (the typical case is `$GOROOT/src/cmd/go.mod` when the workspace root is
//! `$GOROOT/src`) mounts that module as an additional view: only the
//! module-local package directories imported from the opened file are indexed,
//! so `cmd/...` imports resolve for navigation/hover without merging the nested
//! module into the main view's roots, main modules or workspace scans.
use super::Workspace;
use crate::index::IndexedFile;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};

/// Additional views bounded independently of the main workspace.
const MAX_NESTED_MODULES: usize = 8;
const MAX_NESTED_DIRECTORIES: usize = 64;
const MAX_NESTED_FILES: usize = 4096;
const MAX_NESTED_BYTES: usize = 16 << 20;

impl Workspace {
    /// Nearest enclosing `go.mod` below a workspace root that the main module
    /// plan did not select.
    fn nested_module_of(&self, file: &Path) -> Option<(PathBuf, String)> {
        let mut directory = file.parent()?;
        loop {
            if directory.join("go.mod").is_file() {
                if let Some(module) = self.nested_modules.get(directory) {
                    return Some((directory.to_path_buf(), module.clone()));
                }
                if self.modules.contains_key(directory) {
                    return None;
                }
                let manifest = self.manifest(&directory.join("go.mod")).ok().flatten()?;
                return Some((directory.to_path_buf(), manifest.module?));
            }
            if self.roots.iter().any(|root| root == directory) {
                return None;
            }
            if !self.roots.iter().any(|root| directory.starts_with(root)) {
                return None;
            }
            directory = directory.parent()?;
        }
    }

    /// Mount the opened file's nested module as an additional view and load the
    /// module-local packages it imports, transitively and within fixed bounds.
    /// Returns whether the file belongs to such a module.
    pub(super) fn load_nested_view(&mut self, file: &Path) -> bool {
        let Some((root, module)) = self.nested_module_of(file) else {
            return false;
        };
        if !self.nested_modules.contains_key(&root)
            && self.nested_modules.len() >= MAX_NESTED_MODULES
        {
            return false;
        }
        self.nested_modules
            .entry(root.clone())
            .or_insert_with(|| module.clone());
        self.modules
            .entry(root.clone())
            .or_insert_with(|| module.clone());
        let mut queue: VecDeque<String> = self
            .files
            .get(file)
            .map(|stub| {
                stub.imports
                    .iter()
                    .map(|import| import.path.clone())
                    .collect()
            })
            .unwrap_or_default();
        while let Some(import) = queue.pop_front() {
            let Some(directory) = nested_directory(&root, &module, &import) else {
                continue;
            };
            if self.nested_directories.contains(&directory) {
                continue;
            }
            if self.nested_directories.len() >= MAX_NESTED_DIRECTORIES {
                break;
            }
            queue.extend(self.load_nested_directory(&directory));
        }
        self.rebuild_package_names();
        true
    }

    /// Restore mounted views after a transactional root refresh cleared the
    /// derived index; the same bounded directory set is re-read from disk.
    pub(super) fn restore_nested_views(&mut self) {
        if self.nested_modules.is_empty() {
            return;
        }
        let modules: Vec<(PathBuf, String)> = self
            .nested_modules
            .iter()
            .map(|(root, module)| (root.clone(), module.clone()))
            .collect();
        for (root, module) in modules {
            self.modules.insert(root, module);
        }
        let directories: Vec<PathBuf> = self.nested_directories.drain().collect();
        self.nested_files.clear();
        self.nested_bytes = 0;
        for directory in directories {
            if directory.is_dir() {
                self.load_nested_directory(&directory);
            }
        }
        // Drop views whose manifest disappeared or was renamed; the main view is
        // unaffected and the stale module identity never leaks into imports.
        let stale: Vec<PathBuf> = self
            .nested_modules
            .iter()
            .filter(|(root, module)| {
                self.manifest(&root.join("go.mod"))
                    .ok()
                    .flatten()
                    .and_then(|manifest| manifest.module)
                    .is_none_or(|name| &name != *module)
            })
            .map(|(root, _)| root.clone())
            .collect();
        for root in stale {
            self.nested_modules.remove(&root);
            self.modules.remove(&root);
            let stale_files: Vec<PathBuf> = self
                .nested_files
                .iter()
                .filter(|file| file.starts_with(&root))
                .cloned()
                .collect();
            for file in stale_files {
                self.files.remove(&file);
                self.detached.remove(&file);
                self.nested_files.remove(&file);
            }
        }
        self.rebuild_package_names();
    }

    /// Index one module-local package directory (production files only) and
    /// return the module-local imports for the caller's bounded worklist.
    fn load_nested_directory(&mut self, directory: &Path) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(directory) else {
            return Vec::new();
        };
        let mut paths: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension().is_some_and(|extension| extension == "go")
                    && crate::workspace::production_file(path)
            })
            .collect();
        paths.sort();
        let mut imports = Vec::new();
        for path in paths {
            if self.files.contains_key(&path) {
                continue;
            }
            if self.nested_files.len() >= MAX_NESTED_FILES {
                break;
            }
            let Some(source) = self.source(&path) else {
                continue;
            };
            if self.nested_bytes + source.len() > MAX_NESTED_BYTES {
                break;
            }
            let Some(stub) = self.nested_file_from(&path, &source) else {
                continue;
            };
            self.nested_bytes += source.len();
            imports.extend(stub.imports.iter().map(|import| import.path.clone()));
            self.files.insert(path.clone(), stub);
            // Additional-view files never participate in main-view scans.
            self.detached.insert(path.clone());
            self.nested_files.insert(path);
        }
        self.nested_directories.insert(directory.to_path_buf());
        imports
    }

    /// Index an additional view: the build target still applies, but the
    /// main-view discovery/exclusion scope does not.
    fn nested_file_from(&self, file: &Path, source: &str) -> Option<IndexedFile> {
        if source.len() as u64 > crate::index::MAX_FILE_BYTES {
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

/// Directory for a module-local import path (`module` or `module/sub`).
fn nested_directory(root: &Path, module: &str, import: &str) -> Option<PathBuf> {
    if import == module {
        return Some(root.to_path_buf());
    }
    import
        .strip_prefix(module)
        .and_then(|rest| rest.strip_prefix('/'))
        .map(|rest| root.join(rest))
}
