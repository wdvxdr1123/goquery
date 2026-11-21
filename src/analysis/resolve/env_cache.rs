//! Request-scoped per-file environment cache unifying imports, package lookups, and type names.

use super::{UnqualifiedLookup, View};
use crate::analysis::engine::budget::MAX_DECL_CACHE;
use crate::analysis::engine::obj::DeclId;
use crate::analysis::engine::scope as locals;
use crate::workspace::{BuildView, Workspace};
use std::collections::{HashMap, HashSet};
use std::ops::{Bound, Range};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const MAX_ENV_CACHE_BYTES: usize = 64 * 1024;
const MAX_PKG_METHOD_INDICES: usize = 64;
const MAX_PKG_METHODS: usize = 4096;
const MAX_METHOD_HEADERS: usize = 64;
const MAX_METHOD_HEADER_BYTES: usize = 32 * 1024;

pub(crate) struct PkgMethodIndex<'a> {
    pub(crate) by_recv_and_name: HashMap<(&'a str, &'a str), Vec<DeclId<'a>>>,
    pub(crate) alias_methods_by_name: HashMap<&'a str, Vec<DeclId<'a>>>,
}

impl<'a> PkgMethodIndex<'a> {
    fn build(workspace: &'a Workspace, decl_file: &'a Path, view: BuildView<'a>) -> Option<Self> {
        let mut alias_names: HashSet<&'a str> = HashSet::new();
        let mut files: Vec<(&'a PathBuf, &'a crate::index::IndexedFile)> = Vec::new();
        for (file, stub) in workspace.package_files_in_view(decl_file, view) {
            for sym in &stub.symbols {
                if sym.is_type() && sym.alias {
                    alias_names.insert(sym.name.as_str());
                }
            }
            files.push((file, stub));
        }

        let mut by_recv_and_name: HashMap<(&'a str, &'a str), Vec<DeclId<'a>>> = HashMap::new();
        let mut alias_methods_by_name: HashMap<&'a str, Vec<DeclId<'a>>> = HashMap::new();
        let mut count = 0usize;
        for (file, stub) in files {
            let has_dot = stub.imports.iter().any(|import| import.is_dot());
            for method in &stub.symbols {
                if !method.is_method() {
                    continue;
                }
                count += 1;
                if count > MAX_PKG_METHODS {
                    return None;
                }
                let decl = DeclId {
                    file: file.as_path(),
                    symbol: method,
                };
                match method.receiver.as_deref() {
                    Some(recv) => {
                        by_recv_and_name
                            .entry((recv, method.name.as_str()))
                            .or_default()
                            .push(decl);
                        if has_dot || alias_names.contains(recv) {
                            alias_methods_by_name
                                .entry(method.name.as_str())
                                .or_default()
                                .push(decl);
                        }
                    }
                    None => {
                        alias_methods_by_name
                            .entry(method.name.as_str())
                            .or_default()
                            .push(decl);
                    }
                }
            }
        }
        Some(Self {
            by_recv_and_name,
            alias_methods_by_name,
        })
    }
}

pub(crate) struct MethodHeader {
    pub(crate) start: usize,
    pub(crate) text: String,
    pub(crate) parsed: crate::syntax::Signature,
    pub(crate) receiver_names: Vec<Range<usize>>,
}

#[derive(Default)]
struct MethodHeaders<'a> {
    entries: HashMap<DeclId<'a>, Arc<MethodHeader>>,
    bytes: usize,
}

/// Outer `Option`: `None` means shadowed by an import or non-type declaration
/// (universe lookup disallowed). Inner `Option`: `Some(decl)` for a resolved
/// type declaration, `None` for universe fallback.
pub(crate) type TypeNameLookup<'a> = Option<Option<DeclId<'a>>>;

#[derive(Default)]
struct FileEnvEntry<'a> {
    imports: HashMap<Box<str>, Option<&'a Path>>,
    imported_decls: HashMap<Box<str>, HashMap<Box<str>, Option<DeclId<'a>>>>,
    unqualified: HashMap<Box<str>, UnqualifiedLookup<'a>>,
    type_names: HashMap<Box<str>, TypeNameLookup<'a>>,
}

/// Bounded per-file lookup cache shared across [`DeclTable`] and type resolution.
pub(crate) struct FileEnvCache<'a> {
    view: BuildView<'a>,
    files: HashMap<&'a Path, FileEnvEntry<'a>>,
    entries: usize,
    bytes: usize,
    pkg_methods: HashMap<(&'a Path, &'a str, bool), Option<PkgMethodIndex<'a>>>,
    method_headers: MethodHeaders<'a>,
}

impl<'a> FileEnvCache<'a> {
    pub(crate) fn new(view: BuildView<'a>) -> Self {
        Self {
            view,
            files: HashMap::new(),
            entries: 0,
            bytes: 0,
            pkg_methods: HashMap::new(),
            method_headers: MethodHeaders::default(),
        }
    }

    pub(crate) fn pkg_method_index(
        &mut self,
        ws: &'a Workspace,
        decl_file: &'a Path,
    ) -> Option<&PkgMethodIndex<'a>> {
        let dir = decl_file.parent()?;
        let stub = ws.files.get(decl_file)?;
        let key = (dir, stub.package.as_str(), ws.is_detached(decl_file));
        if !self.pkg_methods.contains_key(&key) && self.pkg_methods.len() < MAX_PKG_METHOD_INDICES {
            let built = PkgMethodIndex::build(ws, decl_file, self.view);
            self.pkg_methods.insert(key, built);
        }
        self.pkg_methods.get(&key).and_then(Option::as_ref)
    }

    pub(crate) fn method_header(
        &mut self,
        ws: &'a Workspace,
        declaration: DeclId<'a>,
    ) -> Option<Arc<MethodHeader>> {
        if let Some(header) = self.method_headers.entries.get(&declaration) {
            return Some(Arc::clone(header));
        }
        let method = declaration.symbol;
        let span = method.declaration()?;
        let end = method.body().map_or(span.end, |body| body.start);
        let text = ws.source_slice(declaration.file, span.start..end)?;
        let parsed = crate::syntax::parse_signature(&text);
        if !parsed.diagnostics.is_empty() || !parsed.type_parameters.is_empty() {
            return None;
        }
        let receiver_names =
            locals::receiver_parameters(&text, &parsed.receiver.first()?.annotation);
        let header = Arc::new(MethodHeader {
            start: span.start,
            text,
            parsed,
            receiver_names,
        });
        if self.method_headers.entries.len() < MAX_METHOD_HEADERS
            && self.method_headers.bytes + header.text.len() <= MAX_METHOD_HEADER_BYTES
        {
            self.method_headers.bytes += header.text.len();
            self.method_headers
                .entries
                .insert(declaration, Arc::clone(&header));
        }
        Some(header)
    }

    fn can_insert(&self, extra_bytes: usize) -> bool {
        self.entries < MAX_DECL_CACHE
            && self.bytes.saturating_add(extra_bytes) <= MAX_ENV_CACHE_BYTES
    }

    fn record_insert(&mut self, extra_bytes: usize) {
        self.entries += 1;
        self.bytes = self.bytes.saturating_add(extra_bytes);
    }

    pub(crate) fn find_import_dir(
        &mut self,
        ws: &'a Workspace,
        file: &'a Path,
        alias: &str,
    ) -> Option<&'a Path> {
        let file = ws.files.get_key_value(file)?.0.as_path();
        if let Some(&cached) = self.files.get(file).and_then(|e| e.imports.get(alias)) {
            return cached;
        }
        let target_dir = Self::compute_import_dir(ws, self.view, file, alias);
        let limit_ok = if target_dir.is_some() {
            self.can_insert(alias.len())
        } else {
            self.entries < MAX_DECL_CACHE / 2
                && self.bytes.saturating_add(alias.len()) <= MAX_ENV_CACHE_BYTES
        };
        if limit_ok {
            self.files
                .entry(file)
                .or_default()
                .imports
                .insert(alias.into(), target_dir);
            self.record_insert(alias.len());
        }
        target_dir
    }

    fn compute_import_dir(
        ws: &'a Workspace,
        view: BuildView<'a>,
        file: &'a Path,
        alias: &str,
    ) -> Option<&'a Path> {
        let resolver = View::new(ws, view);
        let imp = ws.files.get(file)?.imports.iter().find(|imp| {
            !imp.is_dot() && !imp.is_blank() && resolver.import_name(file, imp) == Some(alias)
        })?;
        if let Some((p, _)) = resolver.imported_symbol_iter(file, alias).next() {
            return p.parent();
        }
        let import_path = imp.path.as_str();
        for (mod_root, mod_path) in &ws.modules {
            let suffix = if mod_path == "std" {
                import_path
            } else if import_path == mod_path {
                ""
            } else if let Some(suffix) = import_path
                .strip_prefix(mod_path.as_str())
                .and_then(|suffix| suffix.strip_prefix('/'))
            {
                suffix
            } else {
                continue;
            };
            let candidate = mod_root.join(suffix);
            if ws.directory_import_path(&candidate).as_deref() != Some(import_path) {
                continue;
            }
            if let Some((file_path, _)) = ws
                .files
                .range::<Path, _>((Bound::Included(candidate.as_path()), Bound::Unbounded))
                .take_while(|(path, _)| path.starts_with(&candidate))
                .find(|(path, _)| {
                    path.parent() == Some(candidate.as_path()) && view.visible(ws, path)
                })
            {
                return file_path.parent();
            }
        }
        None
    }

    pub(crate) fn resolve_unqualified(
        &mut self,
        ws: &'a Workspace,
        file: &'a Path,
        name: &str,
    ) -> UnqualifiedLookup<'a> {
        let Some(file) = ws.files.get_key_value(file).map(|(p, _)| p.as_path()) else {
            return UnqualifiedLookup::NotFound;
        };
        if let Some(&cached) = self.files.get(file).and_then(|e| e.unqualified.get(name)) {
            return cached;
        }
        let result = View::new(ws, self.view).unqualified_lookup(file, name);
        if self.can_insert(name.len()) {
            self.files
                .entry(file)
                .or_default()
                .unqualified
                .insert(name.into(), result);
            self.record_insert(name.len());
        }
        result
    }

    pub(crate) fn resolve_imported(
        &mut self,
        ws: &'a Workspace,
        file: &'a Path,
        alias: &str,
        name: &str,
    ) -> Option<DeclId<'a>> {
        let file = ws.files.get_key_value(file)?.0.as_path();
        if let Some(&cached) = self
            .files
            .get(file)
            .and_then(|e| e.imported_decls.get(alias))
            .and_then(|m| m.get(name))
        {
            return cached;
        }
        let resolved = Self::resolve_imported_uncached(ws, self.view, file, alias, name);
        let extra_bytes = alias.len().saturating_add(name.len());
        if self.can_insert(extra_bytes) {
            let entry = self.files.entry(file).or_default();
            let members = entry.imported_decls.entry(alias.into()).or_default();
            if !members.contains_key(name) {
                members.insert(name.into(), resolved);
                self.record_insert(extra_bytes);
            }
        }
        resolved
    }

    fn resolve_imported_uncached(
        ws: &'a Workspace,
        view: BuildView<'a>,
        file: &'a Path,
        alias: &str,
        name: &str,
    ) -> Option<DeclId<'a>> {
        let probe = crate::bloom::Fingerprint::new(name.as_bytes());
        let mut candidates = View::new(ws, view)
            .imported_symbol_candidates(file, alias, Some(probe))
            .filter(|(_, symbol)| symbol.name == name);
        let (file, symbol) = candidates.next()?;
        candidates
            .next()
            .is_none()
            .then_some(DeclId { file, symbol })
    }

    pub(crate) fn resolve_type_name(
        &mut self,
        ws: &'a Workspace,
        origin: &Path,
        name: &str,
    ) -> TypeNameLookup<'a> {
        if let Some(&found) = self
            .files
            .get(origin)
            .and_then(|entry| entry.type_names.get(name))
        {
            return found;
        }
        let file = ws.files.get_key_value(origin)?.0.as_path();
        let resolver = View::new(ws, self.view);
        let found = if name.contains('.') {
            resolver
                .resolve_type_declaration(file, name)
                .map(|(f, s)| (f.as_path(), s))
        } else if resolver.imported_name(file, name) {
            if self.can_insert(name.len()) {
                self.files
                    .entry(file)
                    .or_default()
                    .type_names
                    .insert(name.into(), None);
                self.record_insert(name.len());
            }
            return None;
        } else {
            match self.resolve_unqualified(ws, file, name) {
                UnqualifiedLookup::Found(f, symbol) => Some((f.as_path(), symbol)),
                UnqualifiedLookup::Ambiguous | UnqualifiedLookup::NotFound => None,
            }
        };
        let found = match found {
            Some((f, symbol)) if symbol.is_type() => Some(Some(DeclId { file: f, symbol })),
            Some(_) => None,
            None => Some(None),
        };
        if self.can_insert(name.len()) {
            self.files
                .entry(file)
                .or_default()
                .type_names
                .insert(name.into(), found);
            self.record_insert(name.len());
        }
        found
    }
}
