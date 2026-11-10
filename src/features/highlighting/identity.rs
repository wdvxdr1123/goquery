//! Identifier styles depend on resolved identities, not spelling alone.
use super::config::SemanticType;
use super::config::{DEFAULT_LIBRARY, DEFINITION, READONLY, SHADOWING, SIGNATURE, STATIC};
use crate::analysis::check::classify;
use crate::analysis::scope::universe::{builtin, builtin_function, builtin_type};
use crate::analysis::scope::{DeclarationKind, Identity, ObjectKind, Resolver};
use crate::analysis::source::{Name, Role, SourceAnalysis};
use std::path::Path;

pub(super) fn style(
    analysis: &SourceAnalysis,
    resolver: &Resolver<'_>,
    file: &Path,
    source: &str,
    name: &Name,
    identity: &Option<Identity<'_>>,
    builtin_file: Option<&Path>,
) -> (SemanticType, u64) {
    let mut kind = match classify::classify(analysis, resolver, source, name, identity) {
        ObjectKind::Namespace => SemanticType::Namespace,
        ObjectKind::Type => SemanticType::Type,
        ObjectKind::Function => SemanticType::Function,
        ObjectKind::Method => SemanticType::Method,
        ObjectKind::Variable => SemanticType::Variable,
        ObjectKind::Property => SemanticType::Property,
        ObjectKind::Parameter => SemanticType::Parameter,
        ObjectKind::TypeParameter => SemanticType::TypeParameter,
    };
    let spelling = &source[name.span.clone()];
    let mut modifiers = if matches!(name.role, Role::EmbeddedType(_) | Role::FieldDeclaration(_)) {
        DEFINITION
    } else {
        0
    };
    // Universe interface methods (`error.Error`) have no package symbol but may
    // be reached through an inferred binding whose type is the universe error.
    if identity.is_none() && kind == SemanticType::Method {
        let operand = match &name.role {
            Role::Member(Some(span)) | Role::ExpressionMember(span) => Some(span.clone()),
            _ => None,
        };
        if let Some(operand) = operand
            && let Some(receiver) = source.get(operand.clone())
            && let Some(binding) = analysis.locals.definition_at(&operand, receiver)
        {
            let annotated = binding
                .annotation()
                .and_then(|span| source.get(span.clone()))
                .is_some_and(|text| text == "error");
            let inferred = resolver
                .binding_type_cached(file, source, &analysis.locals, binding)
                .as_deref()
                .is_some_and(|text| text == "error");
            if annotated || inferred {
                modifiers |= DEFAULT_LIBRARY;
            }
        }
    }
    match identity {
        Some(Identity::Local(selection)) => {
            if *selection == name.span {
                modifiers |= DEFINITION;
            }
            if let Some(binding) = analysis
                .locals
                .definition_at(selection, spelling)
                .filter(|binding| binding.selection == *selection)
            {
                if *selection == name.span
                    && (analysis.locals.has_outer_binding(binding)
                        || resolver.has_outer_name(spelling)
                        || builtin(spelling))
                {
                    modifiers |= SHADOWING;
                }
                match binding.declaration {
                    DeclarationKind::TypeParameter => kind = SemanticType::TypeParameter,
                    DeclarationKind::Constant => modifiers |= READONLY,
                    _ => {}
                }
                if binding.declaration != DeclarationKind::TypeParameter {
                    let annotation = binding.annotation().cloned().or_else(|| {
                        let (span, result, count) = binding.initializer()?;
                        (result == 0 && count == 1).then(|| span.clone())
                    });
                    if annotation
                        .and_then(|span| source.get(span))
                        .and_then(|text| crate::syntax::Lexer::new(text).next())
                        .is_some_and(|token| token.kind == crate::syntax::TokenKind::Func)
                    {
                        modifiers |= SIGNATURE;
                    }
                }
            }
        }
        Some(Identity::Global(path, symbol)) => {
            if *path == file && symbol.start == name.span.start && symbol.end == name.span.end {
                modifiers |= DEFINITION;
                if !symbol.is_method() && builtin(spelling) {
                    modifiers |= SHADOWING;
                }
            }
            modifiers |= if symbol.is_const() {
                READONLY
            } else if symbol.is_var() {
                STATIC
            } else if symbol.is_callable() {
                SIGNATURE
            } else {
                0
            };
        }
        Some(Identity::Member(member)) => {
            if member.file == file && matches!(name.role, Role::FieldDeclaration(_)) {
                modifiers |= DEFINITION;
            }
            // Universe members (e.g. error.Error) are declared in the SDK's
            // builtin.go; gopls marks them defaultLibrary.
            if builtin_file == Some(member.file) || member.owner.name == "error" {
                modifiers |= DEFAULT_LIBRARY;
            }
            if member.kind == crate::syntax::MemberKind::Method {
                modifiers |= SIGNATURE;
            }
        }
        None if matches!(
            name.role,
            Role::Parameter | Role::Result | Role::BlankDeclaration { .. }
        ) =>
        {
            modifiers |= DEFINITION;
            if matches!(name.role, Role::BlankDeclaration { constant: true, .. }) {
                modifiers |= READONLY;
            }
        }
        None if matches!(name.role, Role::Name | Role::Type(None)) => {
            if spelling == "_" && analysis.locals.range_blank_at(&name.span).is_some() {
                modifiers |= DEFINITION;
            }
            // The resolver has already ruled out local, package and imported
            // objects. A qualified field named len/true is never a builtin.
            if kind == SemanticType::Type && builtin_type(spelling)
                || kind == SemanticType::Function && builtin_function(spelling)
            {
                modifiers |= DEFAULT_LIBRARY;
            } else if matches!(spelling, "true" | "false" | "nil" | "iota")
                && !resolver.imported_name(spelling)
            {
                modifiers |= DEFAULT_LIBRARY | READONLY;
            }
        }
        _ => {}
    }
    (kind, modifiers)
}
