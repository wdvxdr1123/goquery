//! Offline dependency mounts and import-directed declaration loading.
use super::Workspace;
use super::goenv::GoEnv;
use std::collections::{HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};

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
}
