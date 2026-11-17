//! Bounded outer-form projection from native type identities; no wire categories.
use super::{BasicKind, MAX_BYTES, MAX_DEPTH, Ty, TypeArena, TypeId, unparenthesized_type};
use crate::analysis::scope as locals;
use crate::syntax::{Lexer, TokenKind};
use std::path::Path;

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
            match self.nodes.get(usize::from(input.0))?.clone() {
                Ty::Named { declaration, .. } | Ty::Alias { declaration, .. } => {
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
                }
                Ty::LocalDeclaration {
                    origin,
                    selection,
                    formal: false,
                } => {
                    let index = self.local_type_index(origin, &selection)?;
                    source = input;
                    input = match self.local_types[index].state {
                        super::declaration::LocalTypeState::Ready(ty) => ty,
                        _ => return None,
                    };
                }
                _ => return Some((self.form(input)?, input, source)),
            }
        }
        None
    }

    pub(crate) fn is_untyped_bool_or_string(&self, input: TypeId) -> bool {
        let Some(ty) = self.nodes.get(usize::from(input.0)) else {
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
        matches!(
            self.nodes.get(usize::from(unaliased.0)),
            Some(Ty::Parameter(_) | Ty::LocalDeclaration { formal: true, .. })
        )
    }

    /// Aliases/definitions share native lazy expansion and its existing budgets.
    /// A parameter without a represented concrete underlying type stays unknown.
    pub(crate) fn form(&mut self, input: TypeId) -> Option<TypeForm> {
        let input = self.underlying(input)?;
        Some(match self.nodes.get(usize::from(input.0))? {
            Ty::Invalid => return None,
            Ty::Basic(kind) => return TypeForm::basic(*kind),
            Ty::Interface { .. } => TypeForm::Interface,
            Ty::Struct { .. } => TypeForm::Struct,
            Ty::Signature { .. } => TypeForm::Signature,
            Ty::Pointer(_) => TypeForm::Pointer,
            Ty::Array { .. } => TypeForm::Array,
            Ty::Slice(_) => TypeForm::Slice,
            Ty::Map { .. } => TypeForm::Map,
            Ty::Chan { .. } => TypeForm::Chan,
            Ty::Parameter(_)
            | Ty::Named { .. }
            | Ty::Alias { .. }
            | Ty::Tuple(_)
            | Ty::Error(_)
            | Ty::LocalDeclaration { .. } => {
                return None;
            }
        })
    }
}

pub(crate) fn type_form_in_scope(
    arena: &mut TypeArena<'_>,
    file: &Path,
    text: &str,
    locals: &locals::Locals,
    offset: usize,
) -> Option<TypeForm> {
    arena.with_projection(false, |arena| {
        type_form_inner(arena, file, text, locals, offset)
    })
}

fn type_form_inner(
    arena: &mut TypeArena<'_>,
    file: &Path,
    text: &str,
    locals: &locals::Locals,
    offset: usize,
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
            locals
                .resolve_at(
                    offset,
                    locals::named_root(&name).split('.').next().unwrap_or(""),
                )
                .is_some()
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
        locals.resolve_at(offset, &text[span.clone()]).is_some()
    });
    if !shadowed {
        if let Some(name) = locals::named_type(root).filter(|name| name == locals::named_root(name))
            && let Some((origin, symbol)) = arena.resolver().resolve_type_declaration(file, &name)
            && symbol.generic
        {
            return arena.with_projection(true, |arena| {
                let ty = arena.declaration_type(crate::analysis::obj::DeclId {
                    file: origin,
                    symbol,
                })?;
                arena.presentation_type(ty).map(|(form, _, _)| form)
            });
        }
        if let Some(input) = arena.parse(file, text)
            && let Some((form, _, _)) = arena.presentation_type(input)
        {
            return Some(form);
        }
        if arena.has_type_error() {
            return None;
        }
    }
    // This is only an outer syntax category for presentation. Unavailable
    // nominal or local types never acquire an inferred underlying declaration.
    TypeForm::constructor(root)
}
