//! Main/workspace selection and offline mounts; dependency replaces never apply.
use super::Workspace;
use super::discovery::PlanInstall;
use super::packages;
use crate::gomod::{Manifest, Replacement};
use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};

pub(super) struct Plan {
    pub roots: Vec<PathBuf>,
    pub modules: HashMap<PathBuf, String>,
    pub mains: HashSet<PathBuf>,
    pub compiler_checkout: bool,
}

pub(super) fn configuration(file: &Path) -> bool {
    file.file_name()
        .is_some_and(|name| name == "go.mod" || name == "go.work")
}

/// Import-serving module identity with the path prefix that matched the import.
pub(crate) struct ImportModule<'a> {
    /// Module path matched against the import string (suffix base).
    pub prefix: &'a str,
    /// Resolved module path (replacement target when replaced).
    pub module: String,
    /// Resolved module version, when the module comes from the offline cache.
    pub version: Option<String>,
}

/// Reverse the module-cache escaping (`!x` encodes an uppercase letter) so
/// displayed module versions match the original manifest spelling.
fn unescape_cache_component(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch == '!' {
            if let Some(next) = chars.next() {
                result.push(next.to_ascii_uppercase());
            }
        } else {
            result.push(ch);
        }
    }
    result
}

pub(super) fn absolute(base: &Path, value: &Path) -> PathBuf {
    let path = if value.is_absolute() {
        value.to_path_buf()
    } else {
        base.join(value)
    };
    let mut normalized = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                if normalized.file_name().is_some() {
                    normalized.pop();
                }
            }
            part => normalized.push(part.as_os_str()),
        }
    }
    normalized
}

fn same_replacement(left: &(PathBuf, Replacement), right: &(PathBuf, Replacement)) -> bool {
    left.1.target_version == right.1.target_version
        && if left.1.target_version.is_some() {
            left.1.target == right.1.target
        } else {
            absolute(&left.0, Path::new(&left.1.target))
                == absolute(&right.0, Path::new(&right.1.target))
        }
}

/// Reference visibility for one request; opened package siblings are visible
/// even when their closed declarations came from a dependency or another view.
pub(crate) struct ReferenceScope<'a> {
    workspace: &'a Workspace,
    origin: &'a Path,
    open_packages: HashMap<(&'a Path, &'a str), bool>,
}

impl ReferenceScope<'_> {
    /// Search the declaring package and workspace packages, while allowing the
    /// complete dependency graph to participate in identity/type resolution.
    pub(crate) fn contains(&self, file: &Path, target: &Path) -> bool {
        let workspace = self.workspace;
        let package = file
            .parent()
            .zip(workspace.files.get(file))
            .map(|(directory, stub)| (directory, stub.package.as_str()));
        let tests_open = package.and_then(|key| self.open_packages.get(&key));
        // Opening a dependency's production package does not implicitly load its
        // tests into another module view. Explicitly opened tests remain visible.
        if workspace.compiler_checkout
            && !packages::production_file(file)
            && workspace
                .sdk_owner(self.origin)
                .is_some_and(|owner| workspace.sdk_owner(file) != Some(owner))
            && tests_open != Some(&true)
        {
            return false;
        }
        if file.parent() == target.parent()
            && workspace
                .files
                .get(file)
                .zip(workspace.files.get(target))
                .is_some_and(|(source, declaration)| source.package == declaration.package)
        {
            return true;
        }
        if tests_open.is_some() {
            return true;
        }
        if workspace
            .sdk_packages
            .contains(file.parent().unwrap_or(file))
        {
            return false;
        }
        if let Some(root) = workspace
            .modules
            .keys()
            .filter(|root| file.starts_with(root))
            .max_by_key(|root| root.components().count())
        {
            return workspace.main_modules.contains(root)
                && (!workspace.compiler_checkout
                    || workspace
                        .sdk_owner(self.origin)
                        .is_none_or(|owner| workspace.sdk_owner(file) == Some(owner)));
        }
        // Non-module roots and explicitly opened scratch files retain their
        // existing scope, without turning dependency mounts into main modules.
        workspace.roots.iter().any(|root| file.starts_with(root))
            || workspace.open.contains_key(file)
    }
}

/// What a manifest-triggered re-plan does with the current index.
#[derive(Debug, PartialEq, Eq)]
enum PlanAction {
    /// The plan digest is unchanged and only main manifests changed.
    Skip,
    /// Main manifests changed the plan: reuse stubs under shared roots.
    Incremental,
    /// Nested manifests may have changed additional views, or no plan is
    /// installed yet: rebuild from scratch.
    Full,
}

impl Workspace {
    /// Module identity for an import served by an offline dependency mount.
    /// `prefix` is the module path matched against the import string (used to
    /// derive the trailing package suffix); `module`/`version` are the resolved
    /// module identity after `replace` directives, as gopls reports it in
    /// documentation links. Main-module and std imports return none.
    pub(crate) fn import_module(&self, import: &str) -> Option<ImportModule<'_>> {
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
        let resolved = self.module_cache.as_ref().and_then(|cache| {
            let relative = root.strip_prefix(cache).ok()?.to_str()?;
            let (escaped_path, escaped_version) = relative.rsplit_once('@')?;
            Some((
                unescape_cache_component(escaped_path),
                unescape_cache_component(escaped_version),
            ))
        });
        match resolved {
            Some((resolved_module, version)) => Some(ImportModule {
                prefix: module.as_str(),
                module: resolved_module,
                version: Some(version),
            }),
            None => Some(ImportModule {
                prefix: module.as_str(),
                module: module.clone(),
                version: None,
            }),
        }
    }

    /// Implementation search includes indexed forward dependencies, but the
    /// automatic compiler-checkout workspace still has separate std/cmd views.
    pub(crate) fn hierarchy_scope(&self, origin: &Path, candidate: &Path) -> bool {
        if !self.compiler_checkout {
            return true;
        }
        match (self.sdk_owner(origin), self.sdk_owner(candidate)) {
            (Some("std"), Some("cmd")) => false,
            (Some("cmd"), Some("std")) => {
                packages::production_file(candidate) || self.open.contains_key(candidate)
            }
            _ => true,
        }
    }

    pub(crate) fn reference_scope<'a>(&'a self, origin: &'a Path) -> ReferenceScope<'a> {
        let mut open_packages = HashMap::new();
        for file in self.open.keys() {
            if let Some(stub) = self.files.get(file)
                && let Some(directory) = file.parent()
            {
                let test = !packages::production_file(file);
                open_packages
                    .entry((directory, stub.package.as_str()))
                    .and_modify(|tests| *tests |= test)
                    .or_insert(test);
            }
        }
        ReferenceScope {
            workspace: self,
            origin,
            open_packages,
        }
    }

    pub(super) fn manifest(&self, file: &Path) -> Result<Option<Manifest>, String> {
        let source = if file.file_name().is_some_and(|name| name == "go.mod") {
            Self::read_source(file)
        } else {
            self.source(file)
        };
        source
            .map(|source| {
                crate::gomod::parse(&source).map_err(|error| format!("{}: {error}", file.display()))
            })
            .transpose()
    }

    pub(super) fn module_plan(&self, folders: &[PathBuf]) -> Result<Plan, String> {
        let mut mains = HashMap::new();
        let mut compiler_checkout = false;
        let mut roots = Vec::new();
        let mut work_rules: HashMap<_, _> = HashMap::new();
        let mut seen_work = HashSet::new();
        for folder in folders {
            let work = match std::env::var_os("GOWORK") {
                Some(value) if value == "off" => None,
                Some(value) if !value.is_empty() && value != "auto" => Some(PathBuf::from(value)),
                _ => folder
                    .ancestors()
                    .map(|parent| parent.join("go.work"))
                    .find(|path| path.is_file()),
            };
            if let Some(file) = work {
                if seen_work.insert(file.clone()) {
                    let work = self
                        .manifest(&file)?
                        .ok_or_else(|| format!("workspace file unavailable: {}", file.display()))?;
                    let base = file.parent().ok_or("workspace file has no directory")?;
                    for used in work.uses {
                        let root = absolute(base, Path::new(&used));
                        self.main_module(&root, &mut mains)?;
                    }
                    for rule in work.replacements {
                        work_rules.insert(
                            (rule.module.clone(), rule.version.clone()),
                            (base.to_path_buf(), rule),
                        );
                    }
                }
            } else if folder.join("src/builtin/builtin.go").is_file()
                && self
                    .manifest(&folder.join("src/go.mod"))?
                    .is_some_and(|manifest| manifest.module.as_deref() == Some("std"))
                && self
                    .manifest(&folder.join("src/cmd/go.mod"))?
                    .is_some_and(|manifest| manifest.module.as_deref() == Some("cmd"))
            {
                // A Go checkout is a two-module workspace. Keep its intentionally
                // invalid compiler test corpus outside ordinary package discovery.
                compiler_checkout = true;
                self.main_module(&folder.join("src"), &mut mains)?;
                self.main_module(&folder.join("src/cmd"), &mut mains)?;
            } else if let Some(file) = folder
                .ancestors()
                .map(|parent| parent.join("go.mod"))
                .find(|path| path.is_file())
            {
                self.main_module(file.parent().expect("module directory"), &mut mains)?;
            }
        }
        let mut rules: HashMap<_, _> = HashMap::new();
        for (root, manifest) in mains.values() {
            for rule in &manifest.replacements {
                let key = (rule.module.clone(), rule.version.clone());
                if work_rules.contains_key(&key)
                    || work_rules.contains_key(&(rule.module.clone(), None))
                {
                    continue;
                }
                let next = (root.clone(), rule.clone());
                if let Some(previous) = rules.get(&key)
                    && !same_replacement(previous, &next)
                {
                    return Err(format!(
                        "conflicting local replacements for {}",
                        rule.module
                    ));
                }
                rules.insert(key, next);
            }
        }
        rules.extend(work_rules);
        let mut modules: HashMap<_, _> = mains
            .iter()
            .map(|(name, (root, _))| (root.clone(), name.clone()))
            .collect();
        let dependencies = self.dependency_mounts(&mains, &rules)?;
        for (root, name) in dependencies {
            if let Some(previous) = modules.get(&root) {
                if previous != &name {
                    return Err("one directory mounted as different modules".into());
                }
                continue;
            }
            modules.insert(root, name);
        }
        roots.extend(
            folders
                .iter()
                .filter(|folder| {
                    modules.is_empty() || modules.keys().any(|module| folder.starts_with(module))
                })
                .cloned(),
        );
        // Cache modules are mounts, not recursive workspace roots. Load only
        // imported production package declarations; keep local replacements' scope.
        roots.extend(
            modules
                .keys()
                .filter(|root| {
                    !self
                        .module_cache
                        .as_ref()
                        .is_some_and(|cache| root.starts_with(cache))
                })
                .cloned(),
        );
        roots.sort();
        roots.dedup();
        if self.trace.modules {
            let mut mounts: Vec<_> = modules.iter().collect();
            mounts.sort_by_key(|(_, name)| *name);
            for (root, name) in mounts {
                eprintln!("module mount: {name} => {}", root.display());
            }
        }
        let mains = mains.into_values().map(|(root, _)| root).collect();
        Ok(Plan {
            roots,
            modules,
            mains,
            compiler_checkout,
        })
    }

    fn main_module(
        &self,
        root: &Path,
        mains: &mut HashMap<String, (PathBuf, Manifest)>,
    ) -> Result<(), String> {
        let manifest = self
            .manifest(&root.join("go.mod"))?
            .ok_or_else(|| format!("module file unavailable: {}", root.display()))?;
        let name = manifest.module.clone().ok_or("module directive missing")?;
        if let Some((previous, _)) = mains.get(&name)
            && previous != root
        {
            return Err(format!("duplicate workspace module {name}"));
        }
        if mains.len() >= 1024 && !mains.contains_key(&name) {
            return Err("workspace exceeds 1024 main modules".into());
        }
        mains.insert(name, (root.to_path_buf(), manifest));
        Ok(())
    }

    /// Re-plan the workspace for changed manifests. A plan whose roots, modules,
    /// main modules and checkout mode are unchanged is a no-op when the changed
    /// manifests are main-module manifests: the files and import identities stay
    /// valid, so the expensive root walk and dependency/SDK reload are skipped.
    /// Nested-module manifests still force a full rescan because they can change
    /// additional views that the main plan does not record.
    pub(super) fn refresh_modules(&mut self, changed: &[PathBuf]) {
        let folders = self.folders.clone();
        let plan = match self.module_plan(&folders) {
            Ok(plan) => plan,
            Err(error) => {
                eprintln!("workspace modules: {error}");
                return;
            }
        };
        match self.plan_action(&plan, changed) {
            PlanAction::Skip => {
                if self.trace.modules {
                    eprintln!("modules: plan unchanged; skipped workspace rescan");
                }
            }
            PlanAction::Incremental => {
                if let Err(error) = self.install_plan_incremental(folders, plan) {
                    eprintln!("workspace modules: {error}");
                }
            }
            PlanAction::Full => {
                if let Err(error) = self.install_plan(folders, plan, PlanInstall::Full) {
                    eprintln!("workspace modules: {error}");
                }
            }
        }
    }

    /// Classify one manifest-triggered re-plan without walking the filesystem.
    fn plan_action(&self, plan: &Plan, changed: &[PathBuf]) -> PlanAction {
        let main_only = changed.iter().all(|file| self.main_manifest(file));
        if !main_only {
            return PlanAction::Full;
        }
        if self.plan_digest == Some(plan_digest(plan)) {
            return PlanAction::Skip;
        }
        if self.plan_digest.is_some() {
            PlanAction::Incremental
        } else {
            PlanAction::Full
        }
    }

    /// Manifests that can only affect `Plan` (and are fully represented in its
    /// digest): the selected go.work file and main-module go.mod files.
    fn main_manifest(&self, file: &Path) -> bool {
        match file.file_name().and_then(|name| name.to_str()) {
            Some("go.work") => true,
            Some("go.mod") => file
                .parent()
                .is_some_and(|directory| self.main_modules.contains(directory)),
            _ => false,
        }
    }

    pub(super) fn reindex(&mut self, file: &Path) {
        if configuration(file) {
            self.refresh_modules(std::slice::from_ref(&file.to_path_buf()));
        } else {
            self.index(file);
        }
    }
}

/// Stable identity of a plan: used to skip rebuilds when a manifest edit does
/// not change any selected root, module path, main module or checkout mode.
pub(super) fn plan_digest(plan: &Plan) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    plan.roots.hash(&mut hasher);
    let mut modules: Vec<_> = plan.modules.iter().collect();
    modules.sort();
    modules.hash(&mut hasher);
    let mut mains: Vec<_> = plan.mains.iter().collect();
    mains.sort();
    mains.hash(&mut hasher);
    plan.compiler_checkout.hash(&mut hasher);
    hasher.finish()
}
