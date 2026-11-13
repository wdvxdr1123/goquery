//! Compact occurrence classification derived from streaming tokens and body arenas.
use crate::analysis::scope::{Locals, ObjectKind};
use crate::index::IndexedFile;
use crate::syntax::{Lexer, Signature, TokenKind};
use crate::text::SourceMap;
use std::ops::Range;

pub(crate) enum Role {
    Name,
    Type(Option<Range<usize>>),
    EmbeddedType(Option<Range<usize>>),
    Member(Option<Range<usize>>),
    ExpressionMember(Range<usize>),
    Field,
    FieldDeclaration(usize),
    LiteralKey(Range<usize>),
    LiteralElementKey(usize),
    BlankDeclaration { kind: ObjectKind, constant: bool },
    Parameter,
    Result,
    Namespace,
    Ignored,
}

pub(crate) struct Name {
    pub span: Range<usize>,
    pub role: Role,
}

pub(super) enum LiteralType {
    Explicit(Range<usize>),
    Element { container: usize, key: bool },
}

pub(crate) struct SourceAnalysis {
    pub names: Vec<Name>,
    pub locals: Locals,
    pub(super) literal_types: Vec<LiteralType>,
    pub(super) literal_limit: bool,
    pub(super) declaration_operators: Option<Vec<usize>>,
    pub(super) signature_types: Option<Vec<(usize, Range<usize>, bool)>>,
}

impl SourceAnalysis {
    pub(crate) fn new(source: &str, stub: &IndexedFile) -> Result<Self, (i32, String)> {
        Self::build(source, stub, None, None, false)
    }

    pub(crate) fn for_semantic(source: &str, stub: &IndexedFile) -> Result<Self, (i32, String)> {
        Self::build(source, stub, None, None, true)
    }

    pub(crate) fn declaration_operator(&self, offset: usize) -> bool {
        self.declaration_operators
            .as_ref()
            .is_some_and(|positions| positions.binary_search(&offset).is_ok())
    }

    pub(crate) fn signature_type(&self, offset: usize) -> Option<(&Range<usize>, bool)> {
        let entries = self.signature_types.as_ref()?;
        let index = entries
            .binary_search_by_key(&offset, |(start, _, _)| *start)
            .ok()?;
        Some((&entries[index].1, entries[index].2))
    }

    /// Keeps only this lexical name and the scopes needed to resolve its uses.
    pub(crate) fn for_name(
        source: &str,
        stub: &IndexedFile,
        name: &str,
    ) -> Result<Self, (i32, String)> {
        Self::build(source, stub, Some(name), None, false)
    }

    /// External package globals must use one of these import qualifiers.
    /// Dot imports and methods require the unrestricted name analysis instead.
    pub(crate) fn for_qualified_name(
        source: &str,
        stub: &IndexedFile,
        name: &str,
        qualifiers: &[&str],
    ) -> Result<Self, (i32, String)> {
        Self::build(source, stub, Some(name), Some(qualifiers), false)
    }

    fn build(
        source: &str,
        stub: &IndexedFile,
        name: Option<&str>,
        qualifiers: Option<&[&str]>,
        semantic: bool,
    ) -> Result<Self, (i32, String)> {
        let (names, unshadowed) = Self::collect_names(source, stub, name, qualifiers);
        let mut result = Self {
            names,
            locals: Locals::empty(),
            literal_types: Vec::new(),
            literal_limit: false,
            declaration_operators: semantic.then(Vec::new),
            signature_types: semantic.then(Vec::new),
        };
        if unshadowed || name.is_some() && result.names.is_empty() {
            return Ok(result);
        }
        let map = SourceMap::new(source);
        let mut literals = Vec::new();
        for symbol in &stub.symbols {
            if let Some(body) = symbol.body() {
                let Some(start) = map.offset(symbol.range.start) else {
                    continue;
                };
                if name.is_some() && !result.contains_name(&(start..body.end)) {
                    continue;
                }
                let signature = crate::syntax::parse_signature(&source[start..body.start]);
                result.header(source, &signature, start, &mut literals);
                result.expressions(source, &signature.expressions, start, &mut literals);
                let parsed = crate::syntax::parse_block(&source[body.clone()]);
                let mut locals = Locals::from_parsed_signature(
                    &source[start..body.end],
                    body.start - start,
                    &signature,
                );
                if qualifiers.is_some() {
                    locals.append_qualified_body(
                        &source[start..body.end],
                        body.start - start,
                        &parsed,
                    );
                } else {
                    locals.append_body(&source[start..body.end], body.start - start, &parsed);
                }
                locals.rebase(start);
                result.locals.extend(locals);
                result.block(source, &parsed, body.start, &mut literals);
                result.expressions(source, &parsed.expressions, body.start, &mut literals);
                continue;
            }
            let Some(end) = map.offset(symbol.range.end) else {
                continue;
            };
            if symbol.is_callable() {
                let Some(start) = map.offset(symbol.range.start) else {
                    continue;
                };
                if name.is_some() && !result.contains_name(&(start..end)) {
                    continue;
                }
                let text = &source[start..end];
                let signature = crate::syntax::parse_signature(text);
                result.header(source, &signature, start, &mut literals);
                let mut locals = Locals::from_parsed_signature(text, text.len(), &signature);
                locals.rebase(start);
                result.locals.extend(locals);
                continue;
            }
            if !symbol.is_nameable() {
                continue;
            }
            if name.is_some() && !result.contains_name(&(symbol.start..end)) {
                continue;
            }
            if let Some(positions) = &mut result.declaration_operators
                && let Some(start) = map.offset(symbol.range.start)
                && let Some(offset) = declaration_assignment(&source[start..end])
            {
                positions.push(start + offset);
            }
            let mut start = symbol.end;
            if symbol.is_type() {
                start = result.generic_type(source, start, end);
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
                result.aggregate_names(start + parsed.span.start..start + parsed.span.end);
                result.types(&parsed.references, start);
                result.aggregate_embedded(source, &parsed, start);
                result.signature_names(&parsed.signature_names, start);
                result.expressions(source, &parsed.expressions, start, &mut literals);
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
                        result.mark(
                            symbol.end + entry.selection.start..symbol.end + entry.selection.end,
                            Role::FieldDeclaration(symbol.start),
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
                            let start = symbol.end + entry.annotation.start;
                            result.embedded_type(start + name.span.start..start + name.span.end);
                        }
                    }
                }
            }
            if !symbol.is_type() {
                // Initializers are expressions, not type annotations. The stub's
                // range ends at the declaration separator, not at the next file.
                if let Some(assignment) =
                    Lexer::new(&source[start..end]).find(|t| t.kind == TokenKind::Assign)
                {
                    let base = start + assignment.span.end;
                    let parsed = crate::syntax::parse_expression(&source[base..end]);
                    result.types(&parsed.type_references, base);
                    result.expressions(source, &parsed.nodes, base, &mut literals);
                }
            }
        }
        // A depth-first work list retains spans, never parent syntax arenas.
        while let Some((signature, body, depth)) = literals.pop() {
            if name.is_some() && !result.contains_name(&(signature.start..body.end)) {
                continue;
            }
            if depth >= 128 {
                return Err((
                    crate::error::REQUEST_FAILED,
                    "source query exceeds 128 nested function scopes".into(),
                ));
            }
            let header = crate::syntax::parse_literal_signature(&source[signature.clone()]);
            result.header(source, &header, signature.start, &mut literals);
            let mut locals = Locals::from_parsed_signature(
                &source[signature.start..body.end],
                body.start - signature.start,
                &header,
            );
            locals.rebase(signature.start);
            result.locals.extend(locals);
            let parsed = crate::syntax::parse_block(&source[body.clone()]);
            if qualifiers.is_some() {
                result
                    .locals
                    .append_qualified_body(source, body.start, &parsed);
            } else {
                result.locals.append_body(source, body.start, &parsed);
            }
            result.block(source, &parsed, body.start, &mut literals);
            let old = literals.len();
            result.expressions(source, &parsed.expressions, body.start, &mut literals);
            for literal in &mut literals[old..] {
                literal.2 = depth + 1;
            }
        }
        if let Some(positions) = &mut result.declaration_operators {
            positions.sort_unstable();
            positions.dedup();
        }
        if let Some(entries) = &mut result.signature_types {
            entries.sort_unstable_by_key(|(start, _, _)| *start);
            entries.dedup_by_key(|(start, _, _)| *start);
        }
        if result.literal_limit {
            return Err((
                crate::error::REQUEST_FAILED,
                "source query exceeds 4096 literal contexts or 64 elision levels".into(),
            ));
        }
        Ok(result)
    }

    pub(super) fn contains_name(&self, span: &Range<usize>) -> bool {
        let index = self
            .names
            .partition_point(|name| name.span.start < span.start);
        self.names
            .get(index)
            .is_some_and(|name| name.span.end <= span.end)
    }

    fn generic_type(&mut self, source: &str, start: usize, end: usize) -> usize {
        let Some(parameters) = crate::syntax::type_parameter_end(&source[start..end]) else {
            return start;
        };
        if parameters > 16 * 1024 {
            return start;
        }
        let limit = start + parameters;
        let header = format!("func Generic{}()", &source[start..limit]);
        let signature: Signature = crate::syntax::parse_signature(&header);
        if !signature.diagnostics.is_empty() {
            return limit;
        }
        let prefix = "func Generic".len();
        let mut literals = Vec::new();
        for parameter in &signature.type_parameters {
            self.annotate_type(
                source,
                start + parameter.annotation.start - prefix
                    ..start + parameter.annotation.end - prefix,
                &mut literals,
            );
            if let Some(name) = &parameter.name {
                let selection = start + name.start - prefix..start + name.end - prefix;
                self.locals.type_parameter(source, selection, start..end);
            }
        }
        for reference in &signature.type_references {
            let span = start + reference.name.start - prefix..start + reference.name.end - prefix;
            let qualifier = reference
                .qualifier
                .as_ref()
                .map(|q| start + q.start - prefix..start + q.end - prefix);
            if let Some(q) = &qualifier {
                self.mark(q.clone(), Role::Namespace);
            }
            self.mark(span, Role::Type(qualifier));
        }
        limit
    }
}

// Initializers can contain nested function bodies. Only the declaration's
// top-level '=' is excluded; assignments inside those bodies remain operators.
pub(super) fn declaration_assignment(source: &str) -> Option<usize> {
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
