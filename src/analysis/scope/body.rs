//! Function-body scopes from the handwritten statement arena.
use crate::analysis::scope::{
    self as locals, Binding, DeclarationKind, Initializer, InitializerKind, Locals, ObjectKind,
    SwitchCase,
};
use crate::syntax::{BlockTree, ExprKind, Lexer, StmtId, StmtKind, TokenKind};
use std::ops::Range;

impl Locals {
    pub(crate) fn from_body(source: &str, body_offset: usize, tree: &BlockTree) -> Self {
        let mut locals = Self::from_signature(source, body_offset);
        locals.append_body(source, body_offset, tree);
        locals
    }

    pub(crate) fn append_body(&mut self, source: &str, offset: usize, tree: &BlockTree) {
        BodyScopes {
            locals: self,
            source,
            offset,
            tree,
            range_types: true,
        }
        .statement(tree.root, offset..source.len());
    }

    pub(crate) fn append_qualified_body(&mut self, source: &str, offset: usize, tree: &BlockTree) {
        BodyScopes {
            locals: self,
            source,
            offset,
            tree,
            range_types: false,
        }
        .statement(tree.root, offset..source.len());
    }
}

struct BodyScopes<'a> {
    locals: &'a mut Locals,
    source: &'a str,
    offset: usize,
    tree: &'a BlockTree,
    range_types: bool,
}

impl BodyScopes<'_> {
    fn span(&self, range: &Range<usize>) -> Range<usize> {
        self.offset + range.start..self.offset + range.end
    }

    fn statement(&mut self, id: StmtId, scope: Range<usize>) {
        let statement = &self.tree.statements[id.0];
        let own = self.span(&statement.span);
        match &statement.kind {
            StmtKind::Block { statements } | StmtKind::Case { statements, .. } => {
                for &child in statements {
                    self.statement(child, own.clone());
                }
            }
            StmtKind::DeclarationGroup { declarations } => {
                for &child in declarations {
                    self.statement(child, scope.clone());
                }
            }
            StmtKind::Declaration {
                keyword,
                names,
                annotation,
                values,
            } => {
                if let Some(parameters) = self
                    .tree
                    .type_parameters
                    .iter()
                    .find(|entry| entry.declaration == id)
                {
                    let start = names
                        .first()
                        .map_or(own.start, |name| self.offset + name.end);
                    for parameter in &parameters.parameters {
                        let Some(name) = &parameter.name else {
                            continue;
                        };
                        let selection = self.span(name);
                        if &self.source[selection.clone()] == "_" {
                            continue;
                        }
                        self.locals.bindings.push(Binding {
                            name: self.source[selection.clone()].to_owned(),
                            selection,
                            scope: start..own.end,
                            visible_from: start,
                            object_kind: ObjectKind::TypeParameter,
                            declaration: DeclarationKind::TypeParameter,
                            declared_type: None,
                            annotation: None,
                            initializer: None,
                        });
                    }
                }
                let type_declaration = &self.source[self.span(keyword)] == "type";
                let declaration = match &self.source[self.span(keyword)] {
                    "type" => DeclarationKind::Type,
                    "const" => DeclarationKind::Constant,
                    _ => DeclarationKind::Value,
                };
                let declared_type = annotation.as_ref().and_then(|span| {
                    let annotation = self.span(span);
                    let ty = locals::named_type(&self.source[annotation.clone()])?;
                    let qualifier = locals::named_root(&ty).split('.').next()?;
                    self.locals
                        .resolve_at(annotation.start, qualifier)
                        .is_none()
                        .then_some(ty)
                });
                for (index, name) in names.iter().enumerate() {
                    let selection = self.span(name);
                    let visible = if type_declaration {
                        selection.start
                    } else {
                        own.end
                    };
                    let initializer = (!type_declaration)
                        .then(|| self.initializer(values, index, names.len()))
                        .flatten();
                    let annotation = annotation.as_ref().map(|span| Box::new(self.span(span)));
                    if let Some(binding) = self.bind(
                        selection,
                        scope.clone(),
                        visible,
                        if type_declaration {
                            ObjectKind::Type
                        } else {
                            ObjectKind::Variable
                        },
                        if type_declaration {
                            None
                        } else {
                            declared_type.clone()
                        },
                        false,
                    ) {
                        binding.initializer = initializer;
                        binding.annotation = annotation;
                        binding.declaration = declaration;
                    }
                }
            }
            StmtKind::Assignment {
                left,
                operator,
                right,
            } => {
                if &self.source[self.span(operator)] == ":=" {
                    self.names(left, right, scope, own.end);
                }
            }
            StmtKind::RangeClause {
                left,
                operator,
                value,
            } => {
                if operator
                    .as_ref()
                    .is_some_and(|operator| &self.source[self.span(operator)] == ":=")
                {
                    if !self.range_types {
                        self.names(left, &[], scope, own.end);
                        return;
                    }
                    for (index, name) in left.iter().enumerate() {
                        let name = &self.tree.expressions[name.0];
                        if !matches!(name.kind, ExprKind::Name) {
                            continue;
                        }
                        let initializer = Box::new(Initializer {
                            span: self.span(&self.tree.expressions[value.0].span),
                            result: index,
                            count: left.len(),
                            kind: InitializerKind::Range,
                        });
                        if &self.source[self.span(&name.span)] == "_" {
                            self.locals.bindings.push(Binding {
                                name: "_".to_owned(),
                                selection: self.span(&name.span),
                                scope: scope.clone(),
                                visible_from: own.end,
                                object_kind: ObjectKind::Variable,
                                declaration: DeclarationKind::Value,
                                declared_type: None,
                                annotation: None,
                                initializer: Some(initializer),
                            });
                            continue;
                        }
                        if let Some(binding) = self.bind(
                            self.span(&name.span),
                            scope.clone(),
                            own.end,
                            ObjectKind::Variable,
                            None,
                            false,
                        ) {
                            binding.initializer = Some(initializer);
                        }
                    }
                }
            }
            StmtKind::If {
                init,
                then_block,
                otherwise,
                ..
            } => {
                for &child in init
                    .iter()
                    .chain(std::iter::once(then_block))
                    .chain(otherwise.iter())
                {
                    self.statement(child, own.clone());
                }
            }
            StmtKind::For {
                init, post, body, ..
            } => {
                for &child in init.iter().chain(post.iter()).chain(std::iter::once(body)) {
                    self.statement(child, own.clone());
                }
            }
            StmtKind::RangeFor { clause, body } => {
                self.statement(*clause, own.clone());
                self.statement(*body, own);
            }
            StmtKind::Switch { init, cases, .. } => {
                for &child in init.iter().chain(cases) {
                    self.statement(child, own.clone());
                }
            }
            StmtKind::TypeSwitch { init, guard, cases } => {
                if let Some(init) = init {
                    self.statement(*init, own.clone());
                }
                for &case in cases {
                    let case_scope = self.span(&self.tree.statements[case.0].span);
                    if let StmtKind::Case { values, .. } = &self.tree.statements[case.0].kind {
                        for value in values {
                            let span = self.span(&self.tree.expressions[value.0].span);
                            if span.len() <= 16 * 1024 {
                                let parsed = crate::syntax::parse_type(&self.source[span.clone()]);
                                if parsed.diagnostics.is_empty() {
                                    self.locals.type_signature_names(
                                        self.source,
                                        &parsed.signature_names,
                                        span.start,
                                    );
                                }
                            }
                        }
                    }
                    self.switch_binding(*guard, case, cases.len(), case_scope.clone());
                    self.statement(case, case_scope);
                }
            }
            StmtKind::Select { cases } => {
                for &case in cases {
                    self.statement(case, own.clone());
                }
            }
            StmtKind::CommunicationCase {
                communication,
                statements,
            } => {
                if let Some(id) = communication {
                    let statement = &self.tree.statements[id.0];
                    if let StmtKind::Assignment {
                        left,
                        right,
                        operator,
                    } = &statement.kind
                        && &self.source[self.span(operator)] == ":="
                    {
                        let received = right.len() == 1 && self.receive_expression(right[0]);
                        let visible =
                            self.clause_body_start(Some(self.offset + statement.span.end), own.end);
                        if received {
                            self.names(left, right, own.clone(), visible);
                        }
                    } else {
                        self.statement(*id, own.clone());
                    }
                }
                for &child in statements {
                    self.statement(child, own.clone());
                }
            }
            StmtKind::Label {
                statement: Some(child),
                ..
            } => self.statement(*child, scope),
            _ => {}
        }
    }

    fn switch_binding(&mut self, guard: StmtId, case: StmtId, clauses: usize, scope: Range<usize>) {
        let StmtKind::Assignment {
            left,
            right,
            operator,
        } = &self.tree.statements[guard.0].kind
        else {
            return;
        };
        if left.len() != 1 || right.len() != 1 || &self.source[self.span(operator)] != ":=" {
            return;
        }
        let name = &self.tree.expressions[left[0].0];
        let ExprKind::TypeSwitchGuard { operand } = self.tree.expressions[right[0].0].kind else {
            return;
        };
        let StmtKind::Case {
            values, default, ..
        } = &self.tree.statements[case.0].kind
        else {
            return;
        };
        if !matches!(name.kind, ExprKind::Name) {
            return;
        }
        let selection = self.span(&name.span);
        // The implicit variable starts in the clause body, never in its types.
        let header_end = values.last().map_or_else(
            || {
                Lexer::new(&self.source[scope.clone()])
                    .next()
                    .map(|token| scope.start + token.span.end)
            },
            |value| Some(self.offset + self.tree.expressions[value.0].span.end),
        );
        let visible = self.clause_body_start(header_end, scope.end);
        if self
            .bind(
                selection.clone(),
                scope.clone(),
                visible,
                ObjectKind::Variable,
                None,
                true,
            )
            .is_none()
        {
            return;
        }
        if self.locals.switch_cases.len() >= 128
            || self
                .locals
                .switch_cases
                .iter()
                .map(|case| case.types.len())
                .sum::<usize>()
                + values.len()
                > 256
        {
            return;
        }
        self.locals.switch_cases.push(SwitchCase {
            selection: selection.start,
            scope_start: scope.start,
            operand: self.span(&self.tree.expressions[operand.0].span),
            clauses,
            default: *default,
            types: values
                .iter()
                .map(|id| self.span(&self.tree.expressions[id.0].span))
                .collect(),
        });
    }

    fn clause_body_start(&self, header_end: Option<usize>, end: usize) -> usize {
        header_end
            .and_then(|start| {
                let colon = Lexer::new(self.source.get(start..end)?).next()?;
                (colon.kind == TokenKind::Colon).then_some(start + colon.span.end)
            })
            .unwrap_or(end)
    }

    fn receive_expression(&self, mut id: crate::syntax::ExprId) -> bool {
        for _ in 0..32 {
            match &self.tree.expressions[id.0].kind {
                ExprKind::Parenthesized { expression } => id = *expression,
                ExprKind::Unary { operator, .. } => {
                    return &self.source[self.span(operator)] == "<-";
                }
                _ => return false,
            }
        }
        false
    }

    fn initializer(
        &self,
        values: &[crate::syntax::ExprId],
        index: usize,
        count: usize,
    ) -> Option<Box<Initializer>> {
        let (value, result, count) = if values.len() == count {
            (*values.get(index)?, 0, 1)
        } else if values.len() == 1 {
            (values[0], index, count)
        } else {
            return None;
        };
        Some(Box::new(Initializer {
            span: self.span(&self.tree.expressions[value.0].span),
            result,
            count,
            kind: InitializerKind::Value,
        }))
    }

    fn names(
        &mut self,
        names: &[crate::syntax::ExprId],
        values: &[crate::syntax::ExprId],
        scope: Range<usize>,
        visible: usize,
    ) {
        for (index, name) in names.iter().enumerate() {
            let name = &self.tree.expressions[name.0];
            if matches!(name.kind, ExprKind::Name) {
                let initializer = self.initializer(values, index, names.len());
                if let Some(binding) = self.bind(
                    self.span(&name.span),
                    scope.clone(),
                    visible,
                    ObjectKind::Variable,
                    None,
                    true,
                ) {
                    binding.initializer = initializer;
                }
            }
        }
    }

    fn bind(
        &mut self,
        selection: Range<usize>,
        scope: Range<usize>,
        visible_from: usize,
        object_kind: ObjectKind,
        declared_type: Option<String>,
        reuse: bool,
    ) -> Option<&mut Binding> {
        let name = &self.source[selection.clone()];
        if name == "_"
            || reuse
                && self
                    .locals
                    .bindings
                    .iter()
                    .any(|b| b.name == name && b.scope == scope)
        {
            return None;
        }
        self.locals.bindings.push(Binding {
            name: name.to_owned(),
            selection,
            scope,
            visible_from,
            object_kind,
            declaration: if object_kind.is_type() {
                DeclarationKind::Type
            } else {
                DeclarationKind::Value
            },
            declared_type,
            annotation: None,
            initializer: None,
        });
        self.locals.bindings.last_mut()
    }
}
