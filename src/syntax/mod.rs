//! Go syntax frontend. All source spans use UTF-8 byte offsets.
//! Declaration extraction streams tokens and skips bodies without building trees.
//! No parser framework, workspace, or LSP dependencies.
mod ast;
mod lexer;
mod literal;
mod parser;
pub mod token;
/// Read-only syntax index for protocol syntax features.
pub mod view;

pub use ast::{AggregateMember, Import, ImportName, MemberKind, MemberName};
pub use ast::{BlockTree, LocalTypeParameters, Statement, StmtId, StmtKind};
pub use ast::{Diagnostic, Expr, ExprId, ExprKind, ExpressionTree, Stub, Symbol, SymbolKind};
pub use ast::{
    InterfaceMember, SignatureName, SignatureNameKind, TypeField, TypeNode, TypeNodeKind,
    TypeReference, TypeSyntax,
};
pub use ast::{Parameter, Signature};
pub use ast::{TypeConstructor, TypeInfo, TypeTarget};
pub use lexer::{Lexer, is_exported_name, is_identifier_continue, is_identifier_start};
pub use literal::decode_import_path;
pub use literal::decode_rune_literal;
pub use literal::decode_string_literal;
pub use literal::quote_bytes;
pub use literal::quote_string;
pub use parser::parse_type;
pub use parser::parse_type_prefix;
pub use parser::validate_file;
pub use parser::{DeclSpecKind, parse_block, parse_decl_spec, parse_declaration};
pub use parser::{aggregate_members, aggregate_offset, type_parameter_end};
pub use parser::{parse_expression, stub};
pub use parser::{
    parse_literal_signature, parse_method_signature, parse_signature, parse_type_parameters,
};
pub(crate) use parser::{qualified_prefixes, validate_file_with_imports};
pub use token::{Token, TokenKind};
