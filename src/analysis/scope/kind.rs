//! Semantic object categories, independent of negotiated protocol legends.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ObjectKind {
    Type,
    Variable,
    Parameter,
    TypeParameter,
}
impl ObjectKind {
    pub(crate) fn is_type(self) -> bool {
        matches!(self, Self::Type | Self::TypeParameter)
    }
}
