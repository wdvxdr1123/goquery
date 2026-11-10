//! Go basic kinds and untyped/default-type rules, shared by operand checking.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum BasicKind {
    Bool,
    Int,
    Int8,
    Int16,
    Int32,
    Int64,
    Uint,
    Uint8,
    Uint16,
    Uint32,
    Uint64,
    Uintptr,
    Float32,
    Float64,
    Complex64,
    Complex128,
    String,
    Byte,
    Rune,
    UntypedBool,
    UntypedInt,
    UntypedRune,
    UntypedFloat,
    UntypedComplex,
    UntypedString,
    UntypedNil,
}

const BASICS: &[(BasicKind, &str)] = &[
    (BasicKind::UntypedNil, "untyped nil"),
    (BasicKind::Bool, "bool"),
    (BasicKind::Int, "int"),
    (BasicKind::Int8, "int8"),
    (BasicKind::Int16, "int16"),
    (BasicKind::Int32, "int32"),
    (BasicKind::Int64, "int64"),
    (BasicKind::Uint, "uint"),
    (BasicKind::Uint8, "uint8"),
    (BasicKind::Uint16, "uint16"),
    (BasicKind::Uint32, "uint32"),
    (BasicKind::Uint64, "uint64"),
    (BasicKind::Uintptr, "uintptr"),
    (BasicKind::Float32, "float32"),
    (BasicKind::Float64, "float64"),
    (BasicKind::Complex64, "complex64"),
    (BasicKind::Complex128, "complex128"),
    (BasicKind::String, "string"),
    (BasicKind::Byte, "byte"),
    (BasicKind::Rune, "rune"),
    (BasicKind::UntypedBool, "untyped bool"),
    (BasicKind::UntypedInt, "untyped int"),
    (BasicKind::UntypedRune, "untyped rune"),
    (BasicKind::UntypedFloat, "untyped float"),
    (BasicKind::UntypedComplex, "untyped complex"),
    (BasicKind::UntypedString, "untyped string"),
];

impl BasicKind {
    pub(crate) fn named(name: &str) -> Option<Self> {
        BASICS
            .iter()
            .find(|(kind, spelling)| !kind.untyped() && *spelling == name)
            .map(|(kind, _)| *kind)
    }
    pub(crate) fn name(self) -> &'static str {
        BASICS
            .iter()
            .find(|(kind, _)| *kind == self)
            .expect("all basic kinds have names")
            .1
    }
    pub(crate) fn untyped(self) -> bool {
        matches!(
            self,
            Self::UntypedBool
                | Self::UntypedInt
                | Self::UntypedRune
                | Self::UntypedFloat
                | Self::UntypedComplex
                | Self::UntypedString
                | Self::UntypedNil
        )
    }
    pub(crate) fn default_type(self) -> Self {
        match self {
            Self::UntypedBool => Self::Bool,
            Self::UntypedInt => Self::Int,
            Self::UntypedRune => Self::Rune,
            Self::UntypedFloat => Self::Float64,
            Self::UntypedComplex => Self::Complex128,
            Self::UntypedString => Self::String,
            _ => self,
        }
    }
    pub(crate) fn canonical(self) -> Self {
        match self {
            Self::Byte => Self::Uint8,
            Self::Rune => Self::Int32,
            _ => self,
        }
    }
    pub(crate) fn integer(self) -> bool {
        self.integer_width(64).is_some() || matches!(self, Self::UntypedInt | Self::UntypedRune)
    }
    pub(crate) fn numeric_rank(self) -> Option<u8> {
        if self.integer() {
            return Some(if self == Self::UntypedRune { 1 } else { 0 });
        }
        match self {
            Self::Float32 | Self::Float64 | Self::UntypedFloat => Some(2),
            Self::Complex64 | Self::Complex128 | Self::UntypedComplex => Some(3),
            _ => None,
        }
    }
    pub(crate) fn boolean(self) -> bool {
        matches!(self, Self::Bool | Self::UntypedBool)
    }
    pub(crate) fn string(self) -> bool {
        matches!(self, Self::String | Self::UntypedString)
    }
    pub(crate) fn integer_width(self, int_bits: u32) -> Option<(bool, u32)> {
        match self.canonical() {
            Self::Int => Some((true, int_bits)),
            Self::Uint | Self::Uintptr => Some((false, int_bits)),
            Self::Int8 => Some((true, 8)),
            Self::Int16 => Some((true, 16)),
            Self::Int32 => Some((true, 32)),
            Self::Int64 => Some((true, 64)),
            Self::Uint8 => Some((false, 8)),
            Self::Uint16 => Some((false, 16)),
            Self::Uint32 => Some((false, 32)),
            Self::Uint64 => Some((false, 64)),
            _ => None,
        }
    }
}
