//! Orphan-view variants: declarations in build-tag sibling files that are not
//! part of the host package view but belong to a detached document's own build
//! context. Lookups are bounded, read on demand and retain no state.
use crate::workspace::Workspace;
use std::path::{Path, PathBuf};

const MAX_FILES: usize = 512;
const MAX_BYTES: usize = 32 * 1024 * 1024;

/// Byte location and position of a declaration in a variant file.
pub(super) struct Location {
    pub(super) path: PathBuf,
    pub(super) offset: usize,
    pub(super) line: u32,
    pub(super) character: u32,
}

/// Find a package-level declaration in the first target-compatible file.
pub(super) fn package_symbol(
    workspace: &Workspace,
    target: Option<&crate::buildconstraint::Target>,
    directory: &Path,
    package: &str,
    name: &str,
    include_tests: bool,
) -> Option<Location> {
    let max_files = MAX_FILES;
    let mut files: Vec<PathBuf> = std::fs::read_dir(directory)
        .ok()?
        .take(max_files * 4)
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension().is_some_and(|extension| extension == "go")
                && (include_tests
                    || path
                        .file_name()
                        .is_some_and(|file| !file.to_string_lossy().ends_with("_test.go")))
        })
        .collect();
    files.sort();
    files.truncate(max_files);
    let mut total = 0usize;
    for path in files {
        let source = match workspace.source(&path) {
            Some(source) => source,
            None => {
                let length = std::fs::metadata(&path).ok()?.len() as usize;
                if length > MAX_BYTES {
                    continue;
                }
                let source = std::fs::read_to_string(&path).ok()?;
                total = total.saturating_add(source.len());
                if total > MAX_BYTES {
                    break;
                }
                source
            }
        };
        let file_name = path.file_name()?.to_str()?;
        let matched =
            target.is_none_or(|target| target.matches_file(file_name, &source).ok() == Some(true));
        if !matched {
            continue;
        }
        let stub = crate::syntax::stub(&source);
        if stub.package != package {
            continue;
        }
        let Some(symbol) = stub
            .symbols
            .iter()
            .find(|symbol| symbol.name == name && symbol.receiver.is_none())
        else {
            continue;
        };
        let position = crate::text::SourceMap::new(&source).position(symbol.selection.start);
        return Some(Location {
            path,
            offset: symbol.selection.start,
            line: position.line,
            character: position.character,
        });
    }
    None
}

/// On-disk directory of an import path, mirroring the resolver's module map.
pub(super) fn import_directory(workspace: &Workspace, import_path: &str) -> Option<PathBuf> {
    let mut directories = Vec::new();
    for (root, module) in &workspace.modules {
        let suffix = if module == "std" {
            Some(import_path.to_owned())
        } else if import_path == module {
            Some(String::new())
        } else {
            import_path
                .strip_prefix(module.as_str())
                .and_then(|rest| rest.strip_prefix('/'))
                .map(str::to_owned)
        };
        if let Some(suffix) = suffix {
            directories.push(root.join(suffix));
        }
    }
    directories.sort();
    directories.dedup();
    directories.into_iter().find(|directory| directory.is_dir())
}
