//! Workspace metadata and the single owner of document/module mutations.
mod configuration;
mod dependencies;
mod discovery;
mod events;
mod goenv;
mod graph;
mod location;
mod modules;
mod nested;
mod packages;
mod poll;
mod sdk;
mod source;
mod state;
pub(crate) use events::WorkspaceEvent;
pub(crate) use location::{path, uri};
pub(crate) use packages::production_file;
pub(crate) use state::Workspace;
