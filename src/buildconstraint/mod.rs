//! Go build constraints, independent of source parsing and workspace state.
mod experiments;
mod expression;
mod header;
mod target;
#[cfg(test)]
mod tests;

pub use target::Target;
