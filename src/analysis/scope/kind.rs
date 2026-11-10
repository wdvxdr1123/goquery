//! Semantic object categories, independent of negotiated protocol legends.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ObjectKind {
    Namespace,
    Type,
    Function,
    Method,
    Variable,
    Property,
    Parameter,
    TypeParameter,
}
impl ObjectKind {
    pub(crate) fn is_type(self) -> bool {
        matches!(self, Self::Type | Self::TypeParameter)
    }

    /// A value object (not a namespace or type) whose type can own members.
    pub(crate) fn is_value(self) -> bool {
        matches!(
            self,
            Self::Variable | Self::Parameter | Self::Property | Self::Function | Self::Method
        )
    }
}
