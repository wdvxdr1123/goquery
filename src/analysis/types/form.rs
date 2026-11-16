//! Bounded outer-form projection from native type identities; no wire categories.
use super::lookup::TypeTree;
use super::{BasicKind, MAX_BYTES, MAX_DEPTH, Ty, TypeArena, TypeId, unparenthesized_type};
use crate::analysis::scope as locals;
use crate::syntax::{Lexer, TokenKind};
use crate::workspace::Workspace;
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
    workspace: &Workspace,
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
        let mut arena = TypeArena::new(workspace);
        if let Some(input) = arena.parse(file, text)
            && let Some(form) = arena.form(input)
        {
            return Some(form);
        }
        if arena.has_type_error() {
            return None;
        }
    }
    let mut tree = TypeTree::new(workspace);
    let file = workspace.files.get_key_value(file)?.0;
    let input = tree.push(file, text, None)?;
    let form = tree.form(input, 0)?;
    if tree
        .argument_roots
        .iter()
        .any(|(origin, root)| *origin == file && locals.resolve_at(offset, root).is_some())
    {
        return None;
    }
    Some(form)
}

pub(crate) fn declared_type_presentation(
    workspace: &Workspace,
    file: &Path,
    origin: &Path,
    declaration: &str,
) -> Option<TypePresentation> {
    if declaration.len() > MAX_BYTES {
        return None;
    }
    let origin = workspace.files.get_key_value(origin)?.0;
    TypeTree::new(workspace).declaration_presentation(origin, declaration, file)
}

pub(crate) fn local_type_presentation(
    workspace: &Workspace,
    file: &Path,
    source: &str,
    locals: &locals::Locals,
    binding: &locals::Binding,
) -> Option<TypePresentation> {
    if binding.declaration == locals::DeclarationKind::TypeParameter {
        return Some(TypePresentation {
            form: TypeForm::Interface,
            text: "interface{...}".to_owned(),
        });
    }
    if binding.declaration != locals::DeclarationKind::Type {
        return None;
    }
    let mut span = binding.annotation()?.clone();
    for _ in 0..MAX_DEPTH {
        let annotation = source.get(span.clone())?;
        if annotation.len() > MAX_BYTES {
            return None;
        }
        let parsed = crate::syntax::parse_type(annotation);
        if !parsed.diagnostics.is_empty() {
            return None;
        }
        let root = unparenthesized_type(annotation)?;
        if Lexer::new(root).next()?.kind == TokenKind::Ident
            && let Some(name) = locals::named_type(root)
            && let Some(local) =
                locals.resolve_at(span.start, locals::named_root(&name).split('.').next()?)
        {
            if local.declaration != locals::DeclarationKind::Type {
                return None;
            }
            span = local.annotation()?.clone();
            continue;
        }
        let names: Vec<_> = locals
            .visible_bindings(span.start)
            .filter(|binding| {
                binding.object_kind.is_type()
                    && locals
                        .resolve_at(span.start, &binding.name)
                        .is_some_and(|actual| actual.selection == binding.selection)
            })
            .map(|binding| binding.name.clone())
            .collect();
        let file = workspace.files.get_key_value(file)?.0;
        return TypeTree::new(workspace).local_presentation(file, annotation, &names);
    }
    None
}
