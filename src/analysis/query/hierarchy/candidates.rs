//! Conservative metadata reachability, independent of promotion and visibility.
use crate::analysis::scope as locals;
use crate::index::IndexedSymbol as Symbol;
use crate::workspace::Workspace;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

pub(super) type OwnerKey<'a> = (Option<&'a Path>, &'a str, &'a str);
type Restricted<'a> = (u64, Vec<(&'a PathBuf, &'a Symbol)>);
type TypeIndex<'a> =
    HashMap<(Option<&'a Path>, &'a str), HashMap<&'a str, (&'a PathBuf, &'a Symbol)>>;

pub(super) struct Candidates<'a> {
    pub(super) receivers: HashSet<OwnerKey<'a>>,
    pub(super) matching: HashSet<OwnerKey<'a>>,
    /// One bit per queried required method name, set for receiver owners that
    /// declare at least one of them. Query-local and bounded with the owners.
    pub(super) owner_masks: HashMap<OwnerKey<'a>, u64>,
    pub(super) masks_complete: bool,
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
    absent: HashSet<(&'a Path, usize)>,
    /// Query-local resolution memo shared by coverage walks: embedded and
    /// underlying type spellings repeat across candidates in one package.
    underlying: HashMap<(&'a Path, &'a str), Option<(&'a PathBuf, &'a Symbol)>>,
    declarations: HashMap<(&'a Path, &'a str), Option<(&'a PathBuf, &'a Symbol)>>,
    /// Qualified declarations repeat across candidate files that import the same
    /// package; key them by canonical import path and declared name instead of
    /// the consumer file. Alias-to-path lookups are memoized per file.
    imported_declarations: HashMap<(&'a str, &'a str), Option<(&'a PathBuf, &'a Symbol)>>,
    import_aliases: HashMap<(&'a Path, &'a str), Option<&'a str>>,
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
    /// Query-local same-package type-name index. Built once on demand so the
    /// coverage walk does not rescan package files for every embedded spelling.
    type_index: TypeIndex<'a>,
    type_index_entries: usize,
    type_index_complete: bool,
    type_index_built: bool,
    /// Query-local default-import-name memo; the index fast path checks import
    /// shadowing for every embedded spelling.
    import_names: HashMap<&'a str, Option<&'a str>>,
}

struct PackageInventory<'a> {
    masks: HashMap<&'a str, u64>,
    methods: HashMap<&'a str, Vec<(&'a PathBuf, &'a Symbol)>>,
}

impl<'a> Candidates<'a> {
    pub(super) fn new() -> Self {
        Self {
            receivers: HashSet::new(),
            matching: HashSet::new(),
            owner_masks: HashMap::new(),
            masks_complete: true,
            package_inventory: HashMap::new(),
            package_methods_complete: true,
            package_method_entries: 0,
            restricted: HashMap::new(),
            restricted_complete: true,
            coverage_memo: HashMap::new(),
            coverage_active: HashSet::new(),
            absent: HashSet::new(),
            underlying: HashMap::new(),
            declarations: HashMap::new(),
            imported_declarations: HashMap::new(),
            import_aliases: HashMap::new(),
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
            type_index: HashMap::new(),
            type_index_entries: 0,
            type_index_complete: true,
            type_index_built: false,
            import_names: HashMap::new(),
        }
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
        if !self.restricted_complete {
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
        if !self.restricted_complete {
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
            for (path, symbol) in workspace.package_symbols(file) {
                let Some(receiver) = &symbol.receiver else {
                    continue;
                };
                let Some(bit) = required
                    .iter()
                    .position(|name| *name == symbol.name.as_str())
                else {
                    continue;
                };
                *inventory.masks.entry(receiver.as_str()).or_insert(0) |= 1u64 << bit;
                if self.package_method_entries < 16_384 {
                    inventory
                        .methods
                        .entry(receiver.as_str())
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

    /// Bounded query-local resolution memo for coverage walks. Repeated embedded
    /// or underlying spellings within one request reuse the same declaration.
    fn resolve_underlying(
        &mut self,
        workspace: &'a Workspace,
        file: &'a Path,
        spelling: &'a str,
    ) -> Option<(&'a PathBuf, &'a Symbol)> {
        let key_file = resolution_scope(file, spelling);
        if let Some(cached) = self.underlying.get(&(key_file, spelling)) {
            self.resolution_hits += 1;
            return *cached;
        }
        self.resolution_calls += 1;
        self.underlying_calls += 1;
        let resolved = workspace.underlying_target(file, spelling);
        if self.underlying.len() < 4096 {
            self.underlying.insert((key_file, spelling), resolved);
        }
        resolved
    }

    fn resolve_declaration(
        &mut self,
        workspace: &'a Workspace,
        file: &'a Path,
        name: &'a str,
    ) -> Option<(&'a PathBuf, &'a Symbol)> {
        let key_file = resolution_scope(file, name);
        if let Some(cached) = self.declarations.get(&(key_file, name)) {
            self.resolution_hits += 1;
            return *cached;
        }
        // Imported declarations are keyed by the canonical import path, so the
        // same package type reached from different consumer files resolves once.
        if let Some((alias, declared)) = name.split_once('.')
            && let Some(import_path) = self.import_path(workspace, file, alias)
        {
            let key = (import_path, declared);
            if let Some(cached) = self.imported_declarations.get(&key) {
                self.resolution_hits += 1;
                return *cached;
            }
            self.resolution_calls += 1;
            self.declaration_calls += 1;
            let resolved = workspace.type_declaration(file, name);
            self.qualified_fast += 1;
            if self.imported_declarations.len() < 4_096 {
                self.imported_declarations.insert(key, resolved);
            }
            return resolved;
        }
        self.resolution_calls += 1;
        self.declaration_calls += 1;
        let indexed = self.indexed_type(workspace, file, name);
        if indexed.is_some() {
            self.unqualified_fast += 1;
        } else {
            self.unqualified_slow += 1;
        }
        let resolved = indexed.or_else(|| workspace.type_declaration(file, name));
        if self.declarations.len() < 4096 {
            self.declarations.insert((key_file, name), resolved);
        }
        resolved
    }

    /// Canonical import path bound to `alias` in `file`, when the file imports
    /// it under that name. Ambiguous or unindexed aliases keep the slow path.
    fn import_path(
        &mut self,
        workspace: &'a Workspace,
        file: &'a Path,
        alias: &'a str,
    ) -> Option<&'a str> {
        if let Some(cached) = self.import_aliases.get(&(file, alias)) {
            return *cached;
        }
        let stub = workspace.files.get(file)?;
        let mut found = None;
        for import in &stub.imports {
            if import.is_dot() || import.is_blank() {
                continue;
            }
            let name = match import.explicit_name() {
                Some(alias) => Some(alias),
                None => {
                    if let Some(cached) = self.import_names.get(import.path.as_str()) {
                        *cached
                    } else {
                        let value = workspace.import_name(import);
                        if self.import_names.len() < 4_096 {
                            self.import_names.insert(import.path.as_str(), value);
                        }
                        value
                    }
                }
            };
            if name == Some(alias) {
                if found.is_some() {
                    found = None;
                    break;
                }
                found = Some(import.path.as_str());
            }
        }
        if self.import_aliases.len() < 4_096 {
            self.import_aliases.insert((file, alias), found);
        }
        found
    }

    /// Same-package type declaration from the query-local name index. Only
    /// unqualified names without an import alias/dot-import shadow use it; all
    /// other spellings keep the workspace resolver.
    fn indexed_type(
        &mut self,
        workspace: &'a Workspace,
        file: &'a Path,
        name: &'a str,
    ) -> Option<(&'a PathBuf, &'a Symbol)> {
        if name.contains('.') {
            return None;
        }
        let stub = workspace.files.get(file)?;
        for import in &stub.imports {
            if import.is_dot() {
                return None;
            }
            if import.explicit_name() == Some(name) {
                return None;
            }
            let path = import.path.as_str();
            let alias = match self.import_names.get(path) {
                Some(value) => *value,
                None => {
                    let value = workspace.import_name(import);
                    if self.import_names.len() < 4_096 {
                        self.import_names.insert(path, value);
                    }
                    value
                }
            };
            if alias == Some(name) {
                return None;
            }
        }
        self.ensure_type_index(workspace);
        if !self.type_index_complete {
            return None;
        }
        self.type_index
            .get(&(file.parent(), stub.package.as_str()))?
            .get(name)
            .copied()
    }

    fn ensure_type_index(&mut self, workspace: &'a Workspace) {
        if self.type_index_built {
            return;
        }
        self.type_index_built = true;
        const MAX_TYPES: usize = 65_536;
        for (path, stub) in &workspace.files {
            let entry = self
                .type_index
                .entry((path.parent(), stub.package.as_str()))
                .or_default();
            for symbol in &stub.symbols {
                if !symbol.is_type() || entry.contains_key(symbol.name.as_str()) {
                    continue;
                }
                if self.type_index_entries >= MAX_TYPES {
                    self.type_index_complete = false;
                    return;
                }
                entry.insert(symbol.name.as_str(), (path, symbol));
                self.type_index_entries += 1;
            }
        }
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
        if !spelling.contains('.') {
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
        // A same-package named type contributes its own receiver methods; only
        // aliases (and non-indexed spellings) need the underlying projection.
        // Resolve aliases from their declaration file so the query-local memo is
        // shared across consumer files instead of keyed by the spelling site.
        let (next_file, next) = match self.resolve_declaration(workspace, file, resolved_name) {
            Some((path, symbol)) if !symbol.alias => (path, symbol),
            Some((path, symbol)) => {
                self.resolve_underlying(workspace, path, symbol.name.as_str())?
            }
            None => self.resolve_underlying(workspace, file, resolved_name)?,
        };
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
        let members = if info.is_interface() {
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
            let name = name.rsplit('.').next().unwrap_or(name);
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
            if self
                .matching
                .contains(&(file.parent(), package, owner.name.as_str()))
                || declares_required(workspace, file, &owner.name, required)
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
                if name.contains('[') || info.target.as_ref().is_some_and(|target| target.generic) {
                    return true;
                }
                let Some(name) = locals::named_type(name) else {
                    return true;
                };
                if let Some((origin, target)) = workspace.type_declaration(file, &name) {
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
            if self
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
                if let Some((origin, target)) = workspace.type_declaration(file, &name) {
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

/// Unqualified spellings resolve inside their package, so all files of one
/// package share the memo key; qualified spellings keep their own file's imports.
fn resolution_scope<'a>(file: &'a Path, spelling: &str) -> &'a Path {
    if spelling.contains('.') {
        file
    } else {
        file.parent().unwrap_or(file)
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
fn declares_required(workspace: &Workspace, file: &Path, owner: &str, required: &[&str]) -> bool {
    workspace.package_symbols(file).any(|(_, symbol)| {
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
