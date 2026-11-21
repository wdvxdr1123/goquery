//! Compact `u32`-indexed arena builder for [`TypeSyntax`] trees.
use crate::syntax::ast::types::{
    FieldNode, InterfaceMemberNode, MAX_TYPE_NODES, ParamNode, SigNode, SigNodeId, TypeNodeId,
    TypeTreeKind, TypeTreeNode,
};
use std::ops::Range;

pub(super) struct TypeTreeBuilder {
    nodes: Vec<TypeTreeNode>,
    fields: Vec<FieldNode>,
    interface_members: Vec<InterfaceMemberNode>,
    sigs: Vec<SigNode>,
    params: Vec<ParamNode>,
    lists: Vec<TypeNodeId>,
    overflowed: bool,
}

pub(super) struct FinishedTypeTree {
    pub(super) root: Option<TypeNodeId>,
    pub(super) nodes: Vec<TypeTreeNode>,
    pub(super) fields: Vec<FieldNode>,
    pub(super) interface_members: Vec<InterfaceMemberNode>,
    pub(super) sigs: Vec<SigNode>,
    pub(super) params: Vec<ParamNode>,
    pub(super) lists: Vec<TypeNodeId>,
    pub(super) overflowed: bool,
}

impl TypeTreeBuilder {
    pub(super) fn new() -> Self {
        Self {
            nodes: Vec::new(),
            fields: Vec::new(),
            interface_members: Vec::new(),
            sigs: Vec::new(),
            params: Vec::new(),
            lists: Vec::new(),
            overflowed: false,
        }
    }

    pub(super) fn span32(span: Range<usize>) -> Range<u32> {
        let start = u32::try_from(span.start).unwrap_or(u32::MAX);
        let end = u32::try_from(span.end).unwrap_or(u32::MAX);
        start..end
    }

    pub(super) fn is_overflowed(&self) -> bool {
        self.overflowed
    }

    pub(super) fn mark_overflow(&mut self) {
        self.overflowed = true;
    }

    pub(super) fn push_node(&mut self, kind: TypeTreeKind, span: Range<usize>) -> TypeNodeId {
        if self.overflowed || self.nodes.len() >= MAX_TYPE_NODES {
            self.overflowed = true;
            return TypeNodeId(0);
        }
        let id = TypeNodeId(self.nodes.len() as u32);
        self.nodes.push(TypeTreeNode {
            kind,
            span: Self::span32(span),
        });
        id
    }

    pub(super) fn push_missing(&mut self, span: Range<usize>) -> TypeNodeId {
        self.push_node(TypeTreeKind::Missing, span)
    }

    pub(super) fn push_fields(&mut self, fields: Vec<FieldNode>) -> Range<u32> {
        if self.overflowed || self.fields.len().saturating_add(fields.len()) > MAX_TYPE_NODES {
            self.overflowed = true;
            return 0..0;
        }
        let start = self.fields.len() as u32;
        self.fields.extend(fields);
        start..self.fields.len() as u32
    }

    pub(super) fn push_interface_members(
        &mut self,
        members: Vec<InterfaceMemberNode>,
    ) -> Range<u32> {
        if self.overflowed
            || self.interface_members.len().saturating_add(members.len()) > MAX_TYPE_NODES
        {
            self.overflowed = true;
            return 0..0;
        }
        let start = self.interface_members.len() as u32;
        self.interface_members.extend(members);
        start..self.interface_members.len() as u32
    }

    pub(super) fn push_sig(
        &mut self,
        span: Range<usize>,
        params: Vec<ParamNode>,
        results: Vec<ParamNode>,
        variadic: bool,
    ) -> SigNodeId {
        let total_params = self
            .params
            .len()
            .saturating_add(params.len())
            .saturating_add(results.len());
        if self.overflowed || self.sigs.len() >= MAX_TYPE_NODES || total_params > MAX_TYPE_NODES {
            self.overflowed = true;
            return SigNodeId(0);
        }
        let params_start = self.params.len() as u32;
        self.params.extend(params);
        let params_end = self.params.len() as u32;
        let results_start = params_end;
        self.params.extend(results);
        let results_end = self.params.len() as u32;
        let id = SigNodeId(self.sigs.len() as u32);
        self.sigs.push(SigNode {
            span: Self::span32(span),
            params: params_start..params_end,
            results: results_start..results_end,
            variadic,
        });
        id
    }

    pub(super) fn push_type_list(&mut self, items: Vec<TypeNodeId>) -> Range<u32> {
        if self.overflowed || self.lists.len().saturating_add(items.len()) > MAX_TYPE_NODES {
            self.overflowed = true;
            return 0..0;
        }
        let start = self.lists.len() as u32;
        self.lists.extend(items);
        start..self.lists.len() as u32
    }

    /// Derives the implicit unqualified field name for an embedded struct field
    /// directly from its parsed type AST node (`T`, `*T`, `pkg.T`, `*pkg.T[A]`).
    pub(super) fn embedded_field_name(&self, ty: TypeNodeId) -> Option<Range<u32>> {
        if self.overflowed {
            return None;
        }
        let mut current = self.nodes.get(ty.index())?;
        if let TypeTreeKind::Pointer(inner) = current.kind {
            current = self.nodes.get(inner.index())?;
        }
        if let TypeTreeKind::Instantiation { base, .. } = current.kind {
            current = self.nodes.get(base.index())?;
        }
        match &current.kind {
            TypeTreeKind::Name { span } => Some(span.clone()),
            TypeTreeKind::Qualified { name, .. } => Some(name.clone()),
            _ => None,
        }
    }

    pub(super) fn finish(self, root: Option<TypeNodeId>) -> FinishedTypeTree {
        if self.overflowed {
            return FinishedTypeTree {
                root: None,
                nodes: Vec::new(),
                fields: Vec::new(),
                interface_members: Vec::new(),
                sigs: Vec::new(),
                params: Vec::new(),
                lists: Vec::new(),
                overflowed: true,
            };
        }
        let valid_root = root.filter(|id| self.nodes.get(id.index()).is_some());
        FinishedTypeTree {
            root: valid_root,
            nodes: self.nodes,
            fields: self.fields,
            interface_members: self.interface_members,
            sigs: self.sigs,
            params: self.params,
            lists: self.lists,
            overflowed: false,
        }
    }
}
