//! Map supported semantic categories to the client's negotiated legend.
use serde_json::Value;
use tower_lsp::lsp_types::{SemanticTokenModifier, SemanticTokenType, SemanticTokensLegend};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SemanticType {
    Namespace,
    Type,
    Function,
    Method,
    Variable,
    Property,
    Parameter,
    Keyword,
    String,
    Number,
    Comment,
    Operator,
    TypeParameter,
}

impl SemanticType {
    const ALL: [Self; 13] = [
        Self::Namespace,
        Self::Type,
        Self::Function,
        Self::Method,
        Self::Variable,
        Self::Property,
        Self::Parameter,
        Self::Keyword,
        Self::String,
        Self::Number,
        Self::Comment,
        Self::Operator,
        Self::TypeParameter,
    ];

    const fn name(self) -> &'static str {
        match self {
            Self::Namespace => "namespace",
            Self::Type => "type",
            Self::Function => "function",
            Self::Method => "method",
            Self::Variable => "variable",
            Self::Property => "property",
            Self::Parameter => "parameter",
            Self::Keyword => "keyword",
            Self::String => "string",
            Self::Number => "number",
            Self::Comment => "comment",
            Self::Operator => "operator",
            Self::TypeParameter => "typeParameter",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SemanticModifier {
    Declaration,
    Definition,
    Readonly,
    DefaultLibrary,
    Static,
    Signature,
    Number,
    Bool,
    String,
    Interface,
    Struct,
    Pointer,
    Array,
    Slice,
    Map,
    Chan,
    Shadowing,
    Format,
}

impl SemanticModifier {
    const ALL: [Self; 18] = [
        Self::Declaration,
        Self::Definition,
        Self::Readonly,
        Self::DefaultLibrary,
        Self::Static,
        Self::Signature,
        Self::Number,
        Self::Bool,
        Self::String,
        Self::Interface,
        Self::Struct,
        Self::Pointer,
        Self::Array,
        Self::Slice,
        Self::Map,
        Self::Chan,
        Self::Shadowing,
        Self::Format,
    ];
    const SHAPES: [Self; 11] = [
        Self::Signature,
        Self::Number,
        Self::Bool,
        Self::String,
        Self::Interface,
        Self::Struct,
        Self::Pointer,
        Self::Array,
        Self::Slice,
        Self::Map,
        Self::Chan,
    ];

    const fn name(self) -> &'static str {
        match self {
            Self::Declaration => "declaration",
            Self::Definition => "definition",
            Self::Readonly => "readonly",
            Self::DefaultLibrary => "defaultLibrary",
            Self::Static => "static",
            Self::Signature => "signature",
            Self::Number => "number",
            Self::Bool => "bool",
            Self::String => "string",
            Self::Interface => "interface",
            Self::Struct => "struct",
            Self::Pointer => "pointer",
            Self::Array => "array",
            Self::Slice => "slice",
            Self::Map => "map",
            Self::Chan => "chan",
            Self::Shadowing => "shadowing",
            Self::Format => "format",
        }
    }

    const fn bit(self) -> u64 {
        1 << self as usize
    }
}

pub(super) const DEFINITION: u64 = SemanticModifier::Definition.bit();
pub(super) const READONLY: u64 = SemanticModifier::Readonly.bit();
pub(super) const DEFAULT_LIBRARY: u64 = SemanticModifier::DefaultLibrary.bit();
pub(super) const STATIC: u64 = SemanticModifier::Static.bit();
pub(super) const SIGNATURE: u64 = SemanticModifier::Signature.bit();
pub(super) const NUMBER: u64 = SemanticModifier::Number.bit();
pub(super) const BOOL: u64 = SemanticModifier::Bool.bit();
pub(super) const STRING: u64 = SemanticModifier::String.bit();
pub(super) const INTERFACE: u64 = SemanticModifier::Interface.bit();
pub(super) const STRUCT: u64 = SemanticModifier::Struct.bit();
pub(super) const POINTER: u64 = SemanticModifier::Pointer.bit();
pub(super) const ARRAY: u64 = SemanticModifier::Array.bit();
pub(super) const SLICE: u64 = SemanticModifier::Slice.bit();
pub(super) const MAP: u64 = SemanticModifier::Map.bit();
pub(super) const CHAN: u64 = SemanticModifier::Chan.bit();
pub(super) const SHADOWING: u64 = SemanticModifier::Shadowing.bit();
/// Non-standard modifier for printf format directives inside string literals.
pub(super) const FORMAT: u64 = SemanticModifier::Format.bit();

pub(crate) struct SemanticConfig {
    types: [Option<u8>; SemanticType::ALL.len()],
    modifiers: [Option<u8>; SemanticModifier::ALL.len()],
}

impl Default for SemanticConfig {
    fn default() -> Self {
        Self::from_initialize(&Value::Null)
    }
}

impl SemanticConfig {
    pub(super) fn type_shapes(&self) -> bool {
        SemanticModifier::SHAPES
            .iter()
            .any(|modifier| self.modifiers[*modifier as usize].is_some())
    }

    pub(crate) fn from_initialize(params: &Value) -> Self {
        let capabilities = &params["capabilities"]["textDocument"]["semanticTokens"];
        let requested = capabilities["tokenTypes"].as_array();
        let mut next = 0;
        let types = SemanticType::ALL.map(|kind| {
            requested
                .is_none_or(|types| {
                    types
                        .iter()
                        .any(|value| value.as_str() == Some(kind.name()))
                })
                .then(|| {
                    let id = next;
                    next += 1;
                    id
                })
        });
        let requested = capabilities["tokenModifiers"].as_array();
        let mut next = 0;
        let modifiers = SemanticModifier::ALL.map(|modifier| {
            requested
                .is_none_or(|values| {
                    values
                        .iter()
                        .any(|value| value.as_str() == Some(modifier.name()))
                })
                .then(|| {
                    let id = next;
                    next += 1;
                    id
                })
        });
        Self { types, modifiers }
    }

    pub(super) fn type_index(&self, kind: SemanticType) -> Option<u8> {
        self.types[kind as usize].or_else(|| {
            (kind == SemanticType::TypeParameter)
                .then_some(self.types[SemanticType::Type as usize])
                .flatten()
        })
    }

    pub(super) fn modifier_bits(&self, mut flags: u64) -> u64 {
        if flags & DEFINITION != 0
            && self.modifiers[SemanticModifier::Definition as usize].is_none()
            && self.modifiers[SemanticModifier::Declaration as usize].is_some()
        {
            // Older clients only understand declaration; definitions retain
            // that presentation when the more precise modifier is unavailable.
            flags |= SemanticModifier::Declaration.bit();
        }
        SemanticModifier::ALL
            .iter()
            .zip(self.modifiers)
            .fold(0, |bits, (modifier, id)| {
                bits | id
                    .filter(|_| flags & modifier.bit() != 0)
                    .map_or(0, |id| 1 << id)
            })
    }

    pub(crate) fn legend(&self) -> SemanticTokensLegend {
        SemanticTokensLegend {
            token_types: SemanticType::ALL
                .iter()
                .zip(self.types)
                .filter_map(|(kind, id)| {
                    id.map(|_| SemanticTokenType::from(kind.name().to_owned()))
                })
                .collect(),
            token_modifiers: SemanticModifier::ALL
                .iter()
                .zip(self.modifiers)
                .filter_map(|(modifier, id)| {
                    id.map(|_| SemanticTokenModifier::from(modifier.name().to_owned()))
                })
                .collect(),
        }
    }
}
