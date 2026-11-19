//! Protocol lifecycle, capabilities and routing; semantic algorithms live below this boundary.
mod backend;
mod capabilities;
mod scheduling;
mod session;
mod transport;
mod worker;
pub(crate) use session::serve;
