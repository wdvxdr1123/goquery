//! Type references are source spans, plus a compact `u32`-indexed AST tree.
use crate::syntax::ast::expression::{Diagnostic, Expr, ExprId};
use std::ops::Range;

/// Bound for the flat type-shape summary and indexed type-tree arenas. Excess
/// nodes trigger overflow handling; recursive syntax work stays bounded by the
/// parser depth limit.
pub(crate) const MAX_TYPE_NODES: usize = 8192;

/// Compact `u32` index into [`TypeSyntax::tree_nodes`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TypeNodeId(pub(crate) u32);

impl TypeNodeId {
    pub fn index(self) -> usize {
        self.0 as usize
    }

    pub fn raw(self) -> u32 {
        self.0
    }
}

/// Compact `u32` index into [`TypeSyntax::sig_nodes`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SigNodeId(pub(crate) u32);

impl SigNodeId {
    pub fn index(self) -> usize {
        self.0 as usize
    }

    pub fn raw(self) -> u32 {
        self.0
    }
}

/// Channel direction for [`TypeTreeKind::Chan`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ChanDir {
    Both,
    Send,
    Recv,
}

/// Structured type AST node kind using compact `u32` arena indices (zero `Box`).
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum TypeTreeKind {
    /// Recovery node for malformed, incomplete, or depth-limited input.
    Missing,
    Parenthesized(TypeNodeId),
    Pointer(TypeNodeId),
    Slice(TypeNodeId),
    /// Fixed-length `[N]T` (`len_expr: Some(id)`) or inferred `[...]T` (`len_expr: None`).
    Array {
        len_expr: Option<ExprId>,
        elem: TypeNodeId,
    },
    Map {
        key: TypeNodeId,
        value: TypeNodeId,
    },
    Chan {
        dir: ChanDir,
        elem: TypeNodeId,
    },
    Func(SigNodeId),
    /// `fields` indexes [`TypeSyntax::field_nodes`].
    Struct {
        fields: Range<u32>,
    },
    /// `members` indexes [`TypeSyntax::interface_member_nodes`].
    Interface {
        members: Range<u32>,
    },
    /// `terms` indexes [`TypeSyntax::type_node_list`].
    Union {
        terms: Range<u32>,
    },
    /// Underlying-type constraint term `~T`.
    Approx(TypeNodeId),
    Name {
        span: Range<u32>,
    },
    Qualified {
        qual: Range<u32>,
        name: Range<u32>,
    },
    /// `args` indexes [`TypeSyntax::type_node_list`].
    Instantiation {
        base: TypeNodeId,
        args: Range<u32>,
    },
}

/// One node in the compact, children-first indexed type AST.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct TypeTreeNode {
    pub kind: TypeTreeKind,
    pub span: Range<u32>,
}

impl TypeTreeNode {
    pub fn byte_range(&self) -> Range<usize> {
        self.span.start as usize..self.span.end as usize
    }
}

/// One struct field in the indexed type AST.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct FieldNode {
    /// Explicit field name, or the unqualified written embedded type name.
    pub name: Option<Range<u32>>,
    pub ty: TypeNodeId,
    pub embedded: bool,
    /// Undecoded string literal tag, shared by all names in a grouped declaration.
    pub tag: Option<Range<u32>>,
}

/// One interface member in the indexed type AST.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum InterfaceMemberNode {
    Method { name: Range<u32>, sig: SigNodeId },
    Embedded(TypeNodeId),
}

/// One input parameter or result in a [`SigNode`].
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ParamNode {
    pub name: Option<Range<u32>>,
    /// Parameter type (for variadic `...T`, this is the element type `T`).
    pub ty: TypeNodeId,
    pub variadic: bool,
}

/// Function type or interface method signature in the indexed type AST.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct SigNode {
    /// Span covering `(params...) results...`.
    pub span: Range<u32>,
    /// Indexes [`TypeSyntax::param_nodes`].
    pub params: Range<u32>,
    /// Indexes [`TypeSyntax::param_nodes`].
    pub results: Range<u32>,
    pub variadic: bool,
}

#[cfg(target_pointer_width = "64")]
const _: () = {
    assert!(std::mem::size_of::<TypeNodeId>() == 4);
    assert!(std::mem::size_of::<SigNodeId>() == 4);
    assert!(std::mem::size_of::<TypeTreeKind>() <= 24);
    assert!(std::mem::size_of::<TypeTreeNode>() <= 32);
    assert!(std::mem::size_of::<FieldNode>() <= 32);
    assert!(std::mem::size_of::<InterfaceMemberNode>() <= 16);
    assert!(std::mem::size_of::<ParamNode>() <= 24);
    assert!(std::mem::size_of::<SigNode>() <= 32);
};

/// One node of the flat, pre-order type-shape summary. This is not a tree:
/// children are identified only by their source spans.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum TypeNodeKind {
    Name,
    Qualified,
    Instantiation,
    Pointer,
    Array,
    Slice,
    Map,
    Chan,
    SendChan,
    RecvChan,
    Function,
    Struct,
    Interface,
    Parenthesized,
}

/// A shallow type form with its byte span in the parsed source snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct TypeNode {
    pub kind: TypeNodeKind,
    pub span: Range<usize>,
}

/// A type-name occurrence with an optional file-local import qualifier.
#[derive(Debug)]
#[non_exhaustive]
pub struct TypeReference {
    pub qualifier: Option<Range<usize>>,
    pub name: Range<usize>,
}

/// A name declared inside a function type or interface method signature.
#[derive(Debug)]
#[non_exhaustive]
pub struct SignatureName {
    pub name: Range<usize>,
    pub kind: SignatureNameKind,
    pub annotation: Range<usize>,
    pub variadic: bool,
}

/// Input parameters and named results have distinct semantic categories.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SignatureNameKind {
    Parameter,
    Result,
}

/// A struct field, with its containing opening-brace byte offset.
#[derive(Debug)]
#[non_exhaustive]
pub struct TypeField {
    /// Explicit field name, or the unqualified written embedded type name.
    pub name: Range<usize>,
    pub annotation: Range<usize>,
    pub container: usize,
    pub embedded: bool,
    /// Undecoded string literal, shared by all names in a grouped declaration.
    pub tag: Option<Range<usize>>,
}

/// An explicit interface method or embedded type/type term, with byte spans.
#[derive(Debug)]
#[non_exhaustive]
pub struct InterfaceMember {
    pub name: Option<Range<usize>>,
    /// Method parameter/result signature, or the complete embedded type term.
    pub annotation: Range<usize>,
    pub container: usize,
}

/// On-demand type syntax, including expressions used as array lengths and a
/// compact `u32`-indexed AST tree.
#[derive(Debug)]
#[non_exhaustive]
pub struct TypeSyntax {
    pub span: Range<usize>,
    pub references: Vec<TypeReference>,
    pub signature_names: Vec<SignatureName>,
    pub fields: Vec<TypeField>,
    pub interface_members: Vec<InterfaceMember>,
    pub(crate) expressions: Vec<Expr>,
    pub(crate) diagnostics: Vec<Diagnostic>,
    pub(crate) type_nodes: Vec<TypeNode>,
    pub(crate) tree_root: Option<TypeNodeId>,
    pub(crate) tree_nodes: Vec<TypeTreeNode>,
    pub(crate) field_nodes: Vec<FieldNode>,
    pub(crate) interface_member_nodes: Vec<InterfaceMemberNode>,
    pub(crate) sig_nodes: Vec<SigNode>,
    pub(crate) param_nodes: Vec<ParamNode>,
    pub(crate) type_node_lists: Vec<TypeNodeId>,
    pub(crate) tree_overflowed: bool,
}

impl TypeSyntax {
    pub fn expressions(&self) -> &[Expr] {
        &self.expressions
    }

    pub fn expression(&self, id: ExprId) -> Option<&Expr> {
        self.expressions.get(id.index())
    }

    /// Flat pre-order type forms; array lengths and nested expression syntax
    /// are not type nodes.
    pub fn type_nodes(&self) -> &[TypeNode] {
        &self.type_nodes
    }

    /// Root node of the compact indexed type AST, or `None` if an arena budget
    /// was exceeded.
    pub fn root(&self) -> Option<TypeNodeId> {
        self.tree_root
    }

    /// True if building the indexed type AST exceeded [`MAX_TYPE_NODES`]; when
    /// true, [`Self::root`] is `None` and the tree arenas are empty.
    pub fn tree_overflowed(&self) -> bool {
        self.tree_overflowed
    }

    /// All nodes in the compact indexed type AST (children-first).
    pub fn tree_nodes(&self) -> &[TypeTreeNode] {
        &self.tree_nodes
    }

    pub fn tree_node(&self, id: TypeNodeId) -> Option<&TypeTreeNode> {
        self.tree_nodes.get(id.index())
    }

    pub fn field_nodes(&self) -> &[FieldNode] {
        &self.field_nodes
    }

    pub fn struct_fields(&self, range: Range<u32>) -> &[FieldNode] {
        self.field_nodes
            .get(range.start as usize..range.end as usize)
            .unwrap_or(&[])
    }

    pub fn interface_member_nodes(&self) -> &[InterfaceMemberNode] {
        &self.interface_member_nodes
    }

    pub fn interface_members(&self, range: Range<u32>) -> &[InterfaceMemberNode] {
        self.interface_member_nodes
            .get(range.start as usize..range.end as usize)
            .unwrap_or(&[])
    }

    pub fn sig_nodes(&self) -> &[SigNode] {
        &self.sig_nodes
    }

    pub fn sig_node(&self, id: SigNodeId) -> Option<&SigNode> {
        self.sig_nodes.get(id.index())
    }

    pub fn param_nodes(&self) -> &[ParamNode] {
        &self.param_nodes
    }

    pub fn sig_params(&self, range: Range<u32>) -> &[ParamNode] {
        self.param_nodes
            .get(range.start as usize..range.end as usize)
            .unwrap_or(&[])
    }

    pub fn type_node_lists(&self) -> &[TypeNodeId] {
        &self.type_node_lists
    }

    pub fn type_node_list(&self, range: Range<u32>) -> &[TypeNodeId] {
        self.type_node_lists
            .get(range.start as usize..range.end as usize)
            .unwrap_or(&[])
    }

    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    pub fn into_diagnostics(self) -> Vec<Diagnostic> {
        self.diagnostics
    }
}
