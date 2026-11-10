//! Go lexical vocabulary, named after go/token using Rust casing.
use std::ops::Range;

/// Each reserved word and operator has a distinct lexical identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum TokenKind {
    Illegal,
    Eof,
    Comment,
    Ident,
    Int,
    Float,
    Imag,
    Char,
    String,
    Add,
    Sub,
    Mul,
    Quo,
    Rem,
    And,
    Or,
    Xor,
    Shl,
    Shr,
    AndNot,
    AddAssign,
    SubAssign,
    MulAssign,
    QuoAssign,
    RemAssign,
    AndAssign,
    OrAssign,
    XorAssign,
    ShlAssign,
    ShrAssign,
    AndNotAssign,
    Land,
    Lor,
    Arrow,
    Inc,
    Dec,
    Eql,
    Lss,
    Gtr,
    Assign,
    Not,
    Neq,
    Leq,
    Geq,
    Define,
    Ellipsis,
    LParen,
    LBrack,
    LBrace,
    Comma,
    Period,
    RParen,
    RBrack,
    RBrace,
    Semicolon,
    Colon,
    Tilde,
    Break,
    Case,
    Chan,
    Const,
    Continue,
    Default,
    Defer,
    Else,
    Fallthrough,
    For,
    Func,
    Go,
    Goto,
    If,
    Import,
    Interface,
    Map,
    Package,
    Range,
    Return,
    Select,
    Struct,
    Switch,
    Type,
    Var,
}
impl TokenKind {
    /// Go binary operator precedence; non-binary tokens return zero.
    pub const fn precedence(self) -> u8 {
        match self {
            Self::Lor => 1,
            Self::Land => 2,
            Self::Eql | Self::Neq | Self::Lss | Self::Leq | Self::Gtr | Self::Geq => 3,
            Self::Add | Self::Sub | Self::Or | Self::Xor => 4,
            Self::Mul
            | Self::Quo
            | Self::Rem
            | Self::Shl
            | Self::Shr
            | Self::And
            | Self::AndNot => 5,
            _ => 0,
        }
    }
    /// Recognizes reserved words; predeclared names remain identifiers.
    pub fn from_keyword(text: &str) -> Option<Self> {
        Some(match text {
            "break" => Self::Break,
            "case" => Self::Case,
            "chan" => Self::Chan,
            "const" => Self::Const,
            "continue" => Self::Continue,
            "default" => Self::Default,
            "defer" => Self::Defer,
            "else" => Self::Else,
            "fallthrough" => Self::Fallthrough,
            "for" => Self::For,
            "func" => Self::Func,
            "go" => Self::Go,
            "goto" => Self::Goto,
            "if" => Self::If,
            "import" => Self::Import,
            "interface" => Self::Interface,
            "map" => Self::Map,
            "package" => Self::Package,
            "range" => Self::Range,
            "return" => Self::Return,
            "select" => Self::Select,
            "struct" => Self::Struct,
            "switch" => Self::Switch,
            "type" => Self::Type,
            "var" => Self::Var,
            _ => return None,
        })
    }
    /// Recognizes the longest operator/delimiter prefix and its byte length.
    pub fn from_symbol_prefix(text: &str) -> Option<(Self, usize)> {
        const SYMBOLS: &[(&str, TokenKind)] = &[
            ("<<=", TokenKind::ShlAssign),
            (">>=", TokenKind::ShrAssign),
            ("&^=", TokenKind::AndNotAssign),
            ("...", TokenKind::Ellipsis),
            ("<<", TokenKind::Shl),
            (">>", TokenKind::Shr),
            ("&^", TokenKind::AndNot),
            ("+=", TokenKind::AddAssign),
            ("-=", TokenKind::SubAssign),
            ("*=", TokenKind::MulAssign),
            ("/=", TokenKind::QuoAssign),
            ("%=", TokenKind::RemAssign),
            ("&=", TokenKind::AndAssign),
            ("|=", TokenKind::OrAssign),
            ("^=", TokenKind::XorAssign),
            ("&&", TokenKind::Land),
            ("||", TokenKind::Lor),
            ("<-", TokenKind::Arrow),
            ("++", TokenKind::Inc),
            ("--", TokenKind::Dec),
            ("==", TokenKind::Eql),
            ("!=", TokenKind::Neq),
            ("<=", TokenKind::Leq),
            (">=", TokenKind::Geq),
            (":=", TokenKind::Define),
            ("+", TokenKind::Add),
            ("-", TokenKind::Sub),
            ("*", TokenKind::Mul),
            ("/", TokenKind::Quo),
            ("%", TokenKind::Rem),
            ("&", TokenKind::And),
            ("|", TokenKind::Or),
            ("^", TokenKind::Xor),
            ("<", TokenKind::Lss),
            (">", TokenKind::Gtr),
            ("=", TokenKind::Assign),
            ("!", TokenKind::Not),
            ("(", TokenKind::LParen),
            ("[", TokenKind::LBrack),
            ("{", TokenKind::LBrace),
            (",", TokenKind::Comma),
            (".", TokenKind::Period),
            (")", TokenKind::RParen),
            ("]", TokenKind::RBrack),
            ("}", TokenKind::RBrace),
            (";", TokenKind::Semicolon),
            (":", TokenKind::Colon),
            ("~", TokenKind::Tilde),
        ];
        SYMBOLS
            .iter()
            .find(|(spelling, _)| text.starts_with(spelling))
            .map(|(spelling, kind)| (*kind, spelling.len()))
    }

    /// Fixed spelling, or none for source-backed literals and special tokens.
    pub const fn fixed_text(self) -> Option<&'static str> {
        Some(match self {
            Self::Add => "+",
            Self::Sub => "-",
            Self::Mul => "*",
            Self::Quo => "/",
            Self::Rem => "%",
            Self::And => "&",
            Self::Or => "|",
            Self::Xor => "^",
            Self::Shl => "<<",
            Self::Shr => ">>",
            Self::AndNot => "&^",
            Self::AddAssign => "+=",
            Self::SubAssign => "-=",
            Self::MulAssign => "*=",
            Self::QuoAssign => "/=",
            Self::RemAssign => "%=",
            Self::AndAssign => "&=",
            Self::OrAssign => "|=",
            Self::XorAssign => "^=",
            Self::ShlAssign => "<<=",
            Self::ShrAssign => ">>=",
            Self::AndNotAssign => "&^=",
            Self::Land => "&&",
            Self::Lor => "||",
            Self::Arrow => "<-",
            Self::Inc => "++",
            Self::Dec => "--",
            Self::Eql => "==",
            Self::Lss => "<",
            Self::Gtr => ">",
            Self::Assign => "=",
            Self::Not => "!",
            Self::Neq => "!=",
            Self::Leq => "<=",
            Self::Geq => ">=",
            Self::Define => ":=",
            Self::Ellipsis => "...",
            Self::LParen => "(",
            Self::LBrack => "[",
            Self::LBrace => "{",
            Self::Comma => ",",
            Self::Period => ".",
            Self::RParen => ")",
            Self::RBrack => "]",
            Self::RBrace => "}",
            Self::Semicolon => ";",
            Self::Colon => ":",
            Self::Tilde => "~",
            Self::Break => "break",
            Self::Case => "case",
            Self::Chan => "chan",
            Self::Const => "const",
            Self::Continue => "continue",
            Self::Default => "default",
            Self::Defer => "defer",
            Self::Else => "else",
            Self::Fallthrough => "fallthrough",
            Self::For => "for",
            Self::Func => "func",
            Self::Go => "go",
            Self::Goto => "goto",
            Self::If => "if",
            Self::Import => "import",
            Self::Interface => "interface",
            Self::Map => "map",
            Self::Package => "package",
            Self::Range => "range",
            Self::Return => "return",
            Self::Select => "select",
            Self::Struct => "struct",
            Self::Switch => "switch",
            Self::Type => "type",
            Self::Var => "var",
            _ => return None,
        })
    }
    /// Go automatic-semicolon rule for a preceding token.
    pub const fn can_end_statement(self) -> bool {
        matches!(
            self,
            Self::Ident
                | Self::Int
                | Self::Float
                | Self::Imag
                | Self::Char
                | Self::String
                | Self::RParen
                | Self::RBrack
                | Self::RBrace
                | Self::Break
                | Self::Continue
                | Self::Fallthrough
                | Self::Return
                | Self::Inc
                | Self::Dec
        )
    }
}
/// Source-backed token; inserted semicolons have empty byte spans.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Token {
    pub kind: TokenKind,
    pub span: Range<usize>,
    pub inserted: bool,
}
