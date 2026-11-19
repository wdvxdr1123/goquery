//! JSON-RPC and LSP error codes shared by workspace and query layers.
use std::sync::atomic::{AtomicBool, Ordering};

pub(crate) const INVALID_PARAMS: i32 = -32602;
pub(crate) const REQUEST_FAILED: i32 = -32803;

pub(crate) fn check_cancelled(flag: Option<&AtomicBool>) -> Result<(), (i32, String)> {
    if flag.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
        return Err((-32800, "Canceled".into()));
    }
    Ok(())
}
