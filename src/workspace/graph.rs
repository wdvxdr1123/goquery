//! Bounded offline MVS graph construction; only main/workspace replaces apply.
use super::Workspace;
use super::dependencies;
use super::modules;
use crate::gomod::{Manifest, Replacement};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

type Rules = HashMap<(String, Option<String>), (PathBuf, Replacement)>;
fn pruned(manifest: &Manifest) -> bool {
    manifest
        .go
        .as_deref()
        .and_then(|go| go.strip_prefix("1."))
        .and_then(|v| v.split('.').next())
        .and_then(|v| v.parse::<u32>().ok())
        .is_some_and(|minor| minor >= 17)
}
fn target(
    workspace: &Workspace,
    rules: &Rules,
    name: &str,
    version: &str,
) -> Result<Option<PathBuf>, String> {
    if let Some((base, rule)) = rules
        .get(&(name.to_owned(), Some(version.to_owned())))
        .or_else(|| rules.get(&(name.to_owned(), None)))
    {
        if let Some(version) = &rule.target_version {
            return Ok(workspace
                .module_cache
                .as_ref()
                .and_then(|cache| dependencies::cached_directory(cache, &rule.target, version)));
        }
        let path = Path::new(&rule.target);
        if !path.is_absolute() && !rule.target.starts_with('.') {
            return Err("local replacement must use a filesystem path".into());
        }
        return Ok(Some(modules::absolute(base, path)));
    }
    Ok(workspace
        .module_cache
        .as_ref()
        .and_then(|cache| dependencies::cached_directory(cache, name, version)))
}
impl Workspace {
    pub(super) fn dependency_mounts(
        &self,
        mains: &HashMap<String, (PathBuf, Manifest)>,
        rules: &Rules,
    ) -> Result<HashMap<PathBuf, String>, String> {
        let roots: HashSet<_> = mains
            .values()
            .flat_map(|(_, m)| m.requires.iter().map(|(n, _)| n.clone()))
            .collect();
        let excludes: HashSet<_> = mains
            .values()
            .flat_map(|(_, m)| m.excludes.iter().cloned())
            .collect();
        let full = mains.values().any(|(_, m)| !pruned(m));
        let mut pending: VecDeque<_> = mains
            .values()
            .flat_map(|(_, m)| {
                m.requires
                    .iter()
                    .map(|(n, v)| (n.clone(), v.clone(), true, full))
            })
            .collect();
        let mut selected: HashMap<String, String> = HashMap::new();
        let mut expanded = HashMap::new();
        let mut unavailable = 0;
        while let Some((name, version, expand, full)) = pending.pop_front() {
            if mains.contains_key(&name) {
                continue;
            }
            if excludes.contains(&(name.clone(), version.clone())) {
                return Err(format!(
                    "excluded requirement {name}@{version}: offline selection needs a permitted version"
                ));
            }
            let order = selected
                .get(&name)
                .map_or_else(
                    || crate::gomod::compare_versions(&version, &version),
                    |old| crate::gomod::compare_versions(&version, old),
                )
                .ok_or_else(|| format!("invalid module version {name}@{version}"))?;
            if !selected.contains_key(&name) || order == Ordering::Greater {
                selected.insert(name.clone(), version.clone());
            }
            if selected.len() > 1024 || expanded.len() > 16384 {
                return Err("module graph limit exceeded".into());
            }
            if !expand && !full && !roots.contains(&name) {
                continue;
            }
            let key = (name.clone(), version.clone());
            if expanded
                .get(&key)
                .is_some_and(|previous| *previous || !full)
            {
                continue;
            }
            expanded.insert(key, full);
            let Some(root) = target(self, rules, &name, &version)? else {
                continue;
            };
            let mut manifest = self.manifest(&root.join("go.mod"))?;
            if manifest.is_none()
                && let Some(cache) = &self.module_cache
            {
                // The download metadata can exist even when sources are not extracted.
                let encoded = root.strip_prefix(cache).ok().and_then(|path| path.to_str());
                if let Some((path, version)) = encoded.and_then(|path| path.rsplit_once('@')) {
                    manifest = self.manifest(
                        &cache
                            .join("cache/download")
                            .join(path)
                            .join("@v")
                            .join(format!("{version}.mod")),
                    )?;
                }
            }
            let Some(manifest) = manifest else {
                unavailable += 1;
                continue;
            };
            let full = full || !pruned(&manifest);
            pending.extend(
                manifest
                    .requires
                    .into_iter()
                    .map(|(name, version)| (name, version, full, full)),
            );
        }
        let mut modules = HashMap::new();
        for (name, version) in selected {
            if self.trace.modules {
                eprintln!("module selected: {name}@{version}");
            }
            let Some(root) = target(self, rules, &name, &version)? else {
                continue;
            };
            if !root.is_dir() {
                continue;
            }
            if let Some(previous) = modules.insert(root, name.clone())
                && previous != name
            {
                return Err("one directory mounted as different modules".into());
            }
        }
        if unavailable != 0 {
            eprintln!(
                "module graph: {unavailable} cached manifests unavailable; run go mod download and refresh"
            );
        }
        Ok(modules)
    }
}
