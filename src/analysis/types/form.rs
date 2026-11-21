//! Bounded outer-form projection from native type identities; no wire categories.
use super::{BasicKind, MAX_BYTES, MAX_DEPTH, Ty, TypeArena, TypeId, unparenthesized_type};
use crate::analysis::engine::scope as locals;
use crate::syntax::{Lexer, TokenKind};
use std::path::Path;

/// The outer constructor only, never a proof of identity or assignability.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    /// Follow the native declaration chain without expanding a large aggregate
    /// just to display its outer constructor. The returned source node is only
    /// used to attach names to an already checked function signature.
    pub(crate) fn presentation_type(
        &mut self,
        mut input: TypeId,
    ) -> Option<(TypeForm, TypeId, TypeId)> {
        let mut source = input;
        for _ in 0..MAX_DEPTH {
            self.step()?;
            if let Some((declaration, _, _)) = self.named_info(input) {
                self.check_named_arguments(input)?;
                if let Some(info) = declaration
                    .symbol
                    .type_info
                    .as_ref()
                    .filter(|info| info.target.is_none())
                {
                    return Some((
                        if info.is_interface() {
                            TypeForm::Interface
                        } else {
                            TypeForm::Struct
                        },
                        input,
                        source,
                    ));
                }
                source = input;
                input = self.expand(input)?;
            } else if let Some(origin_id) = self.local_origin_id(input) {
                let data = self.origin_data(origin_id)?;
                if data.state != super::ty::NamedState::Loaded {
                    return None;
                }
                source = input;
                input = data.rhs_type()?;
            } else {
                return Some((self.form(input)?, input, source));
            }
        }
        None
    }

    pub(crate) fn is_untyped_bool_or_string(&self, input: TypeId) -> bool {
        let Some(ty) = self.node(input) else {
            return false;
        };
        matches!(
            ty,
            Ty::Basic(BasicKind::UntypedBool | BasicKind::UntypedString)
        )
    }

    pub(crate) fn is_type_param(&mut self, input: TypeId) -> bool {
        let Some(unaliased) = self.unalias(input) else {
            return false;
        };
        matches!(self.node(unaliased), Some(Ty::TypeParam(_)))
    }

    /// Aliases/definitions share native lazy expansion and its existing budgets.
    /// A parameter without a represented concrete underlying type stays unknown.
    pub(crate) fn form(&mut self, input: TypeId) -> Option<TypeForm> {
        let input = self.underlying(input)?;
        Some(match *self.node(input)? {
            Ty::Invalid => return None,
            Ty::Basic(kind) => return TypeForm::basic(kind),
            Ty::Interface(_) => TypeForm::Interface,
            Ty::Struct(_) => TypeForm::Struct,
            Ty::Signature(_) => TypeForm::Signature,
            Ty::Pointer(_) => TypeForm::Pointer,
            Ty::Array { .. } => TypeForm::Array,
            Ty::Slice(_) => TypeForm::Slice,
            Ty::Map { .. } => TypeForm::Map,
            Ty::Chan { .. } => TypeForm::Chan,
            Ty::TypeParam(_)
            | Ty::Named { .. }
            | Ty::Alias { .. }
            | Ty::Tuple(_)
            | Ty::Union(_) => {
                return None;
            }
        })
    }
}

pub(crate) fn type_form_in_scope(
    arena: &mut TypeArena<'_>,
    file: &Path,
    text: &str,
    is_shadowed: impl Fn(&str) -> bool,
) -> Option<TypeForm> {
    arena.with_projection(false, |arena| {
        type_form_inner(arena, file, text, &is_shadowed)
    })
}

fn type_form_inner(
    arena: &mut TypeArena<'_>,
    file: &Path,
    text: &str,
    is_shadowed: &impl Fn(&str) -> bool,
) -> Option<TypeForm> {
    if text.len() > MAX_BYTES {
        return None;
    }
    let root = unparenthesized_type(text)?;
    if Lexer::new(root).next()?.kind == TokenKind::LParen {
        return None;
    }
    if Lexer::new(root).next()?.kind == TokenKind::Ident
        && locals::named_type(root).is_some_and(|name| {
            is_shadowed(locals::named_root(&name).split('.').next().unwrap_or(""))
        })
    {
        return None;
    }
    let parsed = crate::syntax::parse_type(text);
    if !parsed.diagnostics.is_empty() {
        return None;
    }
    let shadowed = parsed.references.iter().any(|reference| {
        let span = reference.qualifier.as_ref().unwrap_or(&reference.name);
        is_shadowed(&text[span.clone()])
    });
    if !shadowed {
        if let Some(name) = locals::named_type(root).filter(|name| name == locals::named_root(name))
            && let Some((origin, symbol)) = arena.resolver().resolve_type_declaration(file, &name)
            && symbol.generic
        {
            return arena.with_projection(true, |arena| {
                let ty = arena.declaration_type(crate::analysis::engine::obj::DeclId {
                    file: origin,
                    symbol,
                })?;
                arena.presentation_type(ty).map(|(form, _, _)| form)
            });
        }
        match arena.parse_checked(file, text) {
            super::CheckOutcome::Ok(input) => {
                if let Some((form, _, _)) = arena.presentation_type(input) {
                    return Some(form);
                }
            }
            super::CheckOutcome::Invalid(()) => return None,
            super::CheckOutcome::Unknown(_) => {}
        }
    }
    // This is only an outer syntax category for presentation. Unavailable
    // nominal or local types never acquire an inferred underlying declaration.
    TypeForm::constructor(root)
}
