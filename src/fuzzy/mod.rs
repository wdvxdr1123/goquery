//! gopls-compatible symbol and completion matching.
//!
//! The linear symbol matcher uses at most 255 Unicode scalar values; the
//! completion scorer uses byte-based dynamic programming with separate limits.
//! Both are derived from gopls v0.24.0 under the Go Authors' BSD-3-Clause
//! license in [`LICENSE-GO`](../../LICENSE-GO); they have no workspace, LSP or
//! I/O dependencies.

mod completion;
mod symbol;

pub use completion::CompletionMatcher;
pub use symbol::{Match, Matcher};
