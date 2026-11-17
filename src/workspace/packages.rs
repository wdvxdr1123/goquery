//! Package-clause metadata, independent of sources and query syntax arenas.
use super::Workspace;
use crate::syntax::Import;
use std::collections::HashMap;
use std::path::Path;

#[derive(Default)]
pub(crate) struct PackageNames {
    names: HashMap<String, Vec<(String, usize)>>,
}

impl PackageNames {
    pub(super) fn add(&mut self, path: String, name: &str) {
        if name.is_empty() {
            return;
        }
        let names = self
            .names
            .entry(path)
            .or_insert_with(|| Vec::with_capacity(1));
        if let Some((_, count)) = names.iter_mut().find(|(previous, _)| previous == name) {
            *count += 1;
        } else {
            names.push((name.to_owned(), 1));
        }
    }

    pub(super) fn remove(&mut self, path: &str, name: &str) {
        let Some(names) = self.names.get_mut(path) else {
            return;
        };
        if let Some(index) = names.iter().position(|(previous, _)| previous == name) {
            if names[index].1 == 1 {
                names.swap_remove(index);
            } else {
                names[index].1 -= 1;
            }
        }
        if names.is_empty() {
            self.names.remove(path);
        }
    }

    fn name(&self, path: &str) -> Option<&str> {
        let names = self.names.get(path)?;
        (names.len() == 1).then(|| names[0].0.as_str())
    }
}

pub(crate) fn production_file(file: &Path) -> bool {
    !file
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with("_test.go"))
}

impl Workspace {
    pub(crate) fn directory_import_path(&self, directory: &Path) -> Option<String> {
        let (root, module) = self
            .modules
            .iter()
            .filter(|(root, _)| directory.starts_with(root))
            .max_by_key(|(root, _)| root.components().count())?;
        let suffix = directory
            .strip_prefix(root)
            .ok()?
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        Some(if module == "std" {
            suffix
        } else if suffix.is_empty() {
            module.clone()
        } else {
            format!("{module}/{suffix}")
        })
    }

    pub(crate) fn import_path(&self, file: &Path) -> Option<String> {
        self.directory_import_path(file.parent()?)
    }

    /// Selector visibility and test-package visibility are separate concerns.
    pub(crate) fn selector_file_visible_in_view(
        &self,
        file: &Path,
        candidate: &Path,
        view: super::BuildView<'_>,
    ) -> bool {
        if !view.visible(self, candidate) {
            return false;
        }
        if production_file(candidate) {
            return true;
        }
        if production_file(file) || file.parent() != candidate.parent() {
            return false;
        }
        let (Some(consumer), Some(target)) = (self.files.get(file), self.files.get(candidate))
        else {
            return false;
        };
        consumer.package == target.package
            || !production_file(file)
                && consumer.package.strip_suffix("_test") == Some(target.package.as_str())
    }

    pub(super) fn rebuild_package_names(&mut self) {
        let mut names = PackageNames::default();
        for (file, stub) in &self.files {
            // Additional-view (nested module) files keep their package identity
            // for import resolution while staying out of main-view scans.
            if self.detached.contains(file) && !self.nested_files.contains(file) {
                continue;
            }
            if production_file(file)
                && let Some(path) = self.import_path(file)
            {
                names.add(path, &stub.package);
            }
        }
        self.package_names = names;
    }

    pub(crate) fn import_name<'a>(&'a self, import: &'a Import) -> Option<&'a str> {
        if let Some(name) = import.explicit_name() {
            return Some(name);
        }
        match self.package_names.names.get(&import.path) {
            Some(names) if names.len() == 1 => Some(names[0].0.as_str()),
            Some(_) => None,
            // Unindexed dependencies retain the existing lexical fallback. No
            // declarations can resolve through this heuristic without metadata.
            None => import.path.rsplit('/').next(),
        }
    }

    pub(crate) fn declared_package_name(&self, import: &str) -> Option<&str> {
        self.package_names.name(import)
    }
}
