//! Classifies semantic occurrences without keeping the temporary body arenas.
use crate::analysis::scope::ObjectKind;
use crate::analysis::source::{Role, SourceAnalysis};
use crate::syntax::{
    BlockTree, Expr, ExprKind, Lexer, Signature, SignatureName, SignatureNameKind, StmtKind,
    TokenKind, TypeReference,
};
use std::ops::Range;

impl SourceAnalysis {
    pub(super) fn mark(&mut self, span: Range<usize>, role: Role) {
        if let Ok(index) = self
            .names
            .binary_search_by_key(&span.start, |name| name.span.start)
            && self.names[index].span == span
            && (!matches!(self.names[index].role, Role::BlankDeclaration { .. })
                || !matches!(role, Role::Name))
        {
            self.names[index].role = role;
        }
    }

    pub(super) fn embedded_type(&mut self, span: Range<usize>) {
        if let Ok(index) = self
            .names
            .binary_search_by_key(&span.start, |name| name.span.start)
            && self.names[index].span == span
        {
            let qualifier = match &self.names[index].role {
                Role::Type(qualifier) | Role::EmbeddedType(qualifier) => qualifier.clone(),
                _ => None,
            };
            self.names[index].role = Role::EmbeddedType(qualifier);
        }
    }

    pub(super) fn types(&mut self, references: &[TypeReference], base: usize) {
        for reference in references {
            let qualifier = reference
                .qualifier
                .as_ref()
                .map(|span| base + span.start..base + span.end);
            if let Some(qualifier) = &qualifier {
                self.mark(qualifier.clone(), Role::Namespace);
            }
            self.mark(
                base + reference.name.start..base + reference.name.end,
                Role::Type(qualifier),
            );
        }
    }

    pub(super) fn aggregate_names(&mut self, span: Range<usize>) {
        let start = self
            .names
            .partition_point(|name| name.span.start < span.start);
        for name in self.names[start..]
            .iter_mut()
            .take_while(|name| name.span.end <= span.end)
        {
            name.role = Role::Field;
        }
    }

    pub(super) fn signature_names(&mut self, names: &[SignatureName], base: usize) {
        for name in names {
            if let Some(entries) = &mut self.signature_types {
                entries.push((
                    base + name.name.start,
                    base + name.annotation.start..base + name.annotation.end,
                    name.variadic,
                ));
            }
            let role = match name.kind {
                SignatureNameKind::Parameter => Role::Parameter,
                SignatureNameKind::Result => Role::Result,
            };
            self.mark(base + name.name.start..base + name.name.end, role);
        }
    }

    pub(super) fn expressions(
        &mut self,
        source: &str,
        expressions: &[Expr],
        base: usize,
        literals: &mut Vec<(Range<usize>, Range<usize>, usize)>,
    ) {
        for expression in expressions {
            match &expression.kind {
                ExprKind::Type => {
                    self.annotate_type(
                        source,
                        base + expression.span.start..base + expression.span.end,
                        literals,
                    );
                }
                ExprKind::Name => self.mark(
                    base + expression.span.start..base + expression.span.end,
                    Role::Name,
                ),
                ExprKind::Selector { operand, field } => {
                    let original = &expressions[operand.0].span;
                    let mut operand = &expressions[operand.0];
                    while let ExprKind::Parenthesized { expression } = operand.kind {
                        operand = &expressions[expression.0];
                    }
                    let qualifier = matches!(operand.kind, ExprKind::Name)
                        .then(|| base + operand.span.start..base + operand.span.end);
                    let role = qualifier.map_or_else(
                        || Role::ExpressionMember(base + original.start..base + original.end),
                        |qualifier| Role::Member(Some(qualifier)),
                    );
                    self.mark(base + field.start..base + field.end, role);
                }
                ExprKind::FunctionLiteral { signature, body } => {
                    let literal = (
                        base + signature.start..base + signature.end,
                        base + body.start..base + body.end,
                        0,
                    );
                    if !literals
                        .iter()
                        .any(|(_, existing, _)| *existing == literal.1)
                    {
                        literals.push(literal);
                    }
                }
                ExprKind::Composite {
                    literal_type: Some(ty),
                    elements,
                } => {
                    // Named types need type-directed field/expression lookup.
                    // An anonymous struct's key is a field label too; map and
                    // array syntax keeps ordinary key-expression name roles.
                    let annotation = &expressions[ty.0];
                    let anonymous_struct = matches!(annotation.kind, ExprKind::Type)
                        && source
                            .get(base + annotation.span.start..base + annotation.span.end)
                            .and_then(|text| Lexer::new(text).next())
                            .is_some_and(|token| token.kind == TokenKind::Struct);
                    if matches!(
                        expressions[ty.0].kind,
                        ExprKind::Name | ExprKind::Selector { .. } | ExprKind::Index { .. }
                    ) || anonymous_struct
                    {
                        for element in elements {
                            if let ExprKind::KeyValue { key, .. } = &expressions[element.0].kind
                                && matches!(expressions[key.0].kind, ExprKind::Name)
                            {
                                let key = &expressions[key.0].span;
                                let ty = &expressions[ty.0].span;
                                self.mark(
                                    base + key.start..base + key.end,
                                    Role::LiteralKey(base + ty.start..base + ty.end),
                                );
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        self.literal_elements(expressions, base);
    }
    pub(super) fn header(
        &mut self,
        source: &str,
        signature: &Signature,
        base: usize,
        literals: &mut Vec<(Range<usize>, Range<usize>, usize)>,
    ) {
        for parameter in signature
            .receiver
            .iter()
            .chain(&signature.parameters)
            .chain(&signature.results)
            .chain(&signature.type_parameters)
        {
            self.annotate_type(
                source,
                base + parameter.annotation.start..base + parameter.annotation.end,
                literals,
            );
        }
        self.types(&signature.type_references, base);
        // Signature declarations are not ordinary identifier uses. In particular,
        // nested function types may have marked their spans as aggregate names.
        let declarations = signature
            .receiver
            .iter()
            .chain(&signature.parameters)
            .map(|parameter| (parameter, Role::Parameter))
            .chain(
                signature
                    .results
                    .iter()
                    .map(|parameter| (parameter, Role::Result)),
            );
        for (parameter, role) in declarations {
            if let Some(name) = &parameter.name {
                self.mark(base + name.start..base + name.end, role);
            }
        }
    }

    pub(super) fn block(
        &mut self,
        source: &str,
        block: &BlockTree,
        base: usize,
        literals: &mut Vec<(Range<usize>, Range<usize>, usize)>,
    ) {
        for entry in &block.type_parameters {
            for parameter in &entry.parameters {
                if let Some(name) = &parameter.name {
                    self.mark(base + name.start..base + name.end, Role::Name);
                }
            }
        }
        for statement in &block.statements {
            if let StmtKind::Declaration { names, keyword, .. } = &statement.kind {
                let keyword = source.get(base + keyword.start..base + keyword.end);
                for name in names {
                    self.mark(
                        base + name.start..base + name.end,
                        if source.get(base + name.start..base + name.end) == Some("_") {
                            Role::BlankDeclaration {
                                kind: if keyword == Some("type") {
                                    ObjectKind::Type
                                } else {
                                    ObjectKind::Variable
                                },
                                constant: keyword == Some("const"),
                            }
                        } else {
                            Role::Name
                        },
                    );
                }
            }
            if let StmtKind::Assignment { left, operator, .. } = &statement.kind
                && source.get(base + operator.start..base + operator.end) == Some(":=")
            {
                for id in left {
                    let expression = &block.expressions[id.0];
                    if matches!(expression.kind, ExprKind::Name)
                        && source.get(base + expression.span.start..base + expression.span.end)
                            == Some("_")
                    {
                        self.mark(
                            base + expression.span.start..base + expression.span.end,
                            Role::BlankDeclaration {
                                kind: ObjectKind::Variable,
                                constant: false,
                            },
                        );
                    }
                }
            }
            if matches!(statement.kind, StmtKind::Declaration { .. })
                && let Some(positions) = &mut self.declaration_operators
                && let Some(offset) = super::build::declaration_assignment(
                    &source[base + statement.span.start..base + statement.span.end],
                )
            {
                positions.push(base + statement.span.start + offset);
            }

            match &statement.kind {
                StmtKind::TypeSwitch { cases, .. } => {
                    for case in cases {
                        if let StmtKind::Case { values, .. } = &block.statements[case.0].kind {
                            for value in values {
                                let span = &block.expressions[value.0].span;
                                if span.len() <= 16 * 1024 {
                                    self.annotate_type(
                                        source,
                                        base + span.start..base + span.end,
                                        literals,
                                    );
                                }
                            }
                        }
                    }
                }
                StmtKind::RangeClause {
                    operator: Some(operator),
                    ..
                } => {
                    if let Some(positions) = &mut self.declaration_operators {
                        positions.push(base + operator.start);
                    }
                }
                StmtKind::Declaration {
                    annotation: Some(annotation),
                    ..
                } => {
                    self.annotate_type(
                        source,
                        base + annotation.start..base + annotation.end,
                        literals,
                    );
                }
                StmtKind::Label { name, .. }
                | StmtKind::Branch {
                    label: Some(name), ..
                } => {
                    self.mark(base + name.start..base + name.end, Role::Ignored);
                }
                _ => {}
            }
        }
        self.types(&block.type_references, base);
    }

    pub(super) fn annotate_type(
        &mut self,
        source: &str,
        span: Range<usize>,
        literals: &mut Vec<(Range<usize>, Range<usize>, usize)>,
    ) {
        let parsed = crate::syntax::parse_type(&source[span.clone()]);
        if parsed.diagnostics.is_empty() {
            self.aggregate_names(span.clone());
            self.types(&parsed.references, span.start);
            // Embedded aggregate fields are implicit field declarations: semantic
            // tokens style the identifier as the field, not as a type reference.
            self.aggregate_embedded(source, &parsed, span.start);
            self.expressions(source, &parsed.expressions, span.start, literals);
            self.signature_names(&parsed.signature_names, span.start);
        }
    }

    pub(super) fn aggregate_embedded(
        &mut self,
        source: &str,
        parsed: &crate::syntax::TypeSyntax,
        base: usize,
    ) {
        for field in &parsed.fields {
            if !field.embedded {
                continue;
            }
            let Some(text) = source.get(base + field.annotation.start..base + field.annotation.end)
            else {
                continue;
            };
            let mut tokens = Vec::new();
            for token in Lexer::new(text) {
                if token.kind == TokenKind::LBrack {
                    break;
                }
                tokens.push(token);
            }
            let Some(last_index) = tokens
                .iter()
                .rposition(|token| token.kind == TokenKind::Ident)
            else {
                continue;
            };
            let last = tokens[last_index].span.clone();
            let selection = base + field.annotation.start + last.start
                ..base + field.annotation.start + last.end;
            let qualifier = (last_index >= 2
                && tokens[last_index - 1].kind == TokenKind::Period
                && tokens[last_index - 2].kind == TokenKind::Ident)
                .then(|| {
                    let qualifier = tokens[last_index - 2].span.clone();
                    base + field.annotation.start + qualifier.start
                        ..base + field.annotation.start + qualifier.end
                });
            self.mark(selection, Role::EmbeddedType(qualifier));
        }
    }
}
