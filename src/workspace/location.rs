//! Shared file URI conversion at protocol result boundaries.
use serde_json::Value;
use std::path::{Path, PathBuf};
use url::Url;

pub(crate) fn path(uri: &Value) -> Option<PathBuf> {
    Url::parse(uri.as_str()?).ok()?.to_file_path().ok()
}
pub(crate) fn uri(path: &Path) -> String {
    Url::from_file_path(path)
        .expect("absolute file path")
        .to_string()
}
