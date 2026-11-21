//! Compact parent-indexed lexical scope tree storing zero heap strings.

use crate::analysis::engine::budget::{MAX_BYTES, MAX_DEPTH};
use crate::analysis::engine::obj::{LocalId, ScopeId};
use crate::syntax::{BlockTree, Expr, ExprId, ExprKind, Signature, StmtId, StmtKind};
use std::ops::Range;

const MAX_SCOPE_NODES: usize = 4096;

/// Kind of lexical scope node inside a Go function/literal body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ScopeKind {
    FuncSignature,
    Block,
    If,
    For,
    Switch,
    TypeSwitch,
    CaseClause,
    CommClause,
}

/// Parent-indexed scope node; spans are file-relative UTF-8 byte offsets.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ScopeNode {
    pub(crate) parent: Option<ScopeId>,
    pub(crate) kind: ScopeKind,
    pub(crate) extent: [u32; 2],
    pub(crate) last_binding: Option<LocalId>,
}

/// Classification of a lexical binding within a function or closure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LocalBindingKind {
    Receiver,
    TypeParam,
    Param,
    NamedResult,
    Var,
    Const,
    TypeDecl { alias: bool },
    ShortVar,
    RangeKey,
    RangeVal,
    TypeSwitchVar,
}

/// Declaration / initializer coordinates for lazy local type inference.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BindingInit {
    Explicit {
        decl_stmt: StmtId,
        spec_index: u16,
        arity_index: u16,
    },
    ShortDefine {
        stmt: StmtId,
        lhs_index: u16,
    },
    Range {
        clause: StmtId,
        is_value: bool,
    },
    TypeSwitch {
        guard_stmt: StmtId,
        case_stmt: StmtId,
    },
    SignatureParam {
        index: u16,
    },
    None,
}

/// One local variable, constant, type, parameter, or type-switch variable.
#[derive(Clone, Copy, Debug)]
pub(crate) struct LocalBinding {
    pub(crate) prev_in_scope: Option<LocalId>,
    pub(crate) name_span: [u32; 2],
    pub(crate) visible_from: u32,
    pub(crate) kind: LocalBindingKind,
    pub(crate) init: BindingInit,
}

/// Parent-indexed lexical tree for a single function/method body (and relevant closures).
pub(crate) struct ScopeTree {
    pub(crate) nodes: Vec<ScopeNode>,
    pub(crate) bindings: Vec<LocalBinding>,
    pub(crate) labels: Vec<[u32; 2]>,
    pub(crate) range_blanks: Vec<[u32; 2]>,
    /// False when lexical scope construction was truncated by a budget.
    pub(crate) complete: bool,
}

impl ScopeTree {
    pub(crate) fn empty(extent: [u32; 2]) -> Self {
        Self {
            nodes: vec![ScopeNode {
                parent: None,
                kind: ScopeKind::FuncSignature,
                extent,
                last_binding: None,
            }],
            bindings: Vec::new(),
            labels: Vec::new(),
            range_blanks: Vec::new(),
            complete: true,
        }
    }

    pub(crate) fn from_function(
        source: &str,
        sig_base: usize,
        sig: Option<&Signature>,
        body_base: usize,
        body: &BlockTree,
        cursor_offset: Option<usize>,
    ) -> Self {
        let root_span = body
            .statement(body.root())
            .map_or(0..0, |stmt| stmt.span.clone());
        let body_end = body_base.saturating_add(root_span.end).min(source.len());
        let start = sig_base.min(body_base);
        let mut tree = Self::empty([
            u32::try_from(start).unwrap_or(0),
            u32::try_from(body_end).unwrap_or(0),
        ]);
        tree.complete = body.diagnostics().is_empty()
            && sig.is_none_or(|signature| signature.diagnostics().is_empty())
            && cursor_offset.is_none();
        let root_scope = ScopeId(0);
        if let Some(signature) = sig {
            tree.populate_signature(source, root_scope, sig_base, body_base, signature);
        }
        let mut builder = TreeBuilder {
            tree: &mut tree,
            source,
            cursor_offset,
        };
        builder.walk_function_body(root_scope, body_base, body, 0);
        tree
    }

    pub(crate) fn from_top_level_decl(
        source: &str,
        body_base: usize,
        body: &BlockTree,
        cursor_offset: Option<usize>,
    ) -> Self {
        let root_span = body
            .statement(body.root())
            .map_or(0..0, |stmt| stmt.span.clone());
        let body_end = body_base.saturating_add(root_span.end).min(source.len());
        let mut tree = Self::empty([
            u32::try_from(body_base).unwrap_or(0),
            u32::try_from(body_end).unwrap_or(0),
        ]);
        tree.complete = body.diagnostics().is_empty() && cursor_offset.is_none();
        let root_scope = ScopeId(0);
        let mut builder = TreeBuilder {
            tree: &mut tree,
            source,
            cursor_offset,
        };
        builder.walk_top_level_stmt(root_scope, body_base, body, body.root(), 0);
        tree
    }

    fn push_scope(
        &mut self,
        parent: Option<ScopeId>,
        kind: ScopeKind,
        extent: [u32; 2],
    ) -> Option<ScopeId> {
        if self.nodes.len() >= MAX_SCOPE_NODES {
            self.complete = false;
            return None;
        }
        let id = ScopeId(u32::try_from(self.nodes.len()).ok()?);
        self.nodes.push(ScopeNode {
            parent,
            kind,
            extent,
            last_binding: None,
        });
        Some(id)
    }

    fn push_binding(
        &mut self,
        scope: ScopeId,
        name_span: [u32; 2],
        visible_from: u32,
        kind: LocalBindingKind,
        init: BindingInit,
    ) -> Option<LocalId> {
        if self.bindings.len() >= MAX_SCOPE_NODES {
            self.complete = false;
            return None;
        }
        let id = LocalId(u32::try_from(self.bindings.len()).ok()?);
        let prev = self.nodes.get(scope.0 as usize)?.last_binding;
        self.bindings.push(LocalBinding {
            prev_in_scope: prev,
            name_span,
            visible_from,
            kind,
            init,
        });
        self.nodes[scope.0 as usize].last_binding = Some(id);
        Some(id)
    }

    fn has_in_scope(&self, source: &str, scope: ScopeId, name: &str) -> bool {
        let mut cursor = self
            .nodes
            .get(scope.0 as usize)
            .and_then(|node| node.last_binding);
        while let Some(id) = cursor {
            let binding = &self.bindings[id.0 as usize];
            if span_text(source, binding.name_span) == Some(name) {
                return true;
            }
            cursor = binding.prev_in_scope;
        }
        false
    }

    pub(crate) fn binding_case_scope(&self, target: LocalId) -> Option<[u32; 2]> {
        for node in &self.nodes {
            if node.kind != ScopeKind::CaseClause {
                continue;
            }
            let mut cursor = node.last_binding;
            while let Some(id) = cursor {
                if id == target {
                    return Some(node.extent);
                }
                cursor = self.bindings.get(id.0 as usize)?.prev_in_scope;
            }
        }
        None
    }

    pub(crate) fn binding_scope(&self, target: LocalId) -> Option<ScopeId> {
        for (index, node) in self.nodes.iter().enumerate() {
            let mut cursor = node.last_binding;
            while let Some(id) = cursor {
                if id == target {
                    return Some(ScopeId(index as u32));
                }
                cursor = self.bindings.get(id.0 as usize)?.prev_in_scope;
            }
        }
        None
    }

    pub(crate) fn has_outer_binding(&self, source: &str, target: LocalId, name: &str) -> bool {
        let Some(target_binding) = self.bindings.get(target.0 as usize) else {
            return false;
        };
        let Some(scope_id) = self.binding_scope(target) else {
            return false;
        };
        let mut current = self.nodes.get(scope_id.0 as usize).and_then(|n| n.parent);
        while let Some(outer_scope_id) = current {
            let Some(node) = self.nodes.get(outer_scope_id.0 as usize) else {
                break;
            };
            let mut cursor = node.last_binding;
            while let Some(id) = cursor {
                let outer = &self.bindings[id.0 as usize];
                if outer.visible_from <= target_binding.name_span[0]
                    && outer.name_span != target_binding.name_span
                    && span_text(source, outer.name_span) == Some(name)
                {
                    return true;
                }
                cursor = outer.prev_in_scope;
            }
            current = node.parent;
        }
        false
    }

    pub(crate) fn has_outer_binding_at_span(
        &self,
        source: &str,
        def_span: [u32; 2],
        name: &str,
    ) -> bool {
        self.bindings
            .iter()
            .enumerate()
            .filter(|(_, binding)| binding.name_span == def_span)
            .any(|(index, _)| self.has_outer_binding(source, LocalId(index as u32), name))
    }

    pub(crate) fn binding_at_exact_span(
        &self,
        span: [u32; 2],
    ) -> Option<(LocalId, LocalBindingKind, [u32; 2])> {
        self.bindings
            .iter()
            .enumerate()
            .find(|(_, binding)| binding.name_span == span)
            .map(|(index, binding)| (LocalId(index as u32), binding.kind, binding.name_span))
    }

    pub(crate) fn is_range_blank(&self, span: [u32; 2]) -> bool {
        self.range_blanks.contains(&span)
    }

    fn populate_signature(
        &mut self,
        source: &str,
        scope: ScopeId,
        sig_base: usize,
        body_base: usize,
        signature: &Signature,
    ) {
        let visible_body = u32::try_from(body_base).unwrap_or(0);
        for (index, parameter) in signature.type_parameters.iter().enumerate() {
            let Some(span) = &parameter.name else {
                continue;
            };
            let abs = to_abs_span(sig_base, span);
            if span_text(source, abs) == Some("_") {
                continue;
            }
            self.push_binding(
                scope,
                abs,
                abs[0],
                LocalBindingKind::TypeParam,
                BindingInit::SignatureParam {
                    index: u16::try_from(index).unwrap_or(u16::MAX),
                },
            );
        }
        if let Some(receiver) = signature.receiver.first() {
            if let Some(sig_slice) = source.get(sig_base..) {
                for span in crate::analysis::engine::scope::receiver_parameters(
                    sig_slice,
                    &receiver.annotation,
                ) {
                    let abs = to_abs_span(sig_base, &span);
                    if span_text(source, abs) == Some("_")
                        || self.has_in_scope(source, scope, span_text(source, abs).unwrap_or(""))
                    {
                        continue;
                    }
                    self.push_binding(
                        scope,
                        abs,
                        visible_body,
                        LocalBindingKind::TypeParam,
                        BindingInit::None,
                    );
                }
            }
            if let Some(name) = &receiver.name {
                let abs = to_abs_span(sig_base, name);
                if span_text(source, abs) != Some("_") {
                    self.push_binding(
                        scope,
                        abs,
                        visible_body,
                        LocalBindingKind::Receiver,
                        BindingInit::SignatureParam { index: 0 },
                    );
                }
            }
        }
        for (index, parameter) in signature.parameters.iter().enumerate() {
            let Some(name) = &parameter.name else {
                continue;
            };
            let abs = to_abs_span(sig_base, name);
            if span_text(source, abs) == Some("_") {
                continue;
            }
            self.push_binding(
                scope,
                abs,
                visible_body,
                LocalBindingKind::Param,
                BindingInit::SignatureParam {
                    index: u16::try_from(index).unwrap_or(u16::MAX),
                },
            );
        }
        for (index, result) in signature.results.iter().enumerate() {
            let Some(name) = &result.name else {
                continue;
            };
            let abs = to_abs_span(sig_base, name);
            if span_text(source, abs) == Some("_") {
                continue;
            }
            self.push_binding(
                scope,
                abs,
                visible_body,
                LocalBindingKind::NamedResult,
                BindingInit::SignatureParam {
                    index: u16::try_from(index).unwrap_or(u16::MAX),
                },
            );
        }
    }

    pub(crate) fn innermost_scope(&self, offset: u32) -> ScopeId {
        let mut best_id = ScopeId(0);
        let mut best_len = u32::MAX;
        for (index, node) in self.nodes.iter().enumerate() {
            if node.extent[0] <= offset && offset <= node.extent[1] {
                let len = node.extent[1].saturating_sub(node.extent[0]);
                if len <= best_len {
                    best_len = len;
                    best_id = ScopeId(index as u32);
                }
            }
        }
        best_id
    }

    pub(crate) fn resolve_at(&self, source: &str, offset: u32, ident: &str) -> Option<LocalId> {
        let mut current = Some(self.innermost_scope(offset));
        while let Some(scope_id) = current {
            let node = self.nodes.get(scope_id.0 as usize)?;
            let mut binding_id = node.last_binding;
            while let Some(id) = binding_id {
                let binding = &self.bindings[id.0 as usize];
                if binding.visible_from <= offset
                    && span_text(source, binding.name_span) == Some(ident)
                {
                    return Some(id);
                }
                binding_id = binding.prev_in_scope;
            }
            current = node.parent;
        }
        None
    }

    pub(crate) fn resolve_label(&self, source: &str, label: &str) -> Option<[u32; 2]> {
        self.labels
            .iter()
            .copied()
            .find(|&span| span_text(source, span) == Some(label))
    }

    pub(crate) fn definition_at(&self, offset: u32) -> Option<LocalId> {
        self.bindings
            .iter()
            .position(|binding| binding.name_span[0] <= offset && offset <= binding.name_span[1])
            .map(|index| LocalId(index as u32))
    }

    /// Names inside a function type have declaration identities, but no body
    /// where they can be used. Keep them out of the enclosing scope's chain.
    pub(crate) fn signature_name(
        &mut self,
        name_span: [u32; 2],
        kind: LocalBindingKind,
    ) -> Option<LocalId> {
        if let Some(index) = self
            .bindings
            .iter()
            .position(|binding| binding.name_span == name_span)
        {
            return Some(LocalId(index as u32));
        }
        if self.bindings.len() >= MAX_SCOPE_NODES {
            self.complete = false;
            return None;
        }
        let id = LocalId(u32::try_from(self.bindings.len()).ok()?);
        self.bindings.push(LocalBinding {
            prev_in_scope: None,
            name_span,
            visible_from: u32::MAX,
            kind,
            init: BindingInit::None,
        });
        Some(id)
    }

    /// Type annotations may contain array-length expressions with closures.
    /// Admit their lexical scopes before the checker walks those expressions.
    pub(crate) fn include_type_expressions(
        &mut self,
        source: &str,
        base: usize,
        expressions: &BlockTree,
        roots: &[ExprId],
        cursor_offset: Option<usize>,
    ) {
        let scope = self.innermost_scope(u32::try_from(base).unwrap_or(u32::MAX));
        let mut builder = TreeBuilder {
            tree: self,
            source,
            cursor_offset,
        };
        for root in roots {
            builder.walk_expr(scope, base, expressions, *root, 0);
        }
    }

    pub(crate) fn visible_bindings<'t>(
        &'t self,
        source: &'t str,
        offset: u32,
    ) -> Vec<(&'t str, LocalId)> {
        let mut visible = Vec::new();
        let mut current = Some(self.innermost_scope(offset));
        while let Some(scope_id) = current {
            let Some(node) = self.nodes.get(scope_id.0 as usize) else {
                break;
            };
            let mut binding_id = node.last_binding;
            while let Some(id) = binding_id {
                let binding = &self.bindings[id.0 as usize];
                if binding.visible_from <= offset
                    && let Some(name) = span_text(source, binding.name_span)
                    && !visible.iter().any(|&(seen, _)| seen == name)
                {
                    visible.push((name, id));
                }
                binding_id = binding.prev_in_scope;
            }
            current = node.parent;
        }
        visible
    }

    /// Completion-oriented visible bindings: skips closed inner blocks at EOF
    /// and includes declaration metadata (`name_span`, `scope_start`, `is_type`).
    pub(crate) fn visible_bindings_for_completion<'t>(
        &'t self,
        source: &'t str,
        offset: u32,
    ) -> Vec<(LocalId, &'t str, [u32; 2], u32, bool)> {
        let mut best_id = ScopeId(0);
        let mut best_len = u32::MAX;
        for (index, node) in self.nodes.iter().enumerate() {
            let contains = if node.kind != ScopeKind::FuncSignature
                && offset == node.extent[1]
                && node.extent[1] > node.extent[0]
                && source
                    .as_bytes()
                    .get(node.extent[1] as usize - 1)
                    .is_some_and(|b| *b == b'}')
            {
                false
            } else {
                node.extent[0] <= offset && offset <= node.extent[1]
            };
            if contains {
                let len = node.extent[1].saturating_sub(node.extent[0]);
                if len <= best_len {
                    best_len = len;
                    best_id = ScopeId(index as u32);
                }
            }
        }
        let mut visible: Vec<(LocalId, &'t str, [u32; 2], u32, bool)> = Vec::new();
        let mut current = Some(best_id);
        while let Some(scope_id) = current {
            let Some(node) = self.nodes.get(scope_id.0 as usize) else {
                break;
            };
            let mut binding_id = node.last_binding;
            while let Some(id) = binding_id {
                let binding = &self.bindings[id.0 as usize];
                if binding.visible_from <= offset
                    && let Some(name) = span_text(source, binding.name_span)
                    && !visible.iter().any(|&(_, seen, ..)| seen == name)
                {
                    let is_type = matches!(
                        binding.kind,
                        LocalBindingKind::TypeDecl { .. } | LocalBindingKind::TypeParam
                    );
                    visible.push((id, name, binding.name_span, node.extent[0], is_type));
                }
                binding_id = binding.prev_in_scope;
            }
            current = node.parent;
        }
        visible
    }
}

struct TreeBuilder<'a> {
    tree: &'a mut ScopeTree,
    source: &'a str,
    cursor_offset: Option<usize>,
}

impl TreeBuilder<'_> {
    fn walk_function_body(
        &mut self,
        fn_scope: ScopeId,
        base: usize,
        body: &BlockTree,
        depth: usize,
    ) {
        if let Some(root) = body.statement(body.root())
            && let StmtKind::Block { statements } = &root.kind
        {
            for &child in statements {
                self.walk_statement(fn_scope, base, body, child, 0, depth);
            }
        } else {
            self.walk_statement(fn_scope, base, body, body.root(), 0, depth);
        }
    }

    fn walk_top_level_stmt(
        &mut self,
        scope: ScopeId,
        base: usize,
        body: &BlockTree,
        stmt_id: StmtId,
        depth: usize,
    ) {
        if depth >= MAX_DEPTH {
            self.tree.complete = false;
            return;
        }
        let Some(stmt) = body.statement(stmt_id) else {
            return;
        };
        match &stmt.kind {
            StmtKind::Block { statements } => {
                for &child in statements {
                    self.walk_top_level_stmt(scope, base, body, child, depth + 1);
                }
            }
            StmtKind::DeclarationGroup { declarations } => {
                for &child in declarations {
                    self.walk_top_level_stmt(scope, base, body, child, depth + 1);
                }
            }
            StmtKind::Declaration { values, .. } => {
                for &value in values {
                    self.walk_expr(scope, base, body, value, depth + 1);
                }
            }
            _ => self.walk_statement(scope, base, body, stmt_id, 0, depth),
        }
    }

    fn walk_statement(
        &mut self,
        scope: ScopeId,
        base: usize,
        body: &BlockTree,
        stmt_id: StmtId,
        spec_index: u16,
        depth: usize,
    ) {
        if depth >= MAX_DEPTH {
            self.tree.complete = false;
            return;
        }
        let Some(stmt) = body.statement(stmt_id) else {
            return;
        };
        let own = to_abs_span(base, &stmt.span);
        match &stmt.kind {
            StmtKind::Block { statements } => {
                let Some(block_scope) = self.tree.push_scope(Some(scope), ScopeKind::Block, own)
                else {
                    return;
                };
                for &child in statements {
                    self.walk_statement(block_scope, base, body, child, 0, depth + 1);
                }
            }
            StmtKind::DeclarationGroup { declarations } => {
                for (ord, &child) in declarations.iter().enumerate() {
                    self.walk_statement(
                        scope,
                        base,
                        body,
                        child,
                        u16::try_from(ord).unwrap_or(u16::MAX),
                        depth + 1,
                    );
                }
            }
            StmtKind::Declaration {
                keyword,
                names,
                values,
                ..
            } => {
                let kw_text = self
                    .source
                    .get(base + keyword.start..base + keyword.end)
                    .unwrap_or("");
                if kw_text == "type"
                    && let Some(parameters) = body
                        .type_parameters()
                        .iter()
                        .find(|entry| entry.declaration == stmt_id)
                {
                    let tp_start = names.first().map_or(own[0], |name| {
                        u32::try_from(base + name.end).unwrap_or(own[0])
                    });
                    let Some(tp_scope) =
                        self.tree
                            .push_scope(Some(scope), ScopeKind::Block, [tp_start, own[1]])
                    else {
                        return;
                    };
                    for (index, parameter) in parameters.parameters.iter().enumerate() {
                        let Some(name) = &parameter.name else {
                            continue;
                        };
                        let abs = to_abs_span(base, name);
                        if span_text(self.source, abs) == Some("_") {
                            continue;
                        }
                        self.tree.push_binding(
                            tp_scope,
                            abs,
                            tp_start,
                            LocalBindingKind::TypeParam,
                            BindingInit::SignatureParam {
                                index: u16::try_from(index).unwrap_or(u16::MAX),
                            },
                        );
                    }
                }
                let is_type = kw_text == "type";
                let kind = match kw_text {
                    "type" => LocalBindingKind::TypeDecl {
                        alias: body.declaration_equals(stmt_id).is_some(),
                    },
                    "const" => LocalBindingKind::Const,
                    _ => LocalBindingKind::Var,
                };
                for (index, name) in names.iter().enumerate() {
                    let abs = to_abs_span(base, name);
                    if span_text(self.source, abs) == Some("_") {
                        continue;
                    }
                    let visible_from = if is_type { abs[0] } else { own[1] };
                    self.tree.push_binding(
                        scope,
                        abs,
                        visible_from,
                        kind,
                        BindingInit::Explicit {
                            decl_stmt: stmt_id,
                            spec_index,
                            arity_index: u16::try_from(index).unwrap_or(u16::MAX),
                        },
                    );
                }
                for &value in values {
                    self.walk_expr(scope, base, body, value, depth + 1);
                }
            }
            StmtKind::Assignment {
                left,
                operator,
                right,
            } => {
                for &expr in right {
                    self.walk_expr(scope, base, body, expr, depth + 1);
                }
                if self.source.get(base + operator.start..base + operator.end) == Some(":=") {
                    self.bind_short_vars(scope, base, body, stmt_id, left, own[1]);
                } else {
                    for &expr in left {
                        self.walk_expr(scope, base, body, expr, depth + 1);
                    }
                }
            }
            StmtKind::Expression { expression } => {
                self.walk_expr(scope, base, body, *expression, depth + 1);
            }
            StmtKind::Return { values } => {
                for &expr in values {
                    self.walk_expr(scope, base, body, expr, depth + 1);
                }
            }
            StmtKind::Increment { expression, .. } => {
                self.walk_expr(scope, base, body, *expression, depth + 1);
            }
            StmtKind::Send { channel, value } => {
                self.walk_expr(scope, base, body, *channel, depth + 1);
                self.walk_expr(scope, base, body, *value, depth + 1);
            }
            StmtKind::Go { call } | StmtKind::Defer { call } => {
                self.walk_expr(scope, base, body, *call, depth + 1);
            }
            StmtKind::If {
                init,
                condition,
                then_block,
                otherwise,
            } => {
                let Some(if_scope) = self.tree.push_scope(Some(scope), ScopeKind::If, own) else {
                    return;
                };
                if let Some(init) = init {
                    self.walk_statement(if_scope, base, body, *init, 0, depth + 1);
                }
                self.walk_expr(if_scope, base, body, *condition, depth + 1);
                self.walk_statement(if_scope, base, body, *then_block, 0, depth + 1);
                if let Some(otherwise) = otherwise {
                    self.walk_statement(if_scope, base, body, *otherwise, 0, depth + 1);
                }
            }
            StmtKind::For {
                init,
                condition,
                post,
                body: loop_body,
            } => {
                let Some(for_scope) = self.tree.push_scope(Some(scope), ScopeKind::For, own) else {
                    return;
                };
                if let Some(init) = init {
                    self.walk_statement(for_scope, base, body, *init, 0, depth + 1);
                }
                if let Some(condition) = condition {
                    self.walk_expr(for_scope, base, body, *condition, depth + 1);
                }
                if let Some(post) = post {
                    self.walk_statement(for_scope, base, body, *post, 0, depth + 1);
                }
                self.walk_statement(for_scope, base, body, *loop_body, 0, depth + 1);
            }
            StmtKind::RangeFor {
                clause,
                body: loop_body,
            } => {
                let Some(for_scope) = self.tree.push_scope(Some(scope), ScopeKind::For, own) else {
                    return;
                };
                self.walk_statement(for_scope, base, body, *clause, 0, depth + 1);
                self.walk_statement(for_scope, base, body, *loop_body, 0, depth + 1);
            }
            StmtKind::RangeClause {
                left,
                operator,
                value,
            } => {
                self.walk_expr(scope, base, body, *value, depth + 1);
                if operator
                    .as_ref()
                    .and_then(|op| self.source.get(base + op.start..base + op.end))
                    == Some(":=")
                {
                    for (index, expr_id) in left.iter().enumerate() {
                        let Some(expr) = body.expression(*expr_id) else {
                            continue;
                        };
                        if !matches!(expr.kind, ExprKind::Name) {
                            continue;
                        }
                        let abs = to_abs_span(base, &expr.span);
                        let Some(name) = span_text(self.source, abs) else {
                            continue;
                        };
                        if name == "_" {
                            self.tree.range_blanks.push(abs);
                            continue;
                        }
                        if self.tree.has_in_scope(self.source, scope, name) {
                            continue;
                        }
                        self.tree.push_binding(
                            scope,
                            abs,
                            own[1],
                            if index == 0 {
                                LocalBindingKind::RangeKey
                            } else {
                                LocalBindingKind::RangeVal
                            },
                            BindingInit::Range {
                                clause: stmt_id,
                                is_value: index > 0,
                            },
                        );
                    }
                }
            }
            StmtKind::Switch { init, tag, cases } => {
                let Some(switch_scope) = self.tree.push_scope(Some(scope), ScopeKind::Switch, own)
                else {
                    return;
                };
                if let Some(init) = init {
                    self.walk_statement(switch_scope, base, body, *init, 0, depth + 1);
                }
                if let Some(tag) = tag {
                    self.walk_expr(switch_scope, base, body, *tag, depth + 1);
                }
                for &case in cases {
                    self.walk_case_clause(switch_scope, base, body, case, None, depth + 1);
                }
            }
            StmtKind::TypeSwitch { init, guard, cases } => {
                let Some(ts_scope) = self
                    .tree
                    .push_scope(Some(scope), ScopeKind::TypeSwitch, own)
                else {
                    return;
                };
                if let Some(init) = init {
                    self.walk_statement(ts_scope, base, body, *init, 0, depth + 1);
                }
                let guard_binding =
                    self.extract_type_switch_guard(ts_scope, base, body, *guard, depth + 1);
                for &case in cases {
                    self.walk_case_clause(ts_scope, base, body, case, guard_binding, depth + 1);
                }
            }
            StmtKind::Case { .. } => {
                self.walk_case_clause(scope, base, body, stmt_id, None, depth + 1);
            }
            StmtKind::Select { cases } => {
                for &case in cases {
                    self.walk_statement(scope, base, body, case, 0, depth + 1);
                }
            }
            StmtKind::CommunicationCase {
                communication,
                statements,
            } => {
                let Some(comm_scope) =
                    self.tree
                        .push_scope(Some(scope), ScopeKind::CommClause, own)
                else {
                    return;
                };
                if let Some(comm_id) = communication
                    && let Some(comm_stmt) = body.statement(*comm_id)
                {
                    let visible = body
                        .clause_body_start(stmt_id)
                        .map_or(own[1], |start| (base + start) as u32);
                    if let StmtKind::Assignment {
                        left,
                        operator,
                        right,
                    } = &comm_stmt.kind
                        && self.source.get(base + operator.start..base + operator.end) == Some(":=")
                    {
                        for &expr in right {
                            self.walk_expr(comm_scope, base, body, expr, depth + 1);
                        }
                        if right.len() == 1
                            && body.is_receive(right[0], |span| {
                                self.source.get(base + span.start..base + span.end) == Some("<-")
                            })
                        {
                            self.bind_short_vars(comm_scope, base, body, *comm_id, left, visible);
                        }
                    } else {
                        self.walk_statement(comm_scope, base, body, *comm_id, 0, depth + 1);
                    }
                }
                for &child in statements {
                    self.walk_statement(comm_scope, base, body, child, 0, depth + 1);
                }
            }
            StmtKind::Label { name, statement } => {
                let abs = to_abs_span(base, name);
                if span_text(self.source, abs).is_some_and(|text| text != "_")
                    && self.tree.labels.len() < MAX_SCOPE_NODES
                {
                    self.tree.labels.push(abs);
                }
                if let Some(child) = statement {
                    self.walk_statement(scope, base, body, *child, 0, depth + 1);
                }
            }
            StmtKind::Branch { .. } | StmtKind::Missing => {}
        }
    }

    fn bind_short_vars(
        &mut self,
        scope: ScopeId,
        base: usize,
        body: &BlockTree,
        stmt_id: StmtId,
        left: &[ExprId],
        visible_from: u32,
    ) {
        for (index, expr_id) in left.iter().enumerate() {
            let Some(expr) = body.expression(*expr_id) else {
                continue;
            };
            if !matches!(expr.kind, ExprKind::Name) {
                continue;
            }
            let abs = to_abs_span(base, &expr.span);
            let Some(name) = span_text(self.source, abs) else {
                continue;
            };
            if name == "_" || self.tree.has_in_scope(self.source, scope, name) {
                continue;
            }
            self.tree.push_binding(
                scope,
                abs,
                visible_from,
                LocalBindingKind::ShortVar,
                BindingInit::ShortDefine {
                    stmt: stmt_id,
                    lhs_index: u16::try_from(index).unwrap_or(u16::MAX),
                },
            );
        }
    }

    fn extract_type_switch_guard(
        &mut self,
        scope: ScopeId,
        base: usize,
        body: &BlockTree,
        guard: StmtId,
        depth: usize,
    ) -> Option<(StmtId, [u32; 2])> {
        let stmt = body.statement(guard)?;
        match &stmt.kind {
            StmtKind::Assignment {
                left,
                operator,
                right,
            } => {
                for &expr in right {
                    self.walk_expr(scope, base, body, expr, depth + 1);
                }
                if left.len() == 1
                    && right.len() == 1
                    && self.source.get(base + operator.start..base + operator.end) == Some(":=")
                    && let Some(lhs) = body.expression(left[0])
                    && matches!(lhs.kind, ExprKind::Name)
                    && let Some(rhs) = body.expression(right[0])
                    && matches!(rhs.kind, ExprKind::TypeSwitchGuard { .. })
                {
                    let abs = to_abs_span(base, &lhs.span);
                    if span_text(self.source, abs).is_some_and(|name| name != "_") {
                        return Some((guard, abs));
                    }
                }
                None
            }
            StmtKind::Expression { expression } => {
                self.walk_expr(scope, base, body, *expression, depth + 1);
                None
            }
            _ => None,
        }
    }

    fn walk_case_clause(
        &mut self,
        parent: ScopeId,
        base: usize,
        body: &BlockTree,
        case_id: StmtId,
        guard_binding: Option<(StmtId, [u32; 2])>,
        depth: usize,
    ) {
        let Some(stmt) = body.statement(case_id) else {
            return;
        };
        let own = to_abs_span(base, &stmt.span);
        let StmtKind::Case {
            values, statements, ..
        } = &stmt.kind
        else {
            return;
        };
        let Some(case_scope) = self
            .tree
            .push_scope(Some(parent), ScopeKind::CaseClause, own)
        else {
            return;
        };
        for &value in values {
            self.walk_expr(parent, base, body, value, depth + 1);
        }
        if let Some((guard_stmt, guard_span)) = guard_binding {
            let visible = body
                .clause_body_start(case_id)
                .map_or(own[1], |start| (base + start) as u32);
            self.tree.push_binding(
                case_scope,
                guard_span,
                visible,
                LocalBindingKind::TypeSwitchVar,
                BindingInit::TypeSwitch {
                    guard_stmt,
                    case_stmt: case_id,
                },
            );
        }
        for &child in statements {
            self.walk_statement(case_scope, base, body, child, 0, depth + 1);
        }
    }

    fn walk_expr(
        &mut self,
        scope: ScopeId,
        base: usize,
        body: &BlockTree,
        expr_id: ExprId,
        depth: usize,
    ) {
        if depth >= MAX_DEPTH {
            self.tree.complete = false;
            return;
        }
        let Some(expr) = body.expression(expr_id) else {
            return;
        };
        self.walk_expr_node(scope, base, body, expr, depth);
    }

    fn walk_expr_node(
        &mut self,
        scope: ScopeId,
        base: usize,
        body: &BlockTree,
        expr: &Expr,
        depth: usize,
    ) {
        match &expr.kind {
            ExprKind::FunctionLiteral {
                signature,
                body: lit_body,
            } => {
                let sig_start = base + signature.start;
                let sig_end = base + signature.end;
                let body_start = base + lit_body.start;
                let body_end = base + lit_body.end;
                if self.tree.nodes.iter().any(|node| {
                    node.kind == ScopeKind::FuncSignature
                        && node.extent == [sig_start as u32, body_end as u32]
                }) {
                    return;
                }
                if body_end <= self.source.len()
                    && sig_end.saturating_sub(sig_start) <= MAX_BYTES
                    && body_end.saturating_sub(body_start)
                        <= crate::analysis::engine::budget::MAX_BODY_BYTES
                    && self
                        .cursor_offset
                        .is_none_or(|cursor| sig_start <= cursor && cursor <= body_end)
                    && let Some(sig_text) = self.source.get(sig_start..sig_end)
                    && let Some(body_text) = self.source.get(body_start..body_end)
                {
                    let parsed_sig = crate::syntax::parse_literal_signature(sig_text);
                    let parsed_body = crate::syntax::parse_block(body_text);
                    self.tree.complete &=
                        parsed_sig.diagnostics().is_empty() && parsed_body.diagnostics().is_empty();
                    if let Some(lit_scope) = self.tree.push_scope(
                        Some(scope),
                        ScopeKind::FuncSignature,
                        [
                            u32::try_from(sig_start).unwrap_or(0),
                            u32::try_from(body_end).unwrap_or(0),
                        ],
                    ) {
                        self.tree.populate_signature(
                            self.source,
                            lit_scope,
                            sig_start,
                            body_start,
                            &parsed_sig,
                        );
                        self.walk_function_body(lit_scope, body_start, &parsed_body, depth + 1);
                    }
                }
            }
            ExprKind::Composite {
                literal_type,
                elements,
            } => {
                if let Some(ty) = literal_type {
                    self.walk_expr(scope, base, body, *ty, depth + 1);
                }
                for &elem in elements {
                    self.walk_expr(scope, base, body, elem, depth + 1);
                }
            }
            ExprKind::KeyValue { key, value } => {
                self.walk_expr(scope, base, body, *key, depth + 1);
                self.walk_expr(scope, base, body, *value, depth + 1);
            }
            ExprKind::Unary { operand, .. }
            | ExprKind::Parenthesized {
                expression: operand,
            }
            | ExprKind::Selector { operand, .. }
            | ExprKind::TypeAssertion { operand, .. }
            | ExprKind::TypeSwitchGuard { operand } => {
                self.walk_expr(scope, base, body, *operand, depth + 1);
            }
            ExprKind::Binary { left, right, .. } => {
                self.walk_expr(scope, base, body, *left, depth + 1);
                self.walk_expr(scope, base, body, *right, depth + 1);
            }
            ExprKind::Call {
                function,
                arguments,
                ..
            } => {
                self.walk_expr(scope, base, body, *function, depth + 1);
                for &arg in arguments {
                    self.walk_expr(scope, base, body, arg, depth + 1);
                }
            }
            ExprKind::Index { operand, arguments } => {
                self.walk_expr(scope, base, body, *operand, depth + 1);
                for &arg in arguments {
                    self.walk_expr(scope, base, body, arg, depth + 1);
                }
            }
            ExprKind::Slice {
                operand,
                low,
                high,
                max,
                ..
            } => {
                self.walk_expr(scope, base, body, *operand, depth + 1);
                for sub in [*low, *high, *max].into_iter().flatten() {
                    self.walk_expr(scope, base, body, sub, depth + 1);
                }
            }
            ExprKind::Name | ExprKind::Literal(_) | ExprKind::Missing | ExprKind::Type => {}
        }
    }
}

fn to_abs_span(base: usize, range: &Range<usize>) -> [u32; 2] {
    [
        u32::try_from(base.saturating_add(range.start)).unwrap_or(u32::MAX),
        u32::try_from(base.saturating_add(range.end)).unwrap_or(u32::MAX),
    ]
}

fn span_text(source: &str, span: [u32; 2]) -> Option<&str> {
    source.get(span[0] as usize..span[1] as usize)
}
