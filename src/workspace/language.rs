//! Event-owned module releases and immutable per-file language selection.
use super::Workspace;
use std::path::Path;

pub(super) fn minor(version: &str) -> Option<u16> {
    let rest = version.strip_prefix("1.")?;
    let end = rest
        .find(|ch: char| !ch.is_ascii_digit())
        .unwrap_or(rest.len());
    let value = rest.get(..end)?;
    if value.is_empty() || value.len() > 1 && value.starts_with('0') {
        return None;
    }
    value.parse().ok()
}

impl Workspace {
    /// Package version for presentation; queries never reopen a manifest.
    pub(crate) fn package_go_version(&self, file: &Path) -> Option<u16> {
        if let Some(root) = self
            .modules
            .keys()
            .filter(|root| file.starts_with(root))
            .max_by_key(|root| root.components().count())
        {
            return self.module_go_versions.get(root).copied();
        }
        self.sdk_root
            .as_deref()
            .filter(|root| file.starts_with(root))?;
        self.sdk_go_version
    }

    /// A file build constraint overrides the package language, with the Go
    /// checker's 1.21 floor. This may both raise and lower the package version.
    pub(crate) fn file_go_version(&self, file: &Path) -> Option<u16> {
        if let Some(version) = self.files.get(file)?.build_go_version {
            return Some(version.max(21));
        }
        self.package_go_version(file)
    }

    pub(super) fn refresh_module_go_version(&mut self, root: &Path) {
        let version = self
            .manifest(&root.join("go.mod"))
            .ok()
            .flatten()
            .and_then(|manifest| manifest.go.map_or(Some(16), |version| minor(&version)));
        match version {
            Some(version) => {
                self.module_go_versions.insert(root.to_path_buf(), version);
            }
            None => {
                self.module_go_versions.remove(root);
            }
        }
    }
}
