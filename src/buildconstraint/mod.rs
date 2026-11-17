//! Go build constraints, independent of source parsing and workspace state.
mod experiments;
mod expression;
mod header;
mod target;
#[cfg(test)]
mod tests;

pub use target::Target;
pub(crate) use target::matching_targets;

pub(crate) fn minimum_go_version(source: &str) -> Option<u16> {
    header::from_header(source).ok()??.minimum_go_version()
}
