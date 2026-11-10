//! Source spans, declaration metadata and query-local expression arenas.
pub(crate) mod declarations;
pub(crate) mod expression;
pub(crate) mod signature;
pub(crate) mod statement;
pub(crate) mod types;

pub use declarations::{AggregateMember, Import, ImportName, MemberKind, MemberName};
pub use declarations::{Stub, Symbol, SymbolKind, TypeConstructor, TypeInfo, TypeTarget};
pub use expression::{Diagnostic, Expr, ExprId, ExprKind, ExpressionTree};
pub use signature::{Parameter, Signature};
pub use statement::{BlockTree, LocalTypeParameters, Statement, StmtId, StmtKind};
pub use types::{
    InterfaceMember, SignatureName, SignatureNameKind, TypeField, TypeReference, TypeSyntax,
};
