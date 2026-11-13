//! Standard-library "Added in go1.x" lookup from the SDK's api manifest files.
//!
//! gopls reads `golang.org/x/tools/internal/stdlib`; the same data is available
//! as `<GOROOT>/api/go1.*.txt`. API files are append-only per release, so the
//! first file that mentions a symbol is its introduction version. We binary
//! search that monotone predicate, bounding each read and the total bytes read
//! for one request; nothing is retained between requests.

use crate::analysis::query::Context;
use crate::index::{IndexedSymbol as Symbol, SymbolKind};
use std::path::Path;

const MAX_FILE_BYTES: u64 = 3 * 1024 * 1024;
const MAX_TOTAL_BYTES: u64 = 12 * 1024 * 1024;

/// The `go1.x` release that introduced a standard-library symbol.
pub(super) fn added(context: &Context<'_>, file: &Path, symbol: &Symbol) -> Option<String> {
    if !crate::syntax::is_exported_name(&symbol.name) {
        return None;
    }
    let workspace = context.workspace();
    let import_path = workspace.import_path(file)?;
    let releases = api_releases(context, file)?;
    let pattern = Pattern {
        path: &import_path,
        kind: PatternKind::of(symbol.kind),
        receiver: symbol.receiver.as_deref(),
        name: &symbol.name,
    };
    scan(&releases, &pattern)?.and_then(|minor| (minor > 0).then(|| format!("go1.{minor}")))
}

/// Release that introduced an exported struct field of an std type.
pub(super) fn added_field(
    context: &Context<'_>,
    file: &Path,
    owner: &str,
    name: &str,
) -> Option<String> {
    if !crate::syntax::is_exported_name(owner) || !crate::syntax::is_exported_name(name) {
        return None;
    }
    let workspace = context.workspace();
    let import_path = workspace.import_path(file)?;
    let releases = api_releases(context, file)?;
    let pattern = Pattern {
        path: &import_path,
        kind: PatternKind::Field,
        receiver: Some(owner),
        name,
    };
    scan(&releases, &pattern)?.and_then(|minor| (minor > 0).then(|| format!("go1.{minor}")))
}

/// Release that introduced a method of an std type (declared or interface).
pub(super) fn added_method(
    context: &Context<'_>,
    file: &Path,
    owner: &str,
    name: &str,
) -> Option<String> {
    if !crate::syntax::is_exported_name(owner) || !crate::syntax::is_exported_name(name) {
        return None;
    }
    let workspace = context.workspace();
    let import_path = workspace.import_path(file)?;
    let releases = api_releases(context, file)?;
    let pattern = Pattern {
        path: &import_path,
        kind: PatternKind::Method,
        receiver: Some(owner),
        name,
    };
    scan(&releases, &pattern)?.and_then(|minor| (minor > 0).then(|| format!("go1.{minor}")))
}

/// Sorted `(minor, path)` api files for a standard-library declaration file.
fn api_releases(context: &Context<'_>, file: &Path) -> Option<Vec<(u64, std::path::PathBuf)>> {
    let workspace = context.workspace();
    let root = workspace
        .modules
        .iter()
        .find(|(root, module)| module.as_str() == "std" && file.starts_with(root))
        .map(|(root, _)| root)?;
    let api = root.parent()?.join("api");
    if !api.is_dir() {
        return None;
    }
    let mut releases: Vec<(u64, std::path::PathBuf)> = std::fs::read_dir(&api)
        .ok()?
        .take(128)
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_str()?.to_owned();
            // go1.txt is the go1.0 baseline; go1.N.txt lists release additions.
            let minor = if name == "go1.txt" {
                0
            } else {
                name.strip_prefix("go1.")?
                    .strip_suffix(".txt")?
                    .parse()
                    .ok()?
            };
            Some((minor, entry.path()))
        })
        .collect();
    if releases.is_empty() {
        return None;
    }
    releases.sort_unstable();
    Some(releases)
}

/// First release whose api file declares the pattern.
fn scan(releases: &[(u64, std::path::PathBuf)], pattern: &Pattern<'_>) -> Option<Option<u64>> {
    let mut total = 0u64;
    // API files list additions per release, not cumulative entries, so scan
    // ascending and keep the first declaration.
    for (minor, path) in releases {
        match contains(path, pattern, &mut total) {
            Some(true) => return Some(Some(*minor)),
            Some(false) => {}
            None => return None,
        }
    }
    Some(None)
}

struct Pattern<'a> {
    path: &'a str,
    kind: PatternKind,
    receiver: Option<&'a str>,
    name: &'a str,
}

/// Declaration shapes declared by Go API manifest lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PatternKind {
    Type,
    Constant,
    Variable,
    Function,
    Method,
    Field,
}

impl PatternKind {
    fn of(kind: SymbolKind) -> Self {
        match kind {
            SymbolKind::Type => Self::Type,
            SymbolKind::Constant => Self::Constant,
            SymbolKind::Variable => Self::Variable,
            SymbolKind::Function => Self::Function,
            SymbolKind::Method => Self::Method,
        }
    }
}

/// Whether one api file declares the symbol; `None` if the read budget failed.
fn contains(path: &Path, pattern: &Pattern<'_>, total: &mut u64) -> Option<bool> {
    let metadata = std::fs::metadata(path).ok()?;
    if metadata.len() > MAX_FILE_BYTES || *total > MAX_TOTAL_BYTES {
        return None;
    }
    let source = std::fs::read_to_string(path).ok()?;
    *total = total.saturating_add(source.len() as u64);
    for line in source.lines() {
        let Some(rest) = line.strip_prefix("pkg ") else {
            continue;
        };
        let Some((package, declaration)) = rest.split_once(", ") else {
            continue;
        };
        let package = package.split(" (").next().unwrap_or(package);
        if package != pattern.path {
            continue;
        }
        if declares(declaration, pattern) {
            return Some(true);
        }
    }
    Some(false)
}

fn declares(declaration: &str, pattern: &Pattern<'_>) -> bool {
    match pattern.kind {
        PatternKind::Function => {
            let Some(rest) = declaration.strip_prefix("func ") else {
                return false;
            };
            symbol_name(rest, '(', pattern.name)
        }
        PatternKind::Method => {
            let Some(rest) = declaration.strip_prefix("method (") else {
                return interface_method(declaration, pattern);
            };
            let Some(receiver) = pattern.receiver else {
                return false;
            };
            let Some((written, rest)) = rest.split_once(')') else {
                return false;
            };
            let rest = rest.trim_start();
            if receiver_name(written) == receiver_name(receiver)
                && rest.starts_with(pattern.name)
                && rest
                    .get(pattern.name.len()..)
                    .is_some_and(|after| after.starts_with(['(', '[']))
            {
                return true;
            }
            interface_method(declaration, pattern)
        }
        PatternKind::Field => declaration.strip_prefix("type ").is_some_and(|rest| {
            let Some((owner, rest)) = rest.split_once(' ') else {
                return false;
            };
            let Some(receiver) = pattern.receiver else {
                return false;
            };
            owner == receiver
                && rest
                    .trim_start()
                    .strip_prefix("struct, ")
                    .is_some_and(|fields| symbol_name(fields, ' ', pattern.name))
        }),
        PatternKind::Constant => declaration.strip_prefix("const ").is_some_and(|rest| {
            symbol_name(rest, ' ', pattern.name) || symbol_name(rest, '=', pattern.name)
        }),
        PatternKind::Variable => declaration
            .strip_prefix("var ")
            .is_some_and(|rest| symbol_name(rest, ' ', pattern.name)),
        PatternKind::Type => declaration.strip_prefix("type ").is_some_and(|rest| {
            symbol_name(rest, ' ', pattern.name) || symbol_name(rest, '[', pattern.name)
        }),
    }
}

/// Interface methods may be individual API records or a type method-name list.
fn interface_method(declaration: &str, pattern: &Pattern<'_>) -> bool {
    let Some(rest) = declaration.strip_prefix("type ") else {
        return false;
    };
    let Some((owner, rest)) = rest.split_once(' ') else {
        return false;
    };
    let Some(receiver) = pattern.receiver else {
        return false;
    };
    if owner != receiver || !rest.trim_start().starts_with("interface") {
        return false;
    }
    if let Some(method) = rest.trim_start().strip_prefix("interface, ") {
        return symbol_name(method, '(', pattern.name);
    }
    let Some(open) = declaration.find('{') else {
        return false;
    };
    let Some(close) = declaration.rfind('}') else {
        return false;
    };
    let Some(inner) = declaration.get(open + 1..close) else {
        return false;
    };
    inner
        .split(|ch: char| !ch.is_alphanumeric() && ch != '_')
        .any(|word| word == pattern.name)
}

/// Match a leading identifier followed by `delimiter`.
fn symbol_name(rest: &str, delimiter: char, name: &str) -> bool {
    let trimmed = rest.trim_start();
    let Some(after) = trimmed.strip_prefix(name) else {
        return false;
    };
    after.starts_with(delimiter) || after.starts_with('[')
}

fn receiver_name(written: &str) -> &str {
    let base = written.trim().trim_start_matches('*');
    base.split(['[', ' ']).next().unwrap_or(base)
}
