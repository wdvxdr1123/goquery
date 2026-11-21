//! Streaming syntax-role classification for semantic tokens without `Locals`.
use crate::index::IndexedFile;
use crate::syntax::{
    BlockTree, Expr, ExprId, ExprKind, Lexer, Signature, SignatureName, SignatureNameKind,
    StmtKind, TokenKind, TypeReference,
};
use crate::text::SourceMap;
use crate::workspace::Workspace;
use std::collections::HashMap;
use std::ops::Range;
use std::path::Path;

#[derive(Clone)]
pub(super) enum TokenRole {
    Name,
    Type(Option<Range<usize>>),
    EmbeddedType(Option<Range<usize>>),
    Member(Option<Range<usize>>),
    ExpressionMember(Range<usize>),
    Field,
    FieldDeclaration(usize),
    LiteralKey(Range<usize>),
    LiteralElementKey(usize),
    BlankDeclaration { is_type: bool, constant: bool },
    Parameter,
    Result,
    Namespace,
    Ignored,
}

#[derive(Clone)]
pub(super) struct TokenName {
    pub(super) span: Range<usize>,
    pub(super) role: TokenRole,
}

enum LiteralType {
    Explicit(Range<usize>),
    Element { container: usize, key: bool },
}

pub(super) struct TokenLayout {
    pub(super) names: Vec<TokenName>,
    literal_types: Vec<LiteralType>,
    literal_limit: bool,
    declaration_operators: Vec<usize>,
    signature_types: Vec<(usize, Range<usize>, bool)>,
}

impl TokenLayout {
    pub(super) fn collect(
        workspace: &Workspace,
        file: &Path,
        source: &str,
        stub: &IndexedFile,
        window: Option<(usize, usize)>,
    ) -> Result<Self, (i32, String)> {
        let mut names = collect_lexical_names(source);
        names.extend(collect_import_names(workspace, file, source));
        names.sort_unstable_by_key(|name| name.span.start);

        let mut layout = Self {
            names,
            literal_types: Vec::new(),
            literal_limit: false,
            declaration_operators: Vec::new(),
            signature_types: Vec::new(),
        };

        let in_window = |start: usize, end: usize| {
            window.is_none_or(|(w_start, w_end)| start < w_end && end > w_start)
        };

        let map = SourceMap::new(source);
        let mut literals = Vec::new();
        for symbol in &stub.symbols {
            if let Some(body) = symbol.body() {
                if body.len() > crate::analysis::budget::MAX_BODY_BYTES {
                    continue;
                }
                let Some(start) = map.offset(symbol.range.start) else {
                    continue;
                };
                if in_window(start, body.end) {
                    // Normal checked function regions use the shared AST during `visit_file_regions`.
                    // Only pre-classify signatures for giant functions or parse failures here if needed;
                    // `classify_region` is idempotent on already-marked tokens.
                }
                continue;
            }
            let Some(end) = map.offset(symbol.range.end) else {
                continue;
            };
            if symbol.is_callable() {
                let Some(start) = map.offset(symbol.range.start) else {
                    continue;
                };
                if !in_window(start, end) {
                    continue;
                }
                let text = &source[start..end];
                let signature = crate::syntax::parse_signature(text);
                layout.classify_header(source, &signature, start, &mut literals);
                continue;
            }
            if !symbol.is_nameable() {
                continue;
            }
            let Some(decl_start) = map.offset(symbol.range.start) else {
                continue;
            };
            if !in_window(decl_start, end) {
                continue;
            }
            if let Some(offset) = declaration_assignment(&source[decl_start..end]) {
                layout.declaration_operators.push(decl_start + offset);
            }
            let mut start = symbol.end;
            if symbol.is_type() {
                start = layout.generic_type(source, start, end);
            }
            let Some(tail) = source.get(start..end) else {
                continue;
            };
            let mut tokens = Lexer::new(tail).peekable();
            if symbol.is_type() && tokens.peek().is_some_and(|t| t.kind == TokenKind::Assign) {
                start += tokens.next().expect("alias assignment").span.end;
            }
            let parsed = crate::syntax::parse_type(&source[start..end]);
            if parsed
                .diagnostics
                .iter()
                .all(|error| error.message == "unexpected token after type")
            {
                layout.aggregate_names(start + parsed.span.start..start + parsed.span.end);
                layout.types(&parsed.references, start);
                layout.aggregate_embedded(source, &parsed, start);
                layout.signature_names(&parsed.signature_names, start);
                layout.expressions(source, &parsed.expressions, start, &mut literals);
            }
            if symbol.is_type()
                && symbol
                    .type_info
                    .as_ref()
                    .is_some_and(|info| !info.members.is_empty() || !info.embedded.is_empty())
            {
                let declaration = &source[symbol.end..end];
                let root = crate::syntax::aggregate_offset(declaration);
                for entry in crate::syntax::aggregate_members(declaration) {
                    if Some(entry.container) != root {
                        continue;
                    }
                    if entry.selection.start != entry.annotation.start {
                        layout.mark(
                            symbol.end + entry.selection.start..symbol.end + entry.selection.end,
                            TokenRole::FieldDeclaration(symbol.start),
                        );
                    } else if symbol
                        .type_info
                        .as_ref()
                        .is_some_and(|info| !info.is_interface())
                    {
                        let annotation = &declaration[entry.annotation.clone()];
                        if let Some(name) = Lexer::new(annotation)
                            .take_while(|token| token.kind != TokenKind::LBrack)
                            .filter(|token| token.kind == TokenKind::Ident)
                            .last()
                        {
                            let ann_start = symbol.end + entry.annotation.start;
                            layout.embedded_type(
                                ann_start + name.span.start..ann_start + name.span.end,
                            );
                        }
                    }
                }
            }
            if !symbol.is_type()
                && let Some(assignment) =
                    Lexer::new(&source[start..end]).find(|t| t.kind == TokenKind::Assign)
            {
                let base = start + assignment.span.end;
                let parsed = crate::syntax::parse_expression(&source[base..end]);
                layout.types(&parsed.type_references, base);
                layout.expressions(source, &parsed.nodes, base, &mut literals);
            }
        }

        layout.drain_literals(source, &mut literals)?;
        layout.promote_anonymous_fields(0..source.len());
        layout.sort_aux();
        Ok(layout)
    }

    pub(super) fn classify_region(
        &mut self,
        source: &str,
        region_span: [u32; 2],
        signature: Option<(usize, &Signature)>,
        body: Option<(usize, &BlockTree)>,
    ) -> Result<(), (i32, String)> {
        if signature.is_none() && body.is_none() {
            return Ok(());
        }
        let mut literals = Vec::new();
        if let Some((sig_base, sig)) = signature {
            self.classify_header(source, sig, sig_base, &mut literals);
            self.expressions(source, &sig.expressions, sig_base, &mut literals);
        }
        if let Some((body_base, block)) = body {
            self.classify_block(source, block, body_base, &mut literals);
            self.expressions(source, &block.expressions, body_base, &mut literals);
        }
        self.drain_literals(source, &mut literals)?;
        self.promote_anonymous_fields(region_span[0] as usize..region_span[1] as usize);
        self.sort_aux();
        Ok(())
    }

    pub(super) fn declaration_operator(&self, offset: usize) -> bool {
        self.declaration_operators.binary_search(&offset).is_ok()
    }

    pub(super) fn signature_type(&self, offset: usize) -> Option<(&Range<usize>, bool)> {
        let index = self
            .signature_types
            .binary_search_by_key(&offset, |(start, _, _)| *start)
            .ok()?;
        Some((
            &self.signature_types[index].1,
            self.signature_types[index].2,
        ))
    }

    pub(super) fn literal_element_projection(
        &self,
        mut context: usize,
    ) -> Option<(Range<usize>, Vec<bool>)> {
        let mut projections = Vec::new();
        let span = loop {
            if projections.len() > 64 {
                return None;
            }
            match self.literal_types.get(context)? {
                LiteralType::Explicit(span) => break span,
                LiteralType::Element { container, key } => {
                    projections.push(*key);
                    context = *container;
                }
            }
        };
        if span.len() > 16 * 1024 {
            return None;
        }
        Some((span.clone(), projections))
    }

    fn drain_literals(
        &mut self,
        source: &str,
        literals: &mut Vec<(Range<usize>, Range<usize>, usize)>,
    ) -> Result<(), (i32, String)> {
        while let Some((signature, body, depth)) = literals.pop() {
            if body.len() > crate::analysis::budget::MAX_BODY_BYTES
                || signature.len() > crate::analysis::budget::MAX_BYTES
            {
                continue;
            }
            if depth >= 128 {
                return Err((
                    crate::error::REQUEST_FAILED,
                    "source query exceeds 128 nested function scopes".into(),
                ));
            }
            let header = crate::syntax::parse_literal_signature(&source[signature.clone()]);
            self.classify_header(source, &header, signature.start, literals);
            let parsed = crate::syntax::parse_block(&source[body.clone()]);
            self.classify_block(source, &parsed, body.start, literals);
            let old = literals.len();
            self.expressions(source, &parsed.expressions, body.start, literals);
            for literal in &mut literals[old..] {
                literal.2 = depth + 1;
            }
        }
        if self.literal_limit {
            return Err((
                crate::error::REQUEST_FAILED,
                "source query exceeds 4096 literal contexts or 64 elision levels".into(),
            ));
        }
        Ok(())
    }

    fn promote_anonymous_fields(&mut self, span: Range<usize>) {
        let start = self
            .names
            .partition_point(|name| name.span.start < span.start);
        for name in self.names[start..]
            .iter_mut()
            .take_while(|name| name.span.end <= span.end)
        {
            if matches!(name.role, TokenRole::Field) {
                name.role = TokenRole::FieldDeclaration(usize::MAX);
            }
        }
    }

    fn sort_aux(&mut self) {
        self.declaration_operators.sort_unstable();
        self.declaration_operators.dedup();
        self.signature_types
            .sort_unstable_by_key(|(start, _, _)| *start);
        self.signature_types.dedup_by_key(|(start, _, _)| *start);
    }

    fn mark(&mut self, span: Range<usize>, role: TokenRole) {
        if let Ok(index) = self
            .names
            .binary_search_by_key(&span.start, |name| name.span.start)
            && self.names[index].span == span
            && (!matches!(self.names[index].role, TokenRole::BlankDeclaration { .. })
                || !matches!(role, TokenRole::Name))
        {
            self.names[index].role = role;
        }
    }

    fn embedded_type(&mut self, span: Range<usize>) {
        if let Ok(index) = self
            .names
            .binary_search_by_key(&span.start, |name| name.span.start)
            && self.names[index].span == span
        {
            let qualifier = match &self.names[index].role {
                TokenRole::Type(qualifier) | TokenRole::EmbeddedType(qualifier) => {
                    qualifier.clone()
                }
                _ => None,
            };
            self.names[index].role = TokenRole::EmbeddedType(qualifier);
        }
    }

    fn contains_name(&self, span: &Range<usize>) -> bool {
        let index = self
            .names
            .partition_point(|name| name.span.start < span.start);
        self.names
            .get(index)
            .is_some_and(|name| name.span.end <= span.end)
    }

    fn types(&mut self, references: &[TypeReference], base: usize) {
        for reference in references {
            let qualifier = reference
                .qualifier
                .as_ref()
                .map(|span| base + span.start..base + span.end);
            if let Some(qualifier) = &qualifier {
                self.mark(qualifier.clone(), TokenRole::Namespace);
            }
            self.mark(
                base + reference.name.start..base + reference.name.end,
                TokenRole::Type(qualifier),
            );
        }
    }

    fn aggregate_names(&mut self, span: Range<usize>) {
        let start = self
            .names
            .partition_point(|name| name.span.start < span.start);
        for name in self.names[start..]
            .iter_mut()
            .take_while(|name| name.span.end <= span.end)
        {
            name.role = TokenRole::Field;
        }
    }

    fn signature_names(&mut self, names: &[SignatureName], base: usize) {
        for name in names {
            self.signature_types.push((
                base + name.name.start,
                base + name.annotation.start..base + name.annotation.end,
                name.variadic,
            ));
            let role = match name.kind {
                SignatureNameKind::Parameter => TokenRole::Parameter,
                SignatureNameKind::Result => TokenRole::Result,
            };
            self.mark(base + name.name.start..base + name.name.end, role);
        }
    }

    fn classify_header(
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
        let declarations = signature
            .receiver
            .iter()
            .chain(&signature.parameters)
            .map(|parameter| (parameter, TokenRole::Parameter))
            .chain(
                signature
                    .results
                    .iter()
                    .map(|parameter| (parameter, TokenRole::Result)),
            );
        for (parameter, role) in declarations {
            if let Some(name) = &parameter.name {
                self.mark(base + name.start..base + name.end, role);
            }
        }
    }

    fn classify_block(
        &mut self,
        source: &str,
        block: &BlockTree,
        base: usize,
        literals: &mut Vec<(Range<usize>, Range<usize>, usize)>,
    ) {
        for entry in &block.type_parameters {
            for parameter in &entry.parameters {
                if let Some(name) = &parameter.name {
                    self.mark(base + name.start..base + name.end, TokenRole::Name);
                }
            }
        }
        for (index, statement) in block.statements.iter().enumerate() {
            if let StmtKind::Declaration { names, keyword, .. } = &statement.kind {
                let keyword = source.get(base + keyword.start..base + keyword.end);
                for name in names {
                    self.mark(
                        base + name.start..base + name.end,
                        if source.get(base + name.start..base + name.end) == Some("_") {
                            TokenRole::BlankDeclaration {
                                is_type: keyword == Some("type"),
                                constant: keyword == Some("const"),
                            }
                        } else {
                            TokenRole::Name
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
                            TokenRole::BlankDeclaration {
                                is_type: false,
                                constant: false,
                            },
                        );
                    }
                }
            }
            if let Some(offset) = block.declaration_equals(crate::syntax::StmtId(index)) {
                self.declaration_operators.push(base + offset);
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
                    self.declaration_operators.push(base + operator.start);
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
                    self.mark(base + name.start..base + name.end, TokenRole::Ignored);
                }
                _ => {}
            }
        }
        self.types(&block.type_references, base);
    }

    fn expressions(
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
                    TokenRole::Name,
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
                        || TokenRole::ExpressionMember(base + original.start..base + original.end),
                        |qualifier| TokenRole::Member(Some(qualifier)),
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
                                    TokenRole::LiteralKey(base + ty.start..base + ty.end),
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

    fn literal_elements(&mut self, expressions: &[Expr], base: usize) {
        let mut contexts = HashMap::new();
        for (id, expression) in expressions.iter().enumerate() {
            let ExprKind::Composite {
                literal_type: Some(ty),
                elements,
            } = &expression.kind
            else {
                continue;
            };
            if !elided_children(expressions, elements).any(|(id, _)| {
                let span = &expressions[id.0].span;
                self.contains_name(&(base + span.start..base + span.end))
            }) {
                continue;
            }
            if self.literal_types.len() >= 4096 {
                self.literal_limit = true;
                return;
            }
            let ty = &expressions[ty.0].span;
            let context = self.literal_types.len();
            self.literal_types
                .push(LiteralType::Explicit(base + ty.start..base + ty.end));
            let mut pending = vec![(ExprId(id), context, 0)];
            while let Some((id, context, depth)) = pending.pop() {
                let ExprKind::Composite {
                    literal_type,
                    elements,
                } = &expressions[id.0].kind
                else {
                    continue;
                };
                if literal_type.is_none() {
                    for element in elements {
                        if let ExprKind::KeyValue { key, .. } = &expressions[element.0].kind
                            && matches!(expressions[key.0].kind, ExprKind::Name)
                        {
                            let key = &expressions[key.0].span;
                            self.mark(
                                base + key.start..base + key.end,
                                TokenRole::LiteralElementKey(context),
                            );
                        }
                    }
                }
                if depth >= 64 {
                    self.literal_limit |= elided_children(expressions, elements).next().is_some();
                    continue;
                }
                for (child, key) in elided_children(expressions, elements) {
                    let span = &expressions[child.0].span;
                    if !self.contains_name(&(base + span.start..base + span.end)) {
                        continue;
                    }
                    let next = match contexts.get(&(context, key)) {
                        Some(next) => *next,
                        None => {
                            if self.literal_types.len() >= 4096 {
                                self.literal_limit = true;
                                return;
                            }
                            let next = self.literal_types.len();
                            self.literal_types.push(LiteralType::Element {
                                container: context,
                                key,
                            });
                            contexts.insert((context, key), next);
                            next
                        }
                    };
                    pending.push((child, next, depth + 1));
                }
            }
        }
    }

    fn annotate_type(
        &mut self,
        source: &str,
        span: Range<usize>,
        literals: &mut Vec<(Range<usize>, Range<usize>, usize)>,
    ) {
        let Some(slice) = source.get(span.clone()) else {
            return;
        };
        let parsed = crate::syntax::parse_type(slice);
        if parsed.diagnostics.is_empty() {
            self.aggregate_names(span.clone());
            self.types(&parsed.references, span.start);
            self.aggregate_embedded(source, &parsed, span.start);
            self.expressions(source, &parsed.expressions, span.start, literals);
            self.signature_names(&parsed.signature_names, span.start);
        }
    }

    fn aggregate_embedded(
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
            self.mark(selection, TokenRole::EmbeddedType(qualifier));
        }
    }

    fn generic_type(&mut self, source: &str, start: usize, end: usize) -> usize {
        let Some(slice) = source.get(start..end) else {
            return start;
        };
        let Some(parameters) = crate::syntax::type_parameter_end(slice) else {
            return start;
        };
        if parameters > 16 * 1024 {
            return start;
        }
        let limit = start + parameters;
        let signature: Signature = crate::syntax::parse_type_parameters(&source[start..limit]);
        if !signature.diagnostics.is_empty() {
            return limit;
        }
        let mut literals = Vec::new();
        for parameter in &signature.type_parameters {
            self.annotate_type(
                source,
                start + parameter.annotation.start..start + parameter.annotation.end,
                &mut literals,
            );
        }
        for reference in &signature.type_references {
            let span = start + reference.name.start..start + reference.name.end;
            let qualifier = reference
                .qualifier
                .as_ref()
                .map(|q| start + q.start..start + q.end);
            if let Some(q) = &qualifier {
                self.mark(q.clone(), TokenRole::Namespace);
            }
            self.mark(span, TokenRole::Type(qualifier));
        }
        limit
    }
}

fn collect_lexical_names(source: &str) -> Vec<TokenName> {
    let mut previous = TokenKind::Illegal;
    let mut names = Vec::new();
    let mut importing = false;
    let mut grouped_import = false;
    for token in Lexer::new(source) {
        if token.kind == TokenKind::Ident {
            names.push(TokenName {
                span: token.span,
                role: if importing {
                    TokenRole::Namespace
                } else {
                    match previous {
                        TokenKind::Period => TokenRole::Member(None),
                        TokenKind::Package | TokenKind::Import => TokenRole::Namespace,
                        _ => TokenRole::Name,
                    }
                },
            });
        }
        match token.kind {
            TokenKind::Import => importing = true,
            TokenKind::LParen if importing => grouped_import = true,
            TokenKind::RParen if importing => {
                importing = false;
                grouped_import = false;
            }
            TokenKind::Semicolon if !grouped_import => importing = false,
            _ => {}
        }
        previous = token.kind;
    }
    names
}

fn collect_import_names(workspace: &Workspace, file: &Path, source: &str) -> Vec<TokenName> {
    let mut names = Vec::new();
    let mut importing = false;
    let mut grouped = false;
    let mut explicit = false;
    for token in Lexer::new(source) {
        if token.kind == TokenKind::Import {
            importing = true;
            explicit = false;
            continue;
        }
        if !importing {
            if matches!(
                token.kind,
                TokenKind::Func | TokenKind::Type | TokenKind::Var | TokenKind::Const
            ) {
                break;
            }
            continue;
        }
        match token.kind {
            TokenKind::LParen => grouped = true,
            TokenKind::RParen => {
                importing = false;
                grouped = false;
            }
            TokenKind::Ident | TokenKind::Period => explicit = true,
            TokenKind::Semicolon => {
                explicit = false;
                if !grouped {
                    importing = false;
                }
            }
            TokenKind::String => {
                if !explicit {
                    let literal = &source[token.span.clone()];
                    let package =
                        crate::syntax::decode_import_path(literal).and_then(|mut path| {
                            workspace.sdk_import_identity(file, &mut path);
                            workspace.declared_package_name(&path)
                        });
                    if let Some(package) = package
                        && let Some(offset) = literal.rfind(package)
                    {
                        names.push(TokenName {
                            span: token.span.start + offset
                                ..token.span.start + offset + package.len(),
                            role: TokenRole::Namespace,
                        });
                    }
                }
                explicit = false;
                if !grouped {
                    importing = false;
                }
            }
            _ => {}
        }
    }
    names
}

fn elided_children<'a>(
    expressions: &'a [Expr],
    elements: &'a [ExprId],
) -> impl Iterator<Item = (ExprId, bool)> + 'a {
    elements
        .iter()
        .flat_map(|element| match expressions[element.0].kind {
            ExprKind::KeyValue { key, value } => [Some((key, true)), Some((value, false))],
            _ => [None, Some((*element, false))],
        })
        .flatten()
}

fn declaration_assignment(source: &str) -> Option<usize> {
    let mut depth = 0usize;
    for token in Lexer::new(source) {
        match token.kind {
            TokenKind::Assign if depth == 0 => return Some(token.span.start),
            TokenKind::LParen | TokenKind::LBrack | TokenKind::LBrace => depth += 1,
            TokenKind::RParen | TokenKind::RBrack | TokenKind::RBrace => {
                depth = depth.checked_sub(1)?
            }
            _ => {}
        }
    }
    None
}
