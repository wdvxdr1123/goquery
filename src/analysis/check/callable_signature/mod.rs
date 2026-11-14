//! Query-local callable presentation during migration to native signature operands.
mod methods;
pub(crate) mod presentation;
mod render;
mod values;
pub(crate) use methods::{PreparedMethods, SignatureSource};
pub(crate) use render::signature_label;

pub(crate) struct ParameterLabel {
    pub(crate) label: String,
}
pub(crate) struct CallableSignature {
    pub(crate) label: String,
    pub(crate) parameters: Vec<ParameterLabel>,
    pub(crate) active_parameter: Option<u32>,
}
