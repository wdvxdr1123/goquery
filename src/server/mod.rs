//! Protocol lifecycle, capabilities and routing; semantic algorithms live below this boundary.
mod backend;
mod capabilities;
mod session;
mod transport;
pub(crate) use session::serve;
