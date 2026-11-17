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
        self.source_with_limit(file, index::MAX_FILE_BYTES as usize)
    }

    pub(super) fn source_with_limit(&self, file: &Path, limit: usize) -> Option<String> {
        let limit = limit.min(index::MAX_FILE_BYTES as usize);
        if let Some(source) = self.open.get(file) {
            return (source.len() <= limit).then(|| source.clone());
        }
        Self::read_source_with_limit(file, limit as u64)
    }

    /// Read only a presentation header, trimming an incomplete trailing UTF-8
    /// character. Closed files still use one bounded temporary handle.
    pub(crate) fn source_prefix(&self, file: &Path, limit: usize) -> Option<String> {
        let limit = limit.min(index::MAX_FILE_BYTES as usize);
        if let Some(source) = self.open.get(file) {
            let mut end = source.len().min(limit);
            while !source.is_char_boundary(end) {
                end -= 1;
            }
            return Some(source[..end].to_owned());
        }
        let handle = fs::File::open(file).ok()?;
        let length = handle.metadata().ok()?.len();
        if length > index::MAX_FILE_BYTES {
            return None;
        }
        let mut bytes = Vec::with_capacity((length as usize).min(limit));
        handle.take(limit as u64).read_to_end(&mut bytes).ok()?;
        if let Err(error) = std::str::from_utf8(&bytes) {
            if error.error_len().is_some() {
                return None;
            }
            bytes.truncate(error.valid_up_to());
        }
        String::from_utf8(bytes).ok()
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
        Self::read_source_with_limit(file, index::MAX_FILE_BYTES)
    }

    fn read_source_with_limit(file: &Path, limit: u64) -> Option<String> {
        let handle = fs::File::open(file).ok()?;
        let length = handle.metadata().ok()?.len();
        if length > limit {
            return None;
        }
        let mut source = String::with_capacity(length as usize);
        // Keep one handle and cap growth between metadata and the actual read.
        handle.take(limit + 1).read_to_string(&mut source).ok()?;
        (source.len() as u64 <= limit).then_some(source)
    }
}
