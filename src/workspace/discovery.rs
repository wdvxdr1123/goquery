//! Consistent discovery rules for traversal, editor buffers and file events.
use crate::workspace::Workspace;
use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

fn ignored_directory(name: &OsStr) -> bool {
    name.to_str().is_some_and(|name| {
        name.starts_with(['.', '_'])
            || matches!(name, "vendor" | "testdata" | "node_modules" | "target")
    })
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
        // Planning is transactional: incomplete manifest edits keep the old index.
        let plan = self.module_plan(&folders)?;
        self.folders = folders;
        self.roots = plan.roots;
        self.modules = plan.modules;
        self.main_modules = plan.mains;
        self.compiler_checkout = plan.compiler_checkout;
        self.configure_sdk_vendor();
        self.files.clear();
        self.sdk_packages.clear();
        self.dependency_packages.clear();
        self.excluded_modules.clear();
        let mut excluded = HashSet::new();
        for root in &self.roots {
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
                        if !self.modules.contains_key(entry.path()) {
                            excluded.insert(entry.path().to_path_buf());
                        }
                        return false;
                    }
                    true
                })
                .filter_map(Result::ok)
            {
                if entry.file_type().is_file()
                    && entry.path().extension().is_some_and(|ext| ext == "go")
                {
                    let file = entry.path();
                    if let Some(stub) = self.indexed_file(file) {
                        self.files.insert(file.to_path_buf(), stub);
                    }
                }
            }
        }
        self.excluded_modules = excluded.into_iter().collect();
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
        self.load_dependencies();
        self.load_sdk();
        self.restore_nested_views();
        self.rebuild_package_names();
        Ok(())
    }
}
