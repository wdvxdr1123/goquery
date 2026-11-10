//! Bounded source access; only explicitly open documents retain text.
use super::Workspace;
use crate::index;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

impl Workspace {
    pub(crate) fn open_source(&self, file: &Path) -> Option<&str> {
        self.open.get(file).map(String::as_str)
    }

    pub(crate) fn source(&self, file: &Path) -> Option<String> {
        if let Some(source) = self.open.get(file) {
            return Some(source.clone());
        }
        Self::read_source(file)
    }

    /// Read a bounded declaration slice, respecting unsaved buffers. Closed files
    /// use one temporary handle and retain neither source nor handle afterward.
    pub(crate) fn source_slice(&self, file: &Path, span: std::ops::Range<usize>) -> Option<String> {
        let length = span.end.checked_sub(span.start)?;
        if length > 16 * 1024 {
            return None;
        }
        if let Some(source) = self.open.get(file) {
            return source.get(span).map(str::to_owned);
        }
        let mut handle = fs::File::open(file).ok()?;
        let size = handle.metadata().ok()?.len();
        if size > index::MAX_FILE_BYTES || span.end as u64 > size {
            return None;
        }
        handle.seek(SeekFrom::Start(span.start as u64)).ok()?;
        let mut bytes = vec![0; length];
        handle.read_exact(&mut bytes).ok()?;
        String::from_utf8(bytes).ok()
    }

    pub(super) fn read_source(file: &Path) -> Option<String> {
        let handle = fs::File::open(file).ok()?;
        let length = handle.metadata().ok()?.len();
        if length > index::MAX_FILE_BYTES {
            return None;
        }
        let mut source = String::with_capacity(length as usize);
        // Keep one handle and cap growth between metadata and the actual read.
        handle
            .take(index::MAX_FILE_BYTES + 1)
            .read_to_string(&mut source)
            .ok()?;
        (source.len() as u64 <= index::MAX_FILE_BYTES).then_some(source)
    }
}
