//! Read-only syntax index for protocol syntax features, assembled on demand
//! from the existing parser outputs.
//!
//! A single pass over the existing lexer token stream supplies comments, raw
//! string literals and bracket groups that the existing parsers do not expose
//! (parameter/field lists, declaration groups, import groups). Bodies and
//! initializers are parsed with `parse_block`/`parse_expression`, and
//! declarations come from `stub`; no second lexer or grammar lives here.
//!
//! The index is query-local and source-free: callers keep the source snapshot.
//! Bounds: 8 MiB source, 600,000 tokens, 262,144 collected nodes. Larger inputs
//! return `None`, and consumers degrade to an empty result.
use crate::syntax::ast::expression::ExprKind;
use crate::syntax::ast::statement::{BlockTree, StmtKind};
use crate::syntax::lexer::Lexer;
use crate::syntax::token::{Token, TokenKind};
use std::ops::Range;

const MAX_SOURCE_BYTES: usize = 8 << 20;
const MAX_TOKENS: usize = 600_000;
const MAX_NODES: usize = 262_144;

/// Folding-relevant bracket groups whose spans the existing parsers do not
/// expose: parameter/field lists, declaration groups and import groups.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct FoldGroup {
    /// Opening bracket token span.
    pub open: Range<usize>,
    /// Closing bracket token span.
    pub close: Range<usize>,
    pub kind: FoldGroupKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum FoldGroupKind {
    /// Receiver, type-parameter, parameter/result, struct or interface list.
    FieldList,
    /// Parenthesized `const`/`var`/`type` declaration group.
    Decl,
    /// Parenthesized `import` declaration group.
    Imports,
}

/// Folding-relevant node with explicit bracket positions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct FoldNode {
    pub kind: FoldNodeKind,
    /// Block/composite/call opening bracket, or clause colon position.
    pub open: usize,
    /// Closing bracket position, or clause end position for clauses.
    pub close: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum FoldNodeKind {
    Block,
    Composite,
    Call,
    /// `case`/`default` clause; fold from `open + 1` to `close`.
    Case,
    /// `select` communication clause; fold from `open + 1` to `close`.
    Comm,
}

/// A comment group with its first comment, as `ast.File.Comments` groups them.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct CommentSpan {
    pub group: Range<usize>,
    pub first: Range<usize>,
}

/// One import specification with its source span and decoded path.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct ImportEntry {
    pub span: Range<usize>,
    pub path: String,
}

/// Query-local syntax index. Released after the request.
#[derive(Debug)]
#[non_exhaustive]
pub struct SyntaxIndex {
    comments: Vec<CommentSpan>,
    comment_spans: Vec<Range<usize>>,
    raw_strings: Vec<Range<usize>>,
    groups: Vec<FoldGroup>,
    nodes: Vec<FoldNode>,
    imports: Vec<ImportEntry>,
}

impl SyntaxIndex {
    /// Build the index for one Go source snapshot.
    pub fn parse(source: &str) -> Option<SyntaxIndex> {
        if source.len() > MAX_SOURCE_BYTES {
            return None;
        }
        let mut tokens = Vec::new();
        let mut comment_spans = Vec::new();
        let mut comment_separated = Vec::new();
        let mut comment_same_line = Vec::new();
        let mut separated = false;
        let mut last_token_start = None;
        for token in Lexer::with_comments(source) {
            if token.kind == TokenKind::Comment {
                let same_line = last_token_start.is_some_and(|start| {
                    start <= token.span.start && !source[start..token.span.start].contains('\n')
                });
                comment_spans.push(comment_text_span(
                    &source[token.span.clone()],
                    token.span.start,
                ));
                comment_separated.push(separated);
                comment_same_line.push(same_line);
                separated = false;
            } else {
                if tokens.len() >= MAX_TOKENS {
                    return None;
                }
                separated = true;
                last_token_start = Some(token.span.start);
                tokens.push(token);
            }
        }
        let mut index = SyntaxIndex {
            comments: comment_groups(
                source,
                &comment_spans,
                &comment_separated,
                &comment_same_line,
            ),
            comment_spans,
            raw_strings: Vec::new(),
            groups: Vec::new(),
            nodes: Vec::new(),
            imports: Vec::new(),
        };
        let partners = bracket_partners(&tokens);
        let mut scan = Scan {
            source,
            tokens: &tokens,
            partners,
            index: &mut index,
            pending_bodies: Vec::new(),
            budget: MAX_NODES,
        };
        scan.raw_strings();
        scan.declaration_groups();
        scan.function_groups();
        scan.bodies();
        scan.flush_pending();
        // Construction hygiene: the assembled sources can observe the same
        // bracket group twice (e.g. a body re-observed through its owner).
        // Distinct AST nodes of one kind never share a bracket span, so drop
        // exact duplicates while keeping cross-kind coincidences (gopls keeps
        // those, e.g. a single-argument call and its raw-string argument).
        index.groups.sort_by_key(|group| {
            (
                group.open.start,
                group.close.end,
                fold_group_rank(group.kind),
            )
        });
        index.groups.dedup_by_key(|group| {
            (
                group.open.start,
                group.close.end,
                fold_group_rank(group.kind),
            )
        });
        index
            .nodes
            .sort_by_key(|node| (node.open, node.close, fold_node_rank(node.kind)));
        index
            .nodes
            .dedup_by_key(|node| (node.open, node.close, fold_node_rank(node.kind)));
        index.raw_strings.sort_by_key(|span| (span.start, span.end));
        index
            .raw_strings
            .dedup_by_key(|span| (span.start, span.end));
        Some(index)
    }

    /// Comment groups in source order.
    pub fn comments(&self) -> &[CommentSpan] {
        &self.comments
    }
    /// Individual comment spans in source order.
    pub fn comment_spans(&self) -> &[Range<usize>] {
        &self.comment_spans
    }
    /// Raw string literal spans, including the back quotes.
    pub fn raw_strings(&self) -> &[Range<usize>] {
        &self.raw_strings
    }
    /// Folding bracket groups in source order.
    pub fn groups(&self) -> &[FoldGroup] {
        &self.groups
    }
    /// Folding nodes with explicit bracket positions.
    pub fn nodes(&self) -> &[FoldNode] {
        &self.nodes
    }
    /// Import specifications in source order, with decoded paths.
    pub fn imports(&self) -> &[ImportEntry] {
        &self.imports
    }
}

// ----------------------------------------------------------------------------
// Construction
// ----------------------------------------------------------------------------
struct Scan<'a, 'b> {
    source: &'a str,
    tokens: &'a [Token],
    partners: Vec<u32>,
    index: &'b mut SyntaxIndex,
    /// Function-literal bodies still to be parsed: (body span, owner).
    pending_bodies: Vec<Range<usize>>,
    budget: usize,
}

impl Scan<'_, '_> {
    fn exhausted(&mut self) -> bool {
        if self.budget == 0 {
            return true;
        }
        self.budget -= 1;
        false
    }
    fn token_index(&self, offset: usize) -> usize {
        self.tokens
            .partition_point(|token| token.span.start < offset)
    }
    fn kind_at(&self, index: usize) -> TokenKind {
        self.tokens
            .get(index)
            .map_or(TokenKind::Eof, |token| token.kind)
    }
    fn span_at(&self, index: usize) -> Range<usize> {
        self.tokens
            .get(index)
            .map_or(self.source.len()..self.source.len(), |token| {
                token.span.clone()
            })
    }
    fn partner_of(&self, index: usize) -> Option<usize> {
        let partner = *self.partners.get(index)?;
        (partner != u32::MAX).then_some(partner as usize)
    }
    fn skip_group(&self, index: usize) -> usize {
        self.partner_of(index).map_or(index + 1, |close| close + 1)
    }
    fn find_kind(&self, offset: usize, kind: TokenKind) -> Option<usize> {
        let start = self.token_index(offset);
        self.tokens[start..]
            .iter()
            .position(|token| token.kind == kind)
            .map(|position| start + position)
    }
    /// First token index at or after `offset` that is not inside a bracket
    /// group and matches `kind`.
    fn find_top_level(&self, offset: usize, kind: TokenKind) -> Option<usize> {
        let mut index = self.token_index(offset);
        while index < self.tokens.len() {
            match self.tokens[index].kind {
                TokenKind::LParen | TokenKind::LBrack | TokenKind::LBrace => {
                    index = self.skip_group(index);
                }
                found if found == kind => return Some(index),
                _ => index += 1,
            }
        }
        None
    }
    fn push_node(&mut self, node: FoldNode) {
        if self.exhausted() {
            return;
        }
        self.index.nodes.push(node);
    }

    fn raw_strings(&mut self) {
        for token in self.tokens {
            if token.kind == TokenKind::String && self.source[token.span.clone()].starts_with('`') {
                self.index.raw_strings.push(token.span.clone());
            }
        }
    }

    /// Top-level and local `import`/`const`/`var`/`type` parenthesized groups.
    fn declaration_groups(&mut self) {
        let mut index = 0;
        let mut braces = 0usize;
        while index < self.tokens.len() {
            match self.tokens[index].kind {
                TokenKind::LBrace => {
                    braces += 1;
                    index += 1;
                    continue;
                }
                TokenKind::RBrace => {
                    braces = braces.saturating_sub(1);
                    index += 1;
                    continue;
                }
                _ => {}
            }
            match self.tokens[index].kind {
                TokenKind::Import | TokenKind::Const | TokenKind::Var | TokenKind::Type => {
                    let next = index + 1;
                    if self.kind_at(next) == TokenKind::LParen {
                        if let Some((open, close)) = self.group(next) {
                            let kind = if self.tokens[index].kind == TokenKind::Import {
                                FoldGroupKind::Imports
                            } else {
                                FoldGroupKind::Decl
                            };
                            if !self.exhausted() {
                                self.index.groups.push(FoldGroup { open, close, kind });
                            }
                        }
                        if self.tokens[index].kind == TokenKind::Import {
                            self.import_specs(next);
                        }
                        index = self.skip_group(next);
                        continue;
                    }
                    if self.tokens[index].kind == TokenKind::Import {
                        self.import_specs(index);
                    }
                    if self.tokens[index].kind == TokenKind::Type {
                        self.type_declaration(index, braces == 0);
                    }
                    index += 1;
                }
                _ => index += 1,
            }
        }
    }

    /// Record the import specifications of one `import` declaration.
    fn import_specs(&mut self, keyword: usize) {
        let grouped = self.kind_at(keyword + 1) == TokenKind::LParen;
        let end = if grouped {
            self.partner_of(keyword + 1).unwrap_or(self.tokens.len())
        } else {
            self.declaration_end(keyword)
        };
        let mut index = if grouped { keyword + 2 } else { keyword + 1 };
        while index < end {
            if self.kind_at(index) == TokenKind::Semicolon {
                index += 1;
                continue;
            }
            let spec_start = index;
            if matches!(self.kind_at(index), TokenKind::Ident | TokenKind::Period) {
                index += 1;
            }
            if self.kind_at(index) == TokenKind::String {
                let path_span = self.span_at(index);
                let span = self.span_at(spec_start).start..path_span.end;
                let path = crate::syntax::decode_import_path(&self.source[path_span.clone()])
                    .unwrap_or_default();
                if !self.exhausted() {
                    self.index.imports.push(ImportEntry { span, path });
                }
                index += 1;
            } else {
                index += 1;
                continue;
            }
            while index < end && self.kind_at(index) != TokenKind::Semicolon {
                index += 1;
            }
        }
    }

    /// Type-parameter groups and array-length expressions of one `type`
    /// declaration (or grouped declaration spec starting at `keyword`).
    fn type_declaration(&mut self, keyword: usize, package_level: bool) {
        let grouped = self.kind_at(keyword + 1) == TokenKind::LParen;
        let (content_start, content_end) = if grouped {
            let open = keyword + 1;
            (open + 1, self.partner_of(open).unwrap_or(self.tokens.len()))
        } else {
            (keyword + 1, self.declaration_end(keyword))
        };
        let mut index = content_start;
        while index < content_end && self.kind_at(index) != TokenKind::RBrace {
            if self.kind_at(index) == TokenKind::Semicolon {
                index += 1;
                continue;
            }
            // spec: Name [type-params] [=] type
            if self.kind_at(index) != TokenKind::Ident {
                index += 1;
                continue;
            }
            let name = index;
            index += 1;
            if self.kind_at(index) == TokenKind::LBrack && self.type_parameter_list(index) {
                self.emit_group(index);
                index = self.skip_group(index);
            }
            let _ = package_level;
            if self.kind_at(index) == TokenKind::Assign {
                index += 1;
            }
            if index >= content_end {
                break;
            }
            let type_start = self.span_at(index).start;
            let type_end = if grouped {
                self.span_at(content_end).start
            } else {
                self.span_at(content_end.saturating_sub(1)).end
            };
            if package_level && type_start < type_end {
                // Package-level type declarations are parsed here; local ones
                // already contribute their array-length expressions through
                // the `parse_block` arena.
                let parsed = crate::syntax::parse_type(&self.source[type_start..type_end]);
                if !parsed.expressions.is_empty() {
                    let expressions = parsed.expressions;
                    self.expression_nodes(&expressions, type_start);
                }
            }
            // Advance past the spec's terminating depth-zero semicolon.
            let _ = name;
            index = self.token_index(type_end);
            while index < content_end && self.kind_at(index) != TokenKind::Semicolon {
                index = if matches!(
                    self.kind_at(index),
                    TokenKind::LParen | TokenKind::LBrack | TokenKind::LBrace
                ) {
                    self.skip_group(index)
                } else {
                    index + 1
                };
            }
        }
    }

    fn group(&self, open_index: usize) -> Option<(Range<usize>, Range<usize>)> {
        let close_index = self.partner_of(open_index)?;
        Some((self.span_at(open_index), self.span_at(close_index)))
    }

    /// Parameter, receiver, type-parameter, struct and interface field lists.
    fn function_groups(&mut self) {
        for index in 0..self.tokens.len() {
            match self.tokens[index].kind {
                TokenKind::Struct | TokenKind::Interface => {
                    if self.kind_at(index + 1) == TokenKind::LBrace
                        && let Some((open, close)) = self.group(index + 1)
                    {
                        if !self.exhausted() {
                            self.index.groups.push(FoldGroup {
                                open,
                                close,
                                kind: FoldGroupKind::FieldList,
                            });
                        }
                        if self.tokens[index].kind == TokenKind::Interface {
                            self.interface_methods(index + 1);
                        }
                    }
                }
                TokenKind::Func => self.function_group(index),
                _ => {}
            }
        }
    }

    /// Classify the bracket groups following one `func` keyword.
    fn function_group(&mut self, keyword: usize) {
        let mut index = keyword + 1;
        if self.kind_at(index) == TokenKind::Ident {
            if self.kind_at(index + 1) == TokenKind::LBrack && self.type_parameter_list(index + 1) {
                let next = self.skip_group(index + 1);
                self.emit_group(index + 1);
                index = next;
            } else {
                index += 1;
            }
        } else if self.kind_at(index) == TokenKind::LParen {
            // Either a method receiver or the parameter list.
            let Some(close) = self.partner_of(index) else {
                return;
            };
            let after = close + 1;
            let receiver = self.kind_at(after) == TokenKind::Ident && {
                let name = after;
                let mut params = name + 1;
                let type_params =
                    self.kind_at(params) == TokenKind::LBrack && self.type_parameter_list(params);
                if type_params {
                    params = self.skip_group(params);
                }
                self.kind_at(params) == TokenKind::LParen
            };
            if receiver {
                self.emit_group(index);
                let name = after;
                let mut params = name + 1;
                if self.kind_at(params) == TokenKind::LBrack && self.type_parameter_list(params) {
                    self.emit_group(params);
                    params = self.skip_group(params);
                }
                index = params;
            }
        } else {
            return;
        }
        // Parameter list.
        if self.kind_at(index) == TokenKind::LParen {
            self.emit_group(index);
            let after = self.skip_group(index);
            // Parenthesized result list.
            if self.kind_at(after) == TokenKind::LParen {
                self.emit_group(after);
            }
        }
    }

    /// Parameter/result field lists of the methods in one interface body.
    fn interface_methods(&mut self, open_index: usize) {
        let Some(close) = self.partner_of(open_index) else {
            return;
        };
        let mut index = open_index + 1;
        while index < close {
            if self.kind_at(index) == TokenKind::Semicolon {
                index += 1;
                continue;
            }
            let member_start = index;
            while index < close && self.kind_at(index) != TokenKind::Semicolon {
                match self.kind_at(index) {
                    TokenKind::Ident
                        if index == member_start
                            && self.kind_at(index + 1) == TokenKind::LParen
                            && index + 1 < close =>
                    {
                        self.emit_group(index + 1);
                        let after = self.skip_group(index + 1);
                        if self.kind_at(after) == TokenKind::LParen && after < close {
                            self.emit_group(after);
                        }
                        index = after;
                    }
                    TokenKind::LParen | TokenKind::LBrack | TokenKind::LBrace => {
                        index = self.skip_group(index);
                    }
                    _ => index += 1,
                }
            }
        }
    }

    fn emit_group(&mut self, open_index: usize) {
        if let Some((open, close)) = self.group(open_index)
            && !self.exhausted()
        {
            self.index.groups.push(FoldGroup {
                open,
                close,
                kind: FoldGroupKind::FieldList,
            });
        }
    }

    /// Prove that a `[` group is a declaration type-parameter list.
    fn type_parameter_list(&self, open_index: usize) -> bool {
        let span = self.span_at(open_index);
        crate::syntax::type_parameter_end(&self.source[span.start..]).is_some()
    }

    /// Function bodies and package-level initializers.
    fn bodies(&mut self) {
        let stub = crate::syntax::stub(self.source);
        for symbol in &stub.symbols {
            if let Some(body) = &symbol.body {
                self.parse_body(body.clone());
            }
        }
        // Declaration initializers: value expressions after a top-level `=`.
        let mut index = 0;
        while index < self.tokens.len() {
            if matches!(self.tokens[index].kind, TokenKind::Var | TokenKind::Const) {
                index = self.initializer_values(index);
            } else {
                index += 1;
            }
        }
    }

    /// Parse the value list of one `var`/`const` declaration; returns the token
    /// index after it.
    fn initializer_values(&mut self, keyword: usize) -> usize {
        let grouped = self.kind_at(keyword + 1) == TokenKind::LParen;
        let content_end = if grouped {
            self.partner_of(keyword + 1).unwrap_or(self.tokens.len())
        } else {
            self.declaration_end(keyword)
        };
        let mut index = if grouped { keyword + 2 } else { keyword + 1 };
        while index < content_end {
            if self.kind_at(index) == TokenKind::Semicolon {
                index += 1;
                continue;
            }
            // Find the depth-zero '=' of this spec.
            let spec_start = index;
            while index < content_end {
                match self.kind_at(index) {
                    TokenKind::Semicolon => break,
                    TokenKind::Assign => {
                        index += 1;
                        break;
                    }
                    TokenKind::LParen | TokenKind::LBrack | TokenKind::LBrace => {
                        index = self.skip_group(index);
                    }
                    _ => index += 1,
                }
            }
            if index >= content_end {
                index = spec_start + 1;
                continue;
            }
            // Value expressions separated by depth-zero commas.
            let mut value_start = index;
            let mut value_index = index;
            while value_index < content_end {
                match self.kind_at(value_index) {
                    TokenKind::Semicolon => break,
                    TokenKind::Comma => {
                        self.parse_value(value_start, value_index);
                        value_index += 1;
                        value_start = value_index;
                    }
                    TokenKind::LParen | TokenKind::LBrack | TokenKind::LBrace => {
                        value_index = self.skip_group(value_index);
                    }
                    _ => value_index += 1,
                }
            }
            self.parse_value(value_start, value_index);
            index = value_index.max(spec_start + 1);
        }
        content_end
    }

    fn declaration_end(&self, keyword: usize) -> usize {
        let mut index = keyword + 1;
        while index < self.tokens.len() {
            match self.kind_at(index) {
                TokenKind::Semicolon => return index,
                TokenKind::LParen | TokenKind::LBrack | TokenKind::LBrace => {
                    index = self.skip_group(index);
                }
                _ => index += 1,
            }
        }
        index
    }

    fn parse_value(&mut self, start: usize, end: usize) {
        if start >= end {
            return;
        }
        let span = self.span_at(start).start..self.span_at(end - 1).end;
        let expression = crate::syntax::parse_expression(&self.source[span.clone()]);
        self.expression_nodes(&expression.nodes, span.start);
    }

    fn parse_body(&mut self, span: Range<usize>) {
        if self.budget == 0 {
            return;
        }
        let tree = crate::syntax::parse_block(&self.source[span.clone()]);
        self.body_nodes(&tree, span.start);
    }

    fn body_nodes(&mut self, tree: &BlockTree, base: usize) {
        for statement in &tree.statements {
            let span = base + statement.span.start..base + statement.span.end;
            match &statement.kind {
                StmtKind::Block { .. } => {
                    if span.end > span.start {
                        self.push_node(FoldNode {
                            kind: FoldNodeKind::Block,
                            open: span.start,
                            close: span.end - 1,
                        });
                    }
                }
                StmtKind::Switch { .. } | StmtKind::TypeSwitch { .. } | StmtKind::Select { .. } => {
                    if span.end > span.start {
                        self.push_node(FoldNode {
                            kind: FoldNodeKind::Block,
                            open: self.body_open(span.start, span.end - 1),
                            close: span.end - 1,
                        });
                    }
                }
                StmtKind::Case { .. } | StmtKind::CommunicationCase { .. } => {
                    self.clause(tree, statement, base);
                }
                _ => {}
            }
        }
        self.expression_nodes(&tree.expressions, base);
    }

    fn clause(&mut self, tree: &BlockTree, statement: &crate::syntax::Statement, base: usize) {
        let span = base + statement.span.start..base + statement.span.end;
        match &statement.kind {
            StmtKind::Case {
                values, statements, ..
            } => {
                let after = values.last().map_or(span.start, |value| {
                    base + tree.expressions[value.0].span.end
                });
                let colon = self
                    .find_top_level(after, TokenKind::Colon)
                    .map(|index| self.span_at(index).start)
                    .unwrap_or(after);
                let end = statements.last().map_or(colon + 1, |statement| {
                    base + tree.statements[statement.0].span.end
                });
                self.push_node(FoldNode {
                    kind: FoldNodeKind::Case,
                    open: colon,
                    close: end,
                });
            }
            StmtKind::CommunicationCase {
                communication,
                statements,
            } => {
                let after = communication.map_or(span.start, |communication| {
                    base + tree.statements[communication.0].span.end
                });
                let colon = self
                    .find_top_level(after, TokenKind::Colon)
                    .map(|index| self.span_at(index).start)
                    .unwrap_or(after);
                let end = statements.last().map_or(colon + 1, |statement| {
                    base + tree.statements[statement.0].span.end
                });
                self.push_node(FoldNode {
                    kind: FoldNodeKind::Comm,
                    open: colon,
                    close: end,
                });
            }
            _ => {}
        }
    }

    /// Opening brace of a switch/select body whose closing brace is `close`.
    fn body_open(&self, from: usize, close: usize) -> usize {
        let mut index = self.token_index(from);
        while index < self.tokens.len() {
            let token = &self.tokens[index];
            if token.kind == TokenKind::LBrace
                && self
                    .partner_of(index)
                    .is_some_and(|partner| self.span_at(partner).start == close)
            {
                return token.span.start;
            }
            index += 1;
        }
        close
    }

    fn expression_nodes(&mut self, expressions: &[crate::syntax::Expr], base: usize) {
        if self.budget == 0 {
            return;
        }
        for expression in expressions {
            let span = base + expression.span.start..base + expression.span.end;
            match &expression.kind {
                ExprKind::Call { function, .. } => {
                    let function_end = base + expressions[function.0].span.end;
                    let open = self
                        .find_kind(function_end, TokenKind::LParen)
                        .map_or(span.start, |index| self.span_at(index).start);
                    self.push_node(FoldNode {
                        kind: FoldNodeKind::Call,
                        open,
                        close: span.end - 1,
                    });
                }
                ExprKind::Composite { literal_type, .. } => {
                    let open = literal_type.map_or(span.start, |literal_type| {
                        let end = base + expressions[literal_type.0].span.end;
                        self.find_kind(end, TokenKind::LBrace)
                            .map_or(span.start, |index| self.span_at(index).start)
                    });
                    self.push_node(FoldNode {
                        kind: FoldNodeKind::Composite,
                        open,
                        close: span.end - 1,
                    });
                }
                ExprKind::FunctionLiteral { body, .. } => {
                    let body_span = base + body.start..base + body.end;
                    self.pending_bodies.push(body_span);
                }
                _ => {}
            }
        }
    }

    fn flush_pending(&mut self) {
        while let Some(body) = self.pending_bodies.pop() {
            if self.budget == 0 {
                return;
            }
            self.parse_body(body);
        }
    }
}

/// `ast.Comment.End` is `Slash + len(Text)`, and the scanner strips CRs from
/// comment text; emulate that so CRLF sources report identical ends.
fn comment_text_span(text: &str, start: usize) -> Range<usize> {
    let bytes = text.as_bytes();
    let mut kept = 0usize;
    let mut index = 0usize;
    if bytes.starts_with(b"//") {
        let mut end = bytes.len();
        if text.ends_with('\r') {
            end -= 1;
        }
        for &byte in &bytes[..end] {
            if byte != b'\r' {
                kept += 1;
            }
        }
        return start..start + kept;
    }
    let block = bytes.starts_with(b"/*");
    let mut last_kept = None;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte != b'\r'
            || block
                && kept > 2
                && last_kept == Some(b'*')
                && index + 1 < bytes.len()
                && bytes[index + 1] == b'/'
        {
            kept += 1;
            last_kept = Some(byte);
        }
        index += 1;
    }
    start..start + kept
}

fn fold_group_rank(kind: FoldGroupKind) -> u8 {
    match kind {
        FoldGroupKind::FieldList => 0,
        FoldGroupKind::Decl => 1,
        FoldGroupKind::Imports => 2,
    }
}

fn fold_node_rank(kind: FoldNodeKind) -> u8 {
    match kind {
        FoldNodeKind::Block => 0,
        FoldNodeKind::Composite => 1,
        FoldNodeKind::Call => 2,
        FoldNodeKind::Case => 3,
        FoldNodeKind::Comm => 4,
    }
}

fn bracket_partners(tokens: &[Token]) -> Vec<u32> {
    let mut partners = vec![u32::MAX; tokens.len()];
    let mut stack: Vec<usize> = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        match token.kind {
            TokenKind::LParen | TokenKind::LBrack | TokenKind::LBrace => stack.push(index),
            TokenKind::RParen | TokenKind::RBrack | TokenKind::RBrace => {
                if let Some(open) = stack.pop() {
                    partners[open] = index as u32;
                    partners[index] = open as u32;
                }
            }
            _ => {}
        }
    }
    partners
}

/// Group comments that are adjacent in the token stream with no blank line
/// between them, matching the scanner's groups.
fn comment_groups(
    source: &str,
    comments: &[Range<usize>],
    separated: &[bool],
    same_line: &[bool],
) -> Vec<CommentSpan> {
    let mut groups: Vec<CommentSpan> = Vec::new();
    let mut cursor = 0usize;
    let mut line = 0usize;
    let mut previous_end_line = 0usize;
    let bytes = source.as_bytes();
    let mut index = 0;
    // Only `n == 1` groups absorb following comment lines; a line comment
    // group attached to a preceding token is closed on its own line.
    let mut joinable = false;
    while index < comments.len() {
        let comment = &comments[index];
        let start_line = advance_lines(bytes, &mut cursor, &mut line, comment.start);
        let adjacent = index > 0 && !separated.get(index).copied().unwrap_or(true);
        if adjacent
            && joinable
            && let Some(group) = groups.last_mut()
            && start_line <= previous_end_line + 1
        {
            group.group.end = comment.end;
            previous_end_line = advance_lines(bytes, &mut cursor, &mut line, comment.end);
            index += 1;
            continue;
        }
        // A comment that starts on the line of the preceding token is a line
        // comment: it only groups with comments on the same line.
        if same_line.get(index).copied().unwrap_or(false) {
            let line_start = start_line;
            let mut group = CommentSpan {
                group: comment.clone(),
                first: comment.clone(),
            };
            previous_end_line = advance_lines(bytes, &mut cursor, &mut line, comment.end);
            index += 1;
            while index < comments.len() && !separated.get(index).copied().unwrap_or(true) {
                let next = &comments[index];
                let next_line = advance_lines(bytes, &mut cursor, &mut line, next.start);
                if next_line != line_start {
                    break;
                }
                group.group.end = next.end;
                previous_end_line = advance_lines(bytes, &mut cursor, &mut line, next.end);
                index += 1;
            }
            groups.push(group);
            joinable = false;
            continue;
        }
        let mut group = CommentSpan {
            group: comment.clone(),
            first: comment.clone(),
        };
        previous_end_line = advance_lines(bytes, &mut cursor, &mut line, comment.end);
        index += 1;
        while index < comments.len() && !separated.get(index).copied().unwrap_or(true) {
            let next = &comments[index];
            let next_line = advance_lines(bytes, &mut cursor, &mut line, next.start);
            if next_line > previous_end_line + 1 {
                break;
            }
            group.group.end = next.end;
            previous_end_line = advance_lines(bytes, &mut cursor, &mut line, next.end);
            index += 1;
        }
        groups.push(group);
        joinable = true;
    }
    groups
}

fn advance_lines(bytes: &[u8], cursor: &mut usize, line: &mut usize, offset: usize) -> usize {
    let offset = offset.min(bytes.len());
    while *cursor < offset {
        if bytes[*cursor] == b'\n' {
            *line += 1;
        }
        *cursor += 1;
    }
    *line
}
