//! Conservative metadata reachability, independent of promotion and visibility.
use crate::analysis::sema as locals;
use crate::bloom::Fingerprint;
use crate::index::IndexedSymbol as Symbol;
use crate::syntax::{TypeConstructor, TypeInfo};
use crate::workspace::Workspace;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

pub(super) type OwnerKey<'a> = (Option<&'a Path>, &'a str, &'a str);
type Restricted<'a> = (u64, Vec<(&'a PathBuf, &'a Symbol)>);
type ResolvedDecl<'a> = Option<(&'a PathBuf, &'a Symbol)>;

const MAX_REACH_PACKAGES: usize = 16_384;
const MAX_REACH_EDGES: usize = 65_536;
const MAX_REACH_PROPAGATIONS: usize = 512_000;
const MAX_LOCAL_ALIASES: usize = 4_096;

/// Query-local transitive package method-name reachability over `IndexedFile`
/// stubs. Strictly a sound necessary-condition negative filter for subtype
/// search (`supers == false`): a type declared in package `P` can satisfy
/// interface `I` only if every required method name (excluding universe `Error`)
/// is declared in `P` or in some package transitively imported by `P`.
pub(super) struct PackageReachability<'a> {
    pkg_ids: HashMap<(&'a Path, &'a str), usize>,
    transitive_masks: Vec<u64>,
    needed_mask: u64,
}

impl<'a> PackageReachability<'a> {
    /// Builds transitive method-name reachability for all non-detached packages
    /// in `workspace`. Returns `None` when disabled or any query bound is hit.
    pub(super) fn build(
        workspace: &'a Workspace,
        origin_file: &Path,
        required: &[&str],
        required_bitmask: u64,
        mask_enabled: bool,
    ) -> Option<Self> {
        if required.is_empty() || workspace.is_detached(origin_file) {
            return None;
        }
        let needed_mask = if mask_enabled {
            let full = if required.len() == 64 {
                u64::MAX
            } else {
                (1u64 << required.len()) - 1
            };
            if let Some(pos) = required.iter().position(|name| *name == "Error") {
                full & !(1u64 << pos)
            } else {
                full
            }
        } else if required.contains(&"Error") {
            required_bitmask & !TypeInfo::method_name_bit("Error")
        } else {
            required_bitmask
        };
        if needed_mask == 0 {
            return None;
        }
        let required_fps: Vec<Fingerprint> = required
            .iter()
            .map(|name| Fingerprint::new(name.as_bytes()))
            .collect();

        let mut pkg_ids: HashMap<(&'a Path, &'a str), usize> = HashMap::new();
        let mut dir_imports: HashMap<&'a Path, Option<String>> = HashMap::new();
        let mut pkg_dirs: Vec<&'a Path> = Vec::new();
        let mut pkg_importable: Vec<bool> = Vec::new();
        let mut direct_masks: Vec<u64> = Vec::new();
        let mut pkg_imports: Vec<Vec<&'a str>> = Vec::new();

        for (path, stub) in &workspace.files {
            if workspace.is_detached(path) {
                continue;
            }
            let Some(dir) = path.parent() else {
                continue;
            };
            let pkg_id = match pkg_ids.get(&(dir, stub.package.as_str())) {
                Some(&id) => id,
                None => {
                    if pkg_ids.len() >= MAX_REACH_PACKAGES {
                        return None;
                    }
                    let id = pkg_ids.len();
                    pkg_ids.insert((dir, stub.package.as_str()), id);
                    dir_imports
                        .entry(dir)
                        .or_insert_with(|| workspace.directory_import_path(dir));
                    pkg_dirs.push(dir);
                    pkg_importable.push(false);
                    direct_masks.push(0);
                    pkg_imports.push(Vec::new());
                    id
                }
            };
            if !is_external_test_file(path, &stub.package) {
                pkg_importable[pkg_id] = true;
            }
            if !stub.methods_opaque {
                if (stub.declared_methods & required_bitmask) != 0 {
                    if mask_enabled {
                        for symbol in &stub.symbols {
                            if symbol.is_method() {
                                if let Some(bit) = required
                                    .iter()
                                    .position(|name| *name == symbol.name.as_str())
                                {
                                    direct_masks[pkg_id] |= 1u64 << bit;
                                }
                            } else if symbol.is_type()
                                && let Some(info) = &symbol.type_info
                                && info.is_interface()
                                && (info.direct_methods_mask & required_bitmask) != 0
                            {
                                direct_masks[pkg_id] |= name_mask(&info.members, required);
                            }
                        }
                    } else {
                        direct_masks[pkg_id] |= stub.declared_methods & required_bitmask;
                    }
                }
            } else if mask_enabled {
                for (bit, fp) in required_fps.iter().enumerate() {
                    let flag = 1u64 << bit;
                    if (direct_masks[pkg_id] & flag) == 0
                        && stub.identifiers.may_contain_fingerprint(*fp)
                    {
                        direct_masks[pkg_id] |= flag;
                    }
                }
            } else {
                for (&name, fp) in required.iter().zip(&required_fps) {
                    let flag = TypeInfo::method_name_bit(name);
                    if (direct_masks[pkg_id] & flag) == 0
                        && stub.identifiers.may_contain_fingerprint(*fp)
                    {
                        direct_masks[pkg_id] |= flag;
                    }
                }
            }
            let imports = &mut pkg_imports[pkg_id];
            for import in &stub.imports {
                if import.is_blank() || matches!(import.path.as_str(), "C" | "unsafe") {
                    continue;
                }
                imports.push(import.path.as_str());
            }
        }

        let num_pkgs = pkg_ids.len();
        let mut import_map: HashMap<&str, Vec<usize>> = HashMap::new();
        for pkg_id in 0..num_pkgs {
            if !pkg_importable[pkg_id] {
                continue;
            }
            if let Some(Some(import_path)) = dir_imports.get(pkg_dirs[pkg_id]) {
                import_map
                    .entry(import_path.as_str())
                    .or_default()
                    .push(pkg_id);
            }
        }

        let mut importers: Vec<Vec<usize>> = vec![Vec::new(); num_pkgs];
        let mut edge_count = 0usize;
        for (importer, imports) in pkg_imports.iter().enumerate() {
            for &imp_path in imports {
                let Some(targets) = import_map.get(imp_path) else {
                    continue;
                };
                for &target in targets {
                    if target != importer && importers[target].last() != Some(&importer) {
                        importers[target].push(importer);
                        edge_count += 1;
                        if edge_count > MAX_REACH_EDGES {
                            return None;
                        }
                    }
                }
            }
        }

        let mut transitive_masks: Vec<u64> = direct_masks
            .into_iter()
            .map(|mask| mask & needed_mask)
            .collect();
        let mut in_queue = vec![false; num_pkgs];
        let mut worklist = Vec::new();
        for (pkg_id, &mask) in transitive_masks.iter().enumerate() {
            if mask != 0 {
                in_queue[pkg_id] = true;
                worklist.push(pkg_id);
            }
        }
        let mut propagations = 0usize;
        while let Some(target) = worklist.pop() {
            in_queue[target] = false;
            let mask = transitive_masks[target];
            for &importer in &importers[target] {
                let next = transitive_masks[importer] | mask;
                if next != transitive_masks[importer] {
                    transitive_masks[importer] = next;
                    propagations += 1;
                    if propagations > MAX_REACH_PROPAGATIONS {
                        return None;
                    }
                    if !in_queue[importer] {
                        in_queue[importer] = true;
                        worklist.push(importer);
                    }
                }
            }
        }

        Some(Self {
            pkg_ids,
            transitive_masks,
            needed_mask,
        })
    }

    /// Returns `false` only when it is mathematically impossible for any type
    /// declared in `(file.parent(), package)` to provide all required method names.
    pub(super) fn package_may_implement(&self, file: &Path, package: &str) -> bool {
        let Some(dir) = file.parent() else {
            return true;
        };
        let Some(&pkg_id) = self.pkg_ids.get(&(dir, package)) else {
            return true;
        };
        (self.transitive_masks[pkg_id] & self.needed_mask) == self.needed_mask
    }
}

fn is_external_test_file(path: &Path, package: &str) -> bool {
    package.ends_with("_test")
        && path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().ends_with("_test.go"))
}

pub(super) struct Candidates<'a> {
    view: crate::analysis::resolve::View<'a>,
    required_bitmask: u64,
    pub(super) receivers: HashSet<OwnerKey<'a>>,
    pub(super) matching: HashSet<OwnerKey<'a>>,
    /// One bit per queried required method name, set for receiver owners that
    /// declare at least one of them. Query-local and bounded with the owners.
    pub(super) owner_masks: HashMap<OwnerKey<'a>, u64>,
    pub(super) masks_complete: bool,
    /// Same-package type aliases recorded during collection so methods declared
    /// on a type alias (`type A = Real; func (A) M()`) propagate soundly to `Real`.
    local_aliases: HashMap<OwnerKey<'a>, Option<&'a str>>,
    aliases_complete: bool,
    alias_unknown_packages: HashSet<(Option<&'a Path>, &'a str)>,
    /// Per-package receiver-name coverage and required-method declarations,
    /// built lazily when the global owner inventory fell back. Packages are
    /// bounded and each inventory only keeps the queried names.
    package_inventory: HashMap<(Option<&'a Path>, &'a str), PackageInventory<'a>>,
    package_methods_complete: bool,
    package_method_entries: usize,
    /// Required-name receiver inventory for the whole query scope, built in the
    /// same pass as the owner masks. Only owners that declare a queried name are
    /// kept, so this stays far below the complete receiver-owner bound.
    restricted: HashMap<OwnerKey<'a>, Restricted<'a>>,
    restricted_complete: bool,
    coverage_memo: HashMap<(&'a Path, usize), Option<u64>>,
    coverage_active: HashSet<(&'a Path, usize)>,
    /// Memoized lower bound of 64-bit `TypeInfo::method_name_bit` required by an
    /// interface (direct methods union transitively embedded interfaces).
    iface_required_memo: HashMap<(&'a Path, usize), u64>,
    iface_required_active: HashSet<(&'a Path, usize)>,
    absent: HashSet<(&'a Path, usize)>,
    /// Exact file scope and the immutable request view bound this candidate memo.
    declarations: HashMap<(&'a Path, &'a str), ResolvedDecl<'a>>,
    /// Package-level memo for unqualified type names in files without dot imports.
    pkg_declarations: HashMap<OwnerKey<'a>, ResolvedDecl<'a>>,
    /// Cross-file memo for imported type names keyed by `(import_path, member)`.
    imported_declarations: HashMap<(&'a str, &'a str), ResolvedDecl<'a>>,
    /// Memo for `view.declared_package_name` keyed by import path.
    import_pkg_names: HashMap<&'a str, Option<&'a str>>,
    pub(super) coverage_calls: usize,
    pub(super) coverage_unknowns: usize,
    pub(super) resolution_calls: usize,
    pub(super) resolution_hits: usize,
    pub(super) underlying_calls: usize,
    pub(super) declaration_calls: usize,
    pub(super) qualified_fast: usize,
    pub(super) qualified_slow: usize,
    pub(super) unqualified_fast: usize,
    pub(super) unqualified_slow: usize,
    pub(super) inventories: usize,
}

struct PackageInventory<'a> {
    masks: HashMap<&'a str, u64>,
    methods: HashMap<&'a str, Vec<(&'a PathBuf, &'a Symbol)>>,
}

impl<'a> Candidates<'a> {
    pub(super) fn new(view: crate::analysis::resolve::View<'a>, required_bitmask: u64) -> Self {
        Self {
            view,
            required_bitmask,
            receivers: HashSet::new(),
            matching: HashSet::new(),
            owner_masks: HashMap::new(),
            masks_complete: true,
            local_aliases: HashMap::new(),
            aliases_complete: true,
            alias_unknown_packages: HashSet::new(),
            package_inventory: HashMap::new(),
            package_methods_complete: true,
            package_method_entries: 0,
            restricted: HashMap::new(),
            restricted_complete: true,
            coverage_memo: HashMap::new(),
            coverage_active: HashSet::new(),
            iface_required_memo: HashMap::new(),
            iface_required_active: HashSet::new(),
            absent: HashSet::new(),
            declarations: HashMap::new(),
            pkg_declarations: HashMap::new(),
            imported_declarations: HashMap::new(),
            import_pkg_names: HashMap::new(),
            coverage_calls: 0,
            coverage_unknowns: 0,
            resolution_calls: 0,
            resolution_hits: 0,
            underlying_calls: 0,
            declaration_calls: 0,
            qualified_fast: 0,
            qualified_slow: 0,
            unqualified_fast: 0,
            unqualified_slow: 0,
            inventories: 0,
        }
    }

    /// Records a same-package type alias during the collection pass so methods
    /// declared on an alias receiver (`type A = Real; func (A) M()`) propagate
    /// to `Real` before any negative filter rejects `Real`.
    pub(super) fn record_local_alias(
        &mut self,
        file: &'a Path,
        package: &'a str,
        symbol: &'a Symbol,
    ) {
        if !symbol.alias {
            return;
        }
        let target = if !symbol.generic
            && let Some(target) = symbol
                .type_info
                .as_ref()
                .and_then(|info| info.target.as_ref())
            && !target.name.contains('.')
            && !target.name.trim_start().starts_with('*')
        {
            Some(locals::named_root(&target.name))
        } else {
            None
        };
        let key = (file.parent(), package, symbol.name.as_str());
        if let Some(existing) = self.local_aliases.get_mut(&key) {
            if *existing != target {
                *existing = None;
            }
            return;
        }
        if self.local_aliases.len() >= MAX_LOCAL_ALIASES {
            self.aliases_complete = false;
            return;
        }
        self.local_aliases.insert(key, target);
    }

    /// Propagates receiver methods declared on same-package type aliases to
    /// their canonical target type name, or marks the package unknown when an
    /// alias receiver cannot be resolved from metadata alone.
    pub(super) fn propagate_local_aliases(&mut self, indexed: bool) {
        if !self.aliases_complete {
            self.restricted_complete = false;
            self.masks_complete = false;
            return;
        }
        if self.local_aliases.is_empty() {
            return;
        }
        let mut alias_receivers: Vec<OwnerKey<'a>> = Vec::new();
        for &key in self.restricted.keys() {
            if self.local_aliases.contains_key(&key) {
                alias_receivers.push(key);
            }
        }
        for &key in self.owner_masks.keys() {
            if self.local_aliases.contains_key(&key) && !self.restricted.contains_key(&key) {
                alias_receivers.push(key);
            }
        }
        if indexed {
            for &key in &self.receivers {
                if self.local_aliases.contains_key(&key)
                    && !self.restricted.contains_key(&key)
                    && !self.owner_masks.contains_key(&key)
                {
                    alias_receivers.push(key);
                }
            }
        }
        for key in alias_receivers {
            let (dir, package, mut current) = key;
            let mut resolved = None;
            for _ in 0..16 {
                match self.local_aliases.get(&(dir, package, current)) {
                    Some(Some(next)) if *next != current => {
                        current = next;
                    }
                    Some(_) => {
                        resolved = None;
                        break;
                    }
                    None => {
                        resolved = Some(current);
                        break;
                    }
                }
            }
            let Some(canonical) = resolved else {
                self.alias_unknown_packages.insert((dir, package));
                continue;
            };
            let target_key = (dir, package, canonical);
            if let Some((bit, methods)) = self.restricted.get(&key).cloned() {
                if let Some(entry) = self.restricted.get_mut(&target_key) {
                    entry.0 |= bit;
                    entry.1.extend(methods);
                } else if self.restricted.len() < 16_384 {
                    self.restricted.insert(target_key, (bit, methods));
                } else {
                    self.restricted_complete = false;
                }
            }
            if let Some(&mask) = self.owner_masks.get(&key) {
                if let Some(entry) = self.owner_masks.get_mut(&target_key) {
                    *entry |= mask;
                } else if self.owner_masks.len() < 16_384 {
                    self.owner_masks.insert(target_key, mask);
                } else {
                    self.masks_complete = false;
                }
            }
            if indexed && self.receivers.contains(&key) {
                self.receivers.insert(target_key);
            }
            if self.matching.contains(&key) && self.matching.len() < 16_384 {
                self.matching.insert(target_key);
            }
        }
    }

    pub(super) fn is_alias_unknown(&self, file: &'a Path, package: &'a str) -> bool {
        !self.aliases_complete
            || self
                .alias_unknown_packages
                .contains(&(file.parent(), package))
    }

    /// Records one required-name receiver declaration in the query-wide
    /// restricted inventory.
    pub(super) fn insert_restricted(
        &mut self,
        path: &'a PathBuf,
        package: &'a str,
        symbol: &'a Symbol,
        bit: u64,
    ) {
        let Some(receiver) = symbol.receiver.as_deref() else {
            return;
        };
        let key = (path.parent(), package, receiver);
        if let Some(entry) = self.restricted.get_mut(&key) {
            entry.0 |= bit;
            entry.1.push((path, symbol));
            return;
        }
        if self.restricted.len() >= 16_384 {
            self.restricted_complete = false;
            return;
        }
        self.restricted.insert(key, (bit, vec![(path, symbol)]));
    }

    pub(super) fn restricted_mask(
        &self,
        file: &'a Path,
        package: &'a str,
        owner: &'a str,
    ) -> Option<u64> {
        if !self.restricted_complete || self.is_alias_unknown(file, package) {
            return None;
        }
        Some(
            self.restricted
                .get(&(file.parent(), package, owner))
                .map_or(0, |entry| entry.0),
        )
    }

    pub(super) fn restricted_methods<'b>(
        &'b self,
        file: &'a Path,
        package: &'a str,
        owner: &'a str,
    ) -> Option<&'b [(&'a PathBuf, &'a Symbol)]> {
        if !self.restricted_complete || self.is_alias_unknown(file, package) {
            return None;
        }
        Some(
            self.restricted
                .get(&(file.parent(), package, owner))
                .map_or(&[][..], |entry| entry.1.as_slice()),
        )
    }

    /// Bounded per-package receiver inventory, used only when the workspace-wide
    /// owner inventory overflowed. `None` means unknown.
    fn inventory(
        &mut self,
        workspace: &'a Workspace,
        file: &'a Path,
        required: &[&str],
    ) -> Option<&PackageInventory<'a>> {
        let stub = workspace.files.get(file)?;
        let key = (file.parent(), stub.package.as_str());
        if !self.package_inventory.contains_key(&key) {
            if self.package_inventory.len() >= 4_096 {
                return None;
            }
            let mut inventory = PackageInventory {
                masks: HashMap::new(),
                methods: HashMap::new(),
            };
            self.inventories += 1;
            for (path, symbol) in self.view.package_symbols(file) {
                let Some(receiver) = &symbol.receiver else {
                    continue;
                };
                let Some(bit) = required
                    .iter()
                    .position(|name| *name == symbol.name.as_str())
                else {
                    continue;
                };
                *inventory.masks.entry(receiver.as_ref()).or_insert(0) |= 1u64 << bit;
                if self.package_method_entries < 16_384 {
                    inventory
                        .methods
                        .entry(receiver.as_ref())
                        .or_default()
                        .push((path, symbol));
                    self.package_method_entries += 1;
                } else {
                    self.package_methods_complete = false;
                }
            }
            self.package_inventory.insert(key, inventory);
        }
        self.package_inventory.get(&key)
    }

    fn package_mask(
        &mut self,
        workspace: &'a Workspace,
        file: &'a Path,
        owner: &'a str,
        required: &[&str],
    ) -> Option<u64> {
        self.inventory(workspace, file, required)
            .map(|inventory| inventory.masks.get(owner).copied().unwrap_or(0))
    }

    /// Required-name receiver declarations for one owner in its own package. Only
    /// a plain declaration has no promotion, so callers must have proven there
    /// are no embeddings or underlying named type.
    pub(super) fn package_methods(
        &mut self,
        workspace: &'a Workspace,
        file: &'a Path,
        owner: &'a str,
        required: &[&str],
    ) -> Option<Vec<(&'a PathBuf, &'a Symbol)>> {
        if !self.package_methods_complete {
            return None;
        }
        let inventory = self.inventory(workspace, file, required)?;
        Some(inventory.methods.get(owner).cloned().unwrap_or_default())
    }

    pub(super) fn resolve_declaration(
        &mut self,
        workspace: &'a Workspace,
        file: &'a Path,
        name: &'a str,
    ) -> Option<(&'a PathBuf, &'a Symbol)> {
        if let Some(cached) = self.declarations.get(&(file, name)) {
            self.resolution_hits += 1;
            return *cached;
        }
        if let Some((alias, member)) = name.split_once('.') {
            if let Some(import_path) = self.resolve_import_path(workspace, file, alias) {
                let is_test_self = is_external_test_file(
                    file,
                    workspace
                        .files
                        .get(file)
                        .map_or("", |stub| stub.package.as_str()),
                ) && workspace.import_path(file).as_deref() == Some(import_path);
                if !is_test_self {
                    if let Some(&cached) = self.imported_declarations.get(&(import_path, member)) {
                        self.resolution_hits += 1;
                        self.qualified_fast += 1;
                        return cached;
                    }
                    self.resolution_calls += 1;
                    self.declaration_calls += 1;
                    self.qualified_slow += 1;
                    let resolved = self.view.resolve_type_declaration(file, name);
                    if self.imported_declarations.len() < 16_384 {
                        self.imported_declarations
                            .insert((import_path, member), resolved);
                    }
                    if self.declarations.len() < 16_384 {
                        self.declarations.insert((file, name), resolved);
                    }
                    return resolved;
                }
            }
            self.resolution_calls += 1;
            self.declaration_calls += 1;
            self.qualified_slow += 1;
            let resolved = self.view.resolve_type_declaration(file, name);
            if self.declarations.len() < 16_384 {
                self.declarations.insert((file, name), resolved);
            }
            return resolved;
        }
        if let Some(stub) = workspace.files.get(file)
            && !stub.imports.iter().any(|import| import.is_dot())
        {
            let pkg_key = (file.parent(), stub.package.as_str(), name);
            if let Some(&cached) = self.pkg_declarations.get(&pkg_key) {
                self.resolution_hits += 1;
                self.unqualified_fast += 1;
                return cached;
            }
            self.resolution_calls += 1;
            self.declaration_calls += 1;
            self.unqualified_slow += 1;
            let resolved = self.view.resolve_type_declaration(file, name);
            if self.pkg_declarations.len() < 16_384 {
                self.pkg_declarations.insert(pkg_key, resolved);
            }
            if self.declarations.len() < 16_384 {
                self.declarations.insert((file, name), resolved);
            }
            return resolved;
        }
        self.resolution_calls += 1;
        self.declaration_calls += 1;
        self.unqualified_slow += 1;
        let resolved = self.view.resolve_type_declaration(file, name);
        if self.declarations.len() < 16_384 {
            self.declarations.insert((file, name), resolved);
        }
        resolved
    }

    fn resolve_import_path(
        &mut self,
        workspace: &'a Workspace,
        file: &'a Path,
        alias: &str,
    ) -> Option<&'a str> {
        let stub = workspace.files.get(file)?;
        for import in &stub.imports {
            if import.is_dot() || import.is_blank() {
                continue;
            }
            let effective = if let Some(explicit) = import.explicit_name() {
                explicit
            } else {
                let path = import.path.as_str();
                let declared = if let Some(&cached) = self.import_pkg_names.get(path) {
                    cached
                } else {
                    let computed = self.view.declared_package_name(file, path);
                    if self.import_pkg_names.len() < 4_096 {
                        self.import_pkg_names.insert(path, computed);
                    }
                    computed
                };
                declared.or_else(|| path.rsplit('/').next())?
            };
            if effective == alias {
                return Some(import.path.as_str());
            }
        }
        None
    }

    /// Sound lower bound of 64-bit `TypeInfo::method_name_bit` masks required by
    /// an interface (direct methods union transitively embedded interfaces and
    /// unshadowed `error`). Any unknown/unresolvable branch contributes 0, so a
    /// concrete type `T` implementing `owner` MUST satisfy
    /// `(target_method_mask & lower_bound) == lower_bound`.
    pub(super) fn interface_required_mask(
        &mut self,
        workspace: &'a Workspace,
        file: &'a Path,
        owner: &'a Symbol,
        shadows: &HashSet<(Option<&'a Path>, &'a str)>,
        shadows_complete: bool,
    ) -> u64 {
        self.interface_required_mask_inner(workspace, file, owner, 0, shadows, shadows_complete)
    }

    fn interface_required_mask_inner(
        &mut self,
        workspace: &'a Workspace,
        file: &'a Path,
        owner: &'a Symbol,
        depth: usize,
        shadows: &HashSet<(Option<&'a Path>, &'a str)>,
        shadows_complete: bool,
    ) -> u64 {
        if depth > 32 {
            return 0;
        }
        let key = (file, owner.start);
        if let Some(&cached) = self.iface_required_memo.get(&key) {
            return cached;
        }
        if !self.iface_required_active.insert(key) {
            return 0;
        }
        let mut mask = 0u64;
        if let Some(info) = &owner.type_info {
            mask |= info.direct_methods_mask;
            if info.is_interface() {
                let unshadowed = shadows_complete
                    && workspace.files.get(file).is_some_and(|stub| {
                        !shadows.contains(&(file.parent(), stub.package.as_str()))
                    });
                for embedded in &info.embedded {
                    let spelling = embedded.trim_start();
                    if spelling.starts_with('*')
                        || !matches!(constructor_class(spelling), ConstructorClass::Named)
                    {
                        continue;
                    }
                    if !spelling.contains('.') {
                        let name = spelling
                            .split(['[', ' ', '|', '~', '(', ')'])
                            .next()
                            .unwrap_or(spelling);
                        if name == "error" && unshadowed {
                            mask |= TypeInfo::method_name_bit("Error");
                            continue;
                        }
                    }
                    let Some(resolved_name) = named_root_candidate(spelling) else {
                        continue;
                    };
                    let Some((next_file, next)) =
                        self.resolve_declaration(workspace, file, resolved_name)
                    else {
                        continue;
                    };
                    if next.alias && next.generic {
                        continue;
                    }
                    mask |= self.interface_required_mask_inner(
                        workspace,
                        next_file,
                        next,
                        depth + 1,
                        shadows,
                        shadows_complete,
                    );
                }
            } else if !(owner.alias && owner.generic)
                && let Some(target) = &info.target
            {
                let spelling = target.name.trim_start();
                if !spelling.starts_with('*')
                    && matches!(constructor_class(spelling), ConstructorClass::Named)
                {
                    let name = locals::named_root(spelling);
                    let unshadowed = !name.contains('.')
                        && shadows_complete
                        && workspace.files.get(file).is_some_and(|stub| {
                            !shadows.contains(&(file.parent(), stub.package.as_str()))
                        });
                    if name == "error" && unshadowed {
                        mask |= TypeInfo::method_name_bit("Error");
                    } else if let Some((next_file, next)) =
                        self.resolve_declaration(workspace, file, name)
                        && !(next.alias && next.generic)
                    {
                        mask |= self.interface_required_mask_inner(
                            workspace,
                            next_file,
                            next,
                            depth + 1,
                            shadows,
                            shadows_complete,
                        );
                    }
                }
            }
        }
        self.iface_required_active.remove(&key);
        if self.iface_required_memo.len() < 32_768 {
            self.iface_required_memo.insert(key, mask);
        }
        mask
    }

    /// Follow only known named indirections to decide whether a candidate type
    /// could possibly have an interface underlying type. Unresolved/opaque shapes
    /// remain candidates (`true`).
    pub(super) fn possible_interface(
        &mut self,
        workspace: &'a Workspace,
        mut file: &'a Path,
        mut symbol: &'a Symbol,
        shadows: &HashSet<(Option<&'a Path>, &'a str)>,
        shadows_complete: bool,
    ) -> bool {
        for _ in 0..64 {
            if symbol.constructor == TypeConstructor::Interface {
                return true;
            }
            if !matches!(
                symbol.constructor,
                TypeConstructor::Unknown | TypeConstructor::Named
            ) {
                return false;
            }
            let Some(info) = &symbol.type_info else {
                return true;
            };
            if info.is_interface() {
                return true;
            }
            let Some(target) = &info.target else {
                return false;
            };
            let spelling = target.name.trim_start();
            if spelling.starts_with('*') {
                return false;
            }
            match constructor_class(spelling) {
                ConstructorClass::Interface => return true,
                ConstructorClass::Other => return false,
                ConstructorClass::Named => {}
            }
            let name = target.name.trim();
            if shadows_complete
                && !name.contains('.')
                && let Some(stub) = workspace.files.get(file)
                && !shadows.contains(&(file.parent(), stub.package.as_str()))
            {
                if matches!(name, "any" | "error" | "comparable") {
                    return true;
                }
                if builtin(name) {
                    return false;
                }
            }
            let root = locals::named_root(name);
            let Some((origin, next)) = self.resolve_declaration(workspace, file, root) else {
                return !builtin(root);
            };
            file = origin;
            symbol = next;
        }
        true
    }

    /// Proves which required method names the candidate (own or promoted via
    /// declared embeddings) can provide. `None` means unknown and must stay a
    /// candidate; no type-name, package or direct-import heuristic is used.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn coverage(
        &mut self,
        workspace: &'a Workspace,
        file: &'a Path,
        owner: &'a Symbol,
        required: &[&str],
        depth: usize,
        shadows: &HashSet<(Option<&'a Path>, &'a str)>,
        shadows_complete: bool,
    ) -> Option<u64> {
        let key = (file, owner.start);
        if let Some(value) = self.coverage_memo.get(&key) {
            return *value;
        }
        self.coverage_calls += 1;
        if !self.coverage_active.insert(key) {
            return None;
        }
        let value = self.coverage_inner(
            workspace,
            file,
            owner,
            required,
            depth,
            shadows,
            shadows_complete,
        );
        self.coverage_active.remove(&key);
        if value.is_none() {
            self.coverage_unknowns += 1;
        }
        if self.coverage_memo.len() < 32_768 {
            self.coverage_memo.insert(key, value);
        }
        value
    }

    /// Required-name mask contributed by one written embedded type. Embedded
    /// fields are always named types (or pointers to them), so a slice, array,
    /// map, func, chan or type-set spelling can only contribute nothing. Named
    /// spellings resolve with the same query-local memo as the coverage walk.
    #[allow(clippy::too_many_arguments)]
    fn embedded_coverage(
        &mut self,
        workspace: &'a Workspace,
        file: &'a Path,
        embedded: &'a str,
        required: &[&str],
        depth: usize,
        shadows: &HashSet<(Option<&'a Path>, &'a str)>,
        shadows_complete: bool,
    ) -> Option<u64> {
        let spelling = embedded.trim_start();
        let spelling = spelling.strip_prefix('*').map_or(spelling, str::trim_start);
        // Embedded fields are always named types or pointers to them; a written
        // slice/array/map/func/chan/type-set spelling contributes nothing and a
        // written interface cannot be proved without resolving it. Byte-level
        // keyword classification avoids a lexer allocation on every edge.
        match constructor_class(spelling) {
            ConstructorClass::Named => {}
            ConstructorClass::Interface => return None,
            ConstructorClass::Other => return Some(0),
        }
        if !spelling.contains('.')
            && shadows_complete
            && workspace
                .files
                .get(file)
                .is_some_and(|stub| !shadows.contains(&(file.parent(), stub.package.as_str())))
        {
            let name = spelling
                .split(['[', ' ', '|', '~', '(', ')'])
                .next()
                .unwrap_or(spelling);
            if let Some(mask) = predeclared_mask(name, required) {
                return Some(mask);
            }
        }
        // Generic instantiations have the same method names as their root type,
        // so name coverage resolves the root once instead of the full spelling.
        // ASCII named spellings are validated by a bounded byte scan; Unicode
        // identifiers keep the full lexer-backed check.
        let resolved_name = match named_root_candidate(spelling) {
            Some(root) => root,
            None => {
                locals::named_type(spelling)?;
                locals::named_root(spelling)
            }
        };
        let (next_file, next) = self.resolve_declaration(workspace, file, resolved_name)?;
        // Generic aliases may substitute a formal receiver. Metadata alone
        // cannot prove their underlying method inventory; use native lookup.
        if next.alias
            && next.generic
            && next
                .type_info
                .as_ref()
                .is_none_or(|info| info.target.is_some())
        {
            return None;
        }
        self.coverage(
            workspace,
            next_file,
            next,
            required,
            depth,
            shadows,
            shadows_complete,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn coverage_inner(
        &mut self,
        workspace: &'a Workspace,
        file: &'a Path,
        owner: &'a Symbol,
        required: &[&str],
        depth: usize,
        shadows: &HashSet<(Option<&'a Path>, &'a str)>,
        shadows_complete: bool,
    ) -> Option<u64> {
        if depth > 32 || required.len() > 64 {
            return None;
        }
        // Cheap negative fact first: a candidate without metadata can never
        // prove name coverage, so the package inventory/mask walk is unnecessary.
        let Some(info) = &owner.type_info else {
            return None;
        };
        let full = if required.len() == 64 {
            u64::MAX
        } else {
            (1u64 << required.len()) - 1
        };
        let package = workspace.files.get(file)?.package.as_str();
        if self.is_alias_unknown(file, package) {
            return None;
        }
        let key = (file.parent(), package, owner.name.as_str());
        let own = if let Some(mask) = self.restricted_mask(file, package, owner.name.as_str()) {
            mask
        } else {
            match self.owner_masks.get(&key) {
                Some(mask) => *mask,
                None if self.masks_complete => 0,
                None => self.package_mask(workspace, file, owner.name.as_str(), required)?,
            }
        };
        // Struct `members` are field names, not methods: they can never satisfy
        // a required method name. Only interface inventories contribute directly;
        // methods on concrete types stay in the receiver-name masks above.
        let members = if info.is_interface()
            && (info.direct_methods_mask & self.required_bitmask != 0 || !info.embeddings_complete)
        {
            name_mask(&info.members, required)
        } else {
            0
        };
        let mut coverage = own | members;
        if coverage == full {
            return Some(full);
        }
        if info.is_interface() {
            if !info.embeddings_complete {
                return None;
            }
            for embedded in &info.embedded {
                coverage |= self.embedded_coverage(
                    workspace,
                    file,
                    embedded,
                    required,
                    depth + 1,
                    shadows,
                    shadows_complete,
                )?;
                if coverage == full {
                    return Some(full);
                }
            }
            return Some(coverage);
        }
        if !info.embeddings_complete {
            return None;
        }
        if let Some(target) = &info.target {
            let name = locals::named_root(&target.name);
            // Unqualified predeclared names add a fixed method inventory unless
            // this package shadows the spelling; resolve only then.
            let universe = !target.name.contains('.')
                && shadows_complete
                && workspace
                    .files
                    .get(file)
                    .is_some_and(|stub| !shadows.contains(&(file.parent(), stub.package.as_str())));
            let predeclared = if universe {
                predeclared_mask(name, required)
            } else {
                None
            };
            match predeclared {
                Some(mask) => coverage |= mask,
                None => {
                    // Generic instantiations share the root type's method names.
                    let (next_file, next) = self.resolve_declaration(workspace, file, name)?;
                    coverage |= self.coverage(
                        workspace,
                        next_file,
                        next,
                        required,
                        depth + 1,
                        shadows,
                        shadows_complete,
                    )?;
                }
            }
            if coverage == full {
                return Some(full);
            }
        }
        for embedded in &info.embedded {
            coverage |= self.embedded_coverage(
                workspace,
                file,
                embedded,
                required,
                depth + 1,
                shadows,
                shadows_complete,
            )?;
            if coverage == full {
                return Some(full);
            }
        }
        Some(coverage)
    }

    /// Fallback narrowing when the global receiver-owner index overflows: a
    /// positive is proved from the candidate package's own method declarations
    /// and metadata embeddings, so no complete owner inventory is required.
    pub(super) fn may_match_ordinary(
        &mut self,
        workspace: &'a Workspace,
        file: &'a Path,
        owner: &'a Symbol,
        required: &[&str],
    ) -> bool {
        let mut pending = vec![(file, owner)];
        let mut seen = HashSet::new();
        while let Some((file, owner)) = pending.pop() {
            if self.absent.contains(&(file, owner.start)) || !seen.insert((file, owner.start)) {
                continue;
            }
            if seen.len() > 4096 {
                return true;
            }
            let package = workspace.files[file].package.as_str();
            if self.is_alias_unknown(file, package) {
                return true;
            }
            let own_match = if let Some(mask) = self.restricted_mask(file, package, &owner.name) {
                mask != 0
            } else {
                self.matching
                    .contains(&(file.parent(), package, owner.name.as_str()))
                    || declares_required(self.view, file, &owner.name, required)
            };
            if own_match {
                return true;
            }
            let Some(info) = &owner.type_info else {
                return true;
            };
            if info.is_interface()
                && info
                    .members
                    .iter()
                    .any(|name| required.contains(&name.as_str()))
            {
                return true;
            }
            if !info.embeddings_complete {
                return true;
            }
            for name in info
                .target
                .iter()
                .map(|target| target.name.as_str())
                .chain(info.embedded.iter().map(String::as_str))
            {
                if name.contains('[') || info.target.as_ref().is_some_and(|target| target.generic) {
                    return true;
                }
                let Some(name) = locals::named_type(name) else {
                    return true;
                };
                if let Some((origin, target)) = self.view.resolve_type_declaration(file, &name) {
                    pending.push((origin.as_path(), target));
                    if pending.len() > 4096 {
                        return true;
                    }
                } else if name == "error" || !builtin(&name) {
                    return true;
                }
            }
        }
        // Reuse only proved negatives, and discard them with this request.
        for identity in seen {
            if self.absent.len() == 16_384 {
                break;
            }
            self.absent.insert(identity);
        }
        false
    }

    /// A positive result is only a candidate. A negative result requires a
    /// complete inventory at every reachable declaration; unknowns stay positive.
    pub(super) fn may_match(
        &mut self,
        workspace: &'a Workspace,
        file: &'a Path,
        owner: &'a Symbol,
        required: &[&str],
    ) -> bool {
        let mut pending = vec![(file, owner)];
        let mut seen = HashSet::new();
        while let Some((file, owner)) = pending.pop() {
            if self.absent.contains(&(file, owner.start)) || !seen.insert((file, owner.start)) {
                continue;
            }
            if seen.len() > 4096 {
                return true;
            }
            let package = workspace.files[file].package.as_str();
            if self.is_alias_unknown(file, package)
                || self
                    .matching
                    .contains(&(file.parent(), package, owner.name.as_str()))
            {
                return true;
            }
            let Some(info) = &owner.type_info else {
                return true;
            };
            if info.is_interface()
                && info
                    .members
                    .iter()
                    .any(|name| required.contains(&name.as_str()))
            {
                return true;
            }
            if !info.embeddings_complete {
                return true;
            }
            for name in info
                .target
                .iter()
                .map(|target| target.name.as_str())
                .chain(info.embedded.iter().map(String::as_str))
            {
                // Generic bindings and opaque type spellings cannot establish
                // a negative proof from declaration identities alone.
                if name.contains('[') || info.target.as_ref().is_some_and(|target| target.generic) {
                    return true;
                }
                let Some(name) = locals::named_type(name) else {
                    return true;
                };
                if let Some((origin, target)) =
                    crate::analysis::resolve::resolve_type_declaration(workspace, file, &name)
                {
                    pending.push((origin.as_path(), target));
                    if pending.len() > 4096 {
                        return true;
                    }
                } else if name == "error" || !builtin(&name) {
                    return true;
                }
            }
        }
        // Reuse only proved negatives, and discard them with this request.
        for identity in seen {
            if self.absent.len() == 16_384 {
                break;
            }
            self.absent.insert(identity);
        }
        false
    }
}

/// Method names contributed by an unshadowed predeclared type spelling.
fn predeclared_mask(name: &str, required: &[&str]) -> Option<u64> {
    if name == "error" {
        return Some(mask_names(&["Error"], required));
    }
    if matches!(name, "any" | "comparable") {
        return Some(0);
    }
    const BASIC: &[&str] = &[
        "bool",
        "byte",
        "rune",
        "string",
        "int",
        "int8",
        "int16",
        "int32",
        "int64",
        "uint",
        "uint8",
        "uint16",
        "uint32",
        "uint64",
        "uintptr",
        "float32",
        "float64",
        "complex64",
        "complex128",
    ];
    BASIC.contains(&name).then_some(0)
}

fn mask_names(names: &[&str], required: &[&str]) -> u64 {
    let mut mask = 0u64;
    for name in names {
        if let Some(bit) = required.iter().position(|required| required == name)
            && bit < 64
        {
            mask |= 1u64 << bit;
        }
    }
    mask
}

/// Bits of the required method names present in the declaration inventory.
/// Root of an ASCII named type spelling (`Name`, `pkg.Name`, `Name[Args]`),
/// validated without allocating. `None` defers to the lexer-backed check.
fn named_root_candidate(spelling: &str) -> Option<&str> {
    let text = spelling.trim();
    let bytes = text.as_bytes();
    let mut index = 0;
    if bytes.first() == Some(&b'*') {
        index += 1;
        while bytes
            .get(index)
            .is_some_and(|byte| byte.is_ascii_whitespace())
        {
            index += 1;
        }
    }
    let start = index;
    while bytes
        .get(index)
        .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
    {
        index += 1;
    }
    if index == start {
        return None;
    }
    let mut root_end = index;
    if bytes.get(index) == Some(&b'.') {
        index += 1;
        let member = index;
        while bytes
            .get(index)
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
        {
            index += 1;
        }
        if index == member {
            return None;
        }
        root_end = index;
    }
    if bytes.get(index) == Some(&b'[') {
        let mut depth = 0i32;
        while let Some(byte) = bytes.get(index) {
            match byte {
                b'[' => depth += 1,
                b']' => {
                    depth -= 1;
                    if depth == 0 {
                        index += 1;
                        break;
                    }
                }
                _ => {}
            }
            index += 1;
        }
        if depth != 0 {
            return None;
        }
    }
    if bytes
        .get(index)
        .is_some_and(|byte| !byte.is_ascii_whitespace())
    {
        return None;
    }
    Some(text[..root_end].trim_end())
}

fn name_mask(names: &[String], required: &[&str]) -> u64 {
    let full = if required.len() >= 64 {
        u64::MAX
    } else {
        (1u64 << required.len()) - 1
    };
    let mut mask = 0u64;
    for name in names {
        if let Some(bit) = required
            .iter()
            .position(|required| *required == name.as_str())
            && bit < 64
        {
            mask |= 1u64 << bit;
            if mask == full {
                break;
            }
        }
    }
    mask
}

/// First-token class of a written type spelling, enough to decide whether an
/// embedded type can contribute interface methods without tokenizing.
enum ConstructorClass {
    /// Named (possibly qualified/generic) spelling: needs resolution.
    Named,
    /// Written interface type: method inventory remains unknown until resolved.
    Interface,
    /// Written constructors that are never an interface method source.
    Other,
}

fn constructor_class(spelling: &str) -> ConstructorClass {
    let bytes = spelling.as_bytes();
    match bytes.first() {
        None => return ConstructorClass::Other,
        Some(b'[' | b'~' | b'<') => return ConstructorClass::Other,
        _ => {}
    }
    let ident = bytes
        .iter()
        .take_while(|byte| byte.is_ascii_alphanumeric() || **byte == b'_')
        .count();
    match &spelling[..ident] {
        "map" | "chan" | "func" | "struct" | "interface" => {
            // An identifier that merely starts with a keyword is a named type.
            if bytes
                .get(ident)
                .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
            {
                return ConstructorClass::Named;
            }
            if spelling.starts_with("interface") {
                ConstructorClass::Interface
            } else {
                ConstructorClass::Other
            }
        }
        _ => ConstructorClass::Named,
    }
}

/// Whether the package declares a method with a required name on the receiver.
fn declares_required(
    view: crate::analysis::resolve::View<'_>,
    file: &Path,
    owner: &str,
    required: &[&str],
) -> bool {
    view.package_symbols(file).any(|(_, symbol)| {
        symbol.receiver.as_deref() == Some(owner) && required.contains(&symbol.name.as_str())
    })
}

fn builtin(name: &str) -> bool {
    matches!(
        name,
        "bool"
            | "byte"
            | "rune"
            | "string"
            | "int"
            | "int8"
            | "int16"
            | "int32"
            | "int64"
            | "uint"
            | "uint8"
            | "uint16"
            | "uint32"
            | "uint64"
            | "uintptr"
            | "float32"
            | "float64"
            | "complex64"
            | "complex128"
            | "any"
            | "comparable"
    )
}

#[cfg(test)]
mod tests {
    use super::{Candidates, PackageReachability};
    use crate::analysis::query::Context;
    use crate::syntax::TypeInfo;
    use crate::workspace::{self, Workspace};
    use serde_json::json;
    use std::collections::HashSet;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    struct TempMod {
        root: PathBuf,
    }

    impl TempMod {
        fn new(label: &str, files: &[(&str, &str)]) -> Self {
            let id = COUNTER.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "goquery-impl-pruning-{label}-{}-{id}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            for (rel, content) in files {
                let path = root.join(rel);
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).unwrap();
                }
                std::fs::write(path, content).unwrap();
            }
            Self { root }
        }

        fn workspace(&self) -> Workspace {
            let mut ws = Workspace::default();
            ws.initialize(&json!({
                "rootUri": workspace::uri(&self.root),
                "initializationOptions": {
                    "goroot": "",
                    "fileWatch": false,
                }
            }))
            .unwrap();
            ws
        }
    }

    impl Drop for TempMod {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn find_file<'a>(ws: &'a Workspace, suffix: &str) -> &'a Path {
        ws.files
            .keys()
            .find(|path| path.to_string_lossy().replace('\\', "/").ends_with(suffix))
            .unwrap_or_else(|| panic!("missing file {suffix}"))
            .as_path()
    }

    fn relation_names(ws: &Workspace, suffix: &str, type_name: &str, supers: bool) -> Vec<String> {
        let file = find_file(ws, suffix);
        let sym = ws.files[file]
            .symbols
            .iter()
            .find(|s| s.is_type() && s.name == type_name)
            .unwrap_or_else(|| panic!("missing type {type_name} in {suffix}"));
        let mut ctx = Context::metadata(ws);
        ctx.select_query_file(file).unwrap();
        let mut names: Vec<String> = ctx
            .type_relations(file, sym, supers)
            .unwrap()
            .into_iter()
            .map(|target| target.symbol.name.clone())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn package_reachability_prunes_unrelated_and_preserves_transitive_embeddings() {
        let dir = TempMod::new(
            "reach",
            &[
                ("go.mod", "module example.test/mod\n\ngo 1.24\n"),
                (
                    "iface/iface.go",
                    "package iface\ntype Parser interface {\n\tParseExpr() int\n\tParseMetric() int\n}\n",
                ),
                (
                    "core/core.go",
                    "package core\ntype Base struct{}\nfunc (*Base) ParseExpr() int { return 0 }\n",
                ),
                (
                    "mid/mid.go",
                    "package mid\nimport \"example.test/mod/core\"\ntype Mid struct { core.Base }\nfunc (Mid) ParseMetric() int { return 0 }\n",
                ),
                (
                    "leaf/leaf.go",
                    "package leaf\nimport \"example.test/mod/mid\"\ntype Leaf struct { mid.Mid }\n",
                ),
                (
                    "partial/partial.go",
                    "package partial\ntype Partial struct{}\nfunc (Partial) ParseExpr() int { return 0 }\n",
                ),
                (
                    "unrelated/unrelated.go",
                    "package unrelated\ntype Unrelated struct{}\nfunc (Unrelated) Other() {}\n",
                ),
            ],
        );
        let ws = dir.workspace();
        let iface_file = find_file(&ws, "iface/iface.go");
        let required = ["ParseExpr", "ParseMetric"];
        let bitmask =
            TypeInfo::method_name_bit("ParseExpr") | TypeInfo::method_name_bit("ParseMetric");
        let reach = PackageReachability::build(&ws, iface_file, &required, bitmask, true)
            .expect("reachability index");

        assert!(
            !reach.package_may_implement(find_file(&ws, "unrelated/unrelated.go"), "unrelated")
        );
        assert!(!reach.package_may_implement(find_file(&ws, "partial/partial.go"), "partial"));
        assert!(!reach.package_may_implement(find_file(&ws, "core/core.go"), "core"));
        assert!(reach.package_may_implement(find_file(&ws, "mid/mid.go"), "mid"));
        assert!(reach.package_may_implement(find_file(&ws, "leaf/leaf.go"), "leaf"));

        assert_eq!(
            relation_names(&ws, "iface/iface.go", "Parser", false),
            vec!["Leaf".to_owned(), "Mid".to_owned()]
        );
    }

    #[test]
    fn embedded_interfaces_compute_sound_supertype_lower_bounds() {
        let dir = TempMod::new(
            "supers",
            &[
                ("go.mod", "module example.test/mod\n\ngo 1.24\n"),
                (
                    "sup/sup.go",
                    "package sup\n\
                     type A interface { Alpha() }\n\
                     type B interface { Beta() }\n\
                     type AB interface { A; B }\n\
                     type ABC interface { AB; Gamma() }\n\
                     type DefAB AB\n\
                     type ErrWithCode interface { error; Code() int }\n\
                     type OnlyAlpha struct{}\n\
                     func (OnlyAlpha) Alpha() {}\n\
                     type Both struct{}\n\
                     func (Both) Alpha() {}\n\
                     func (Both) Beta() {}\n\
                     type MyErr struct{}\n\
                     func (MyErr) Error() string { return \"\" }\n\
                     func (MyErr) Code() int { return 0 }\n",
                ),
            ],
        );
        let ws = dir.workspace();
        let file = find_file(&ws, "sup/sup.go");
        let mut candidates =
            Candidates::new(crate::analysis::resolve::View::for_file(&ws, file), 0);
        let shadows = HashSet::new();
        let sym = |name: &str| {
            ws.files[file]
                .symbols
                .iter()
                .find(|s| s.is_type() && s.name == name)
                .unwrap()
        };

        let ab_lb = candidates.interface_required_mask(&ws, file, sym("AB"), &shadows, true);
        let def_ab_lb = candidates.interface_required_mask(&ws, file, sym("DefAB"), &shadows, true);
        let err_lb =
            candidates.interface_required_mask(&ws, file, sym("ErrWithCode"), &shadows, true);

        let alpha_beta = TypeInfo::method_name_bit("Alpha") | TypeInfo::method_name_bit("Beta");
        assert_eq!(ab_lb & alpha_beta, alpha_beta);
        assert_eq!(def_ab_lb & alpha_beta, alpha_beta);
        let err_code = TypeInfo::method_name_bit("Error") | TypeInfo::method_name_bit("Code");
        assert_eq!(err_lb & err_code, err_code);

        // OnlyAlpha's mask fails the lower-bound test for AB before MethodSet::new.
        let only_alpha_mask = TypeInfo::method_name_bit("Alpha");
        assert_ne!(only_alpha_mask & ab_lb, ab_lb);

        assert_eq!(
            relation_names(&ws, "sup/sup.go", "Both", true),
            vec![
                "A".to_owned(),
                "AB".to_owned(),
                "B".to_owned(),
                "DefAB".to_owned(),
            ]
        );
        assert_eq!(
            relation_names(&ws, "sup/sup.go", "MyErr", true),
            vec!["ErrWithCode".to_owned()]
        );
    }

    #[test]
    fn opaque_interface_fallback_and_over_64_methods_stay_sound() {
        // 1. Opaque interfaces (nested anonymous interface / parenthesized) fall
        //    back to IdentifierFilter: never reject when identifier is present,
        //    and still reject opaque packages that lack the identifier.
        let dir = TempMod::new(
            "opaque",
            &[
                ("go.mod", "module example.test/mod\n\ngo 1.24\n"),
                (
                    "iface/iface.go",
                    "package iface\ntype Target interface { Magic() }\n",
                ),
                (
                    "op/op.go",
                    "package op\n\
                     type Base[T any] interface { Magic() }\n\
                     type OpaqueSub interface { Base[int] }\n\
                     type EmbedsOpaque struct { OpaqueSub }\n",
                ),
                (
                    "other/other.go",
                    "package other\n\
                     type OtherOpaque (interface { SomethingElse() })\n",
                ),
            ],
        );
        let ws = dir.workspace();
        let iface_file = find_file(&ws, "iface/iface.go");
        let op_file = find_file(&ws, "op/op.go");
        let other_file = find_file(&ws, "other/other.go");
        assert!(ws.files[op_file].methods_opaque);
        assert!(ws.files[other_file].methods_opaque);
        let reach = PackageReachability::build(
            &ws,
            iface_file,
            &["Magic"],
            TypeInfo::method_name_bit("Magic"),
            true,
        )
        .expect("reachability index");
        assert!(reach.package_may_implement(op_file, "op"));
        assert!(!reach.package_may_implement(other_file, "other"));
        let subs = relation_names(&ws, "iface/iface.go", "Target", false);
        assert!(subs.contains(&"Base".to_owned()), "got {subs:?}");
        assert!(subs.contains(&"OpaqueSub".to_owned()), "got {subs:?}");
        assert!(subs.contains(&"EmbedsOpaque".to_owned()), "got {subs:?}");

        // 2. Interfaces with > 64 required methods still prune unreachable packages via 64-bit hash mask.
        let mut iface_src = String::from("package big\ntype Huge interface {\n");
        let mut impl_src = String::from("package ok\ntype All struct{}\n");
        let mut names = Vec::new();
        for i in 0..70 {
            let name = format!("Method{i:02}");
            iface_src.push_str(&format!("\t{name}()\n"));
            impl_src.push_str(&format!("func (All) {name}() {{}}\n"));
            names.push(name);
        }
        iface_src.push_str("}\n");
        let dir_big = TempMod::new(
            "over64",
            &[
                ("go.mod", "module example.test/big\n\ngo 1.24\n"),
                ("big/big.go", &iface_src),
                ("ok/ok.go", &impl_src),
                ("nope/nope.go", "package nope\ntype Empty struct{}\n"),
            ],
        );
        let ws_big = dir_big.workspace();
        let big_file = find_file(&ws_big, "big/big.go");
        let required: Vec<&str> = names.iter().map(String::as_str).collect();
        let bitmask = required
            .iter()
            .fold(0u64, |acc, name| acc | TypeInfo::method_name_bit(name));
        let reach_big = PackageReachability::build(&ws_big, big_file, &required, bitmask, false)
            .expect("over-64 reachability index");
        assert!(reach_big.package_may_implement(find_file(&ws_big, "ok/ok.go"), "ok"));
        assert!(!reach_big.package_may_implement(find_file(&ws_big, "nope/nope.go"), "nope"));
        assert_eq!(
            relation_names(&ws_big, "big/big.go", "Huge", false),
            vec!["All".to_owned()]
        );
    }
}
