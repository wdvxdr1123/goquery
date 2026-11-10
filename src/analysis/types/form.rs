//! Bounded outer-form projection from native type identities; no wire categories.
use super::{BasicKind, Ty, TypeArena, TypeId};
use crate::syntax::{Lexer, TokenKind};

/// The outer constructor only, never a proof of identity or assignability.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum TypeForm {
    Number,
    Bool,
    String,
    Interface,
    Struct,
    Signature,
    Pointer,
    Array,
    Slice,
    Map,
    Chan,
}
pub(crate) struct TypePresentation {
    pub form: TypeForm,
    pub text: String,
}
impl TypeForm {
    pub(crate) fn basic(kind: BasicKind) -> Option<Self> {
        if kind.boolean() {
            Some(Self::Bool)
        } else if kind.string() {
            Some(Self::String)
        } else if kind.numeric_rank().is_some() {
            Some(Self::Number)
        } else {
            None
        }
    }
    // Presentation fallback only: constructor syntax cannot prove a valid type.
    pub(crate) fn constructor(text: &str) -> Option<Self> {
        let mut tokens = Lexer::new(text);
        Some(match tokens.next()?.kind {
            TokenKind::Struct => Self::Struct,
            TokenKind::Interface => Self::Interface,
            TokenKind::Func => Self::Signature,
            TokenKind::Mul => Self::Pointer,
            TokenKind::Map => Self::Map,
            TokenKind::Chan | TokenKind::Arrow => Self::Chan,
            TokenKind::LBrack => {
                if tokens.next()?.kind == TokenKind::RBrack {
                    Self::Slice
                } else {
                    Self::Array
                }
            }
            _ => return None,
        })
    }
    pub(crate) fn builtin(name: &str) -> Option<Self> {
        if matches!(name, "any" | "error" | "comparable") {
            Some(Self::Interface)
        } else {
            Self::basic(BasicKind::named(name)?)
        }
    }
    #[cfg(test)]
    pub(crate) fn test_name(self) -> &'static str {
        match self {
            Self::Number => "number",
            Self::Bool => "bool",
            Self::String => "string",
            Self::Interface => "interface",
            Self::Struct => "struct",
            Self::Signature => "signature",
            Self::Pointer => "pointer",
            Self::Array => "array",
            Self::Slice => "slice",
            Self::Map => "map",
            Self::Chan => "chan",
        }
    }
}
impl TypeArena<'_> {
    /// Aliases/definitions share native lazy expansion and its existing budgets.
    /// A parameter without a represented concrete underlying type stays unknown.
    pub(crate) fn form(&mut self, input: TypeId) -> Option<TypeForm> {
        let input = self.underlying(input)?;
        Some(match self.nodes.get(usize::from(input.0))? {
            Ty::Basic(kind) => return TypeForm::basic(*kind),
            Ty::Interface { .. } => TypeForm::Interface,
            Ty::Struct { .. } => TypeForm::Struct,
            Ty::Signature { .. } => TypeForm::Signature,
            Ty::Pointer(_) => TypeForm::Pointer,
            Ty::Array { .. } => TypeForm::Array,
            Ty::Slice(_) => TypeForm::Slice,
            Ty::Map { .. } => TypeForm::Map,
            Ty::Chan { .. } => TypeForm::Chan,
            Ty::Parameter(_) | Ty::Named { .. } | Ty::Error(_) | Ty::LocalDeclaration { .. } => {
                return None;
            }
        })
    }
}
