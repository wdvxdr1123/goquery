//! Declaration-only recursive descent with bounded token lookahead.
use crate::syntax::ast::declarations::{
    Import, ImportName, Stub, Symbol, SymbolKind, TypeConstructor,
};
use crate::syntax::lexer::Lexer;
use crate::syntax::literal;
use crate::syntax::token::{Token, TokenKind};
use std::iter::Peekable;

pub fn stub(source: &str) -> Stub {
    Parser {
        source,
        tokens: Lexer::new(source).peekable(),
        result: Stub::default(),
    }
    .parse()
}

struct Parser<'a> {
    source: &'a str,
    tokens: Peekable<Lexer<'a>>,
    result: Stub,
}

impl Parser<'_> {
    fn spelling(&self, token: &Token) -> &str {
        &self.source[token.span.clone()]
    }

    fn parse(mut self) -> Stub {
        while let Some(token) = self.tokens.next() {
            match self.spelling(&token) {
                "package" => {
                    if let Some(name) = self.tokens.next().filter(|t| t.kind == TokenKind::Ident) {
                        self.result.package = self.spelling(&name).to_owned();
                    }
                }
                "import" => self.imports(),
                "func" => self.function(token),
                "type" => self.declarations(SymbolKind::Type, token.span.start),
                "var" => self.declarations(SymbolKind::Variable, token.span.start),
                "const" => self.declarations(SymbolKind::Constant, token.span.start),
                _ => {}
            }
        }
        self.result
    }

    fn imports(&mut self) {
        let grouped = self.eat(TokenKind::LParen);
        if !grouped {
            self.import_spec();
            return;
        }
        loop {
            while self.eat(TokenKind::Semicolon) {}
            if self.eat(TokenKind::RParen) {
                break;
            }
            let Some(before) = self.tokens.peek().map(|token| token.span.start) else {
                break;
            };
            self.import_spec();
            // Malformed specs must not consume the group terminator or the rest
            // of the file: advance only past a token that failed to form a spec.
            if self.tokens.peek().map(|token| token.span.start) == Some(before) {
                self.tokens.next();
            }
        }
    }

    /// Consumes one import spec (grouped or ungrouped). Malformed specs stop
    /// before a `)`/`;` so grouped recovery can continue at the next spec instead
    /// of swallowing subsequent declarations.
    fn import_spec(&mut self) {
        let Some(first) = self.tokens.peek().cloned() else {
            return;
        };
        let name = match first.kind {
            TokenKind::String => {
                self.tokens.next();
                if let Some(path) = literal::import_path(self.spelling(&first)) {
                    self.result.imports.push(Import {
                        name: ImportName::Package,
                        path,
                    });
                }
                return;
            }
            TokenKind::Period => {
                self.tokens.next();
                ImportName::Dot
            }
            TokenKind::Ident => {
                let name = if self.spelling(&first) == "_" {
                    ImportName::Blank
                } else {
                    ImportName::Alias(self.spelling(&first).to_owned())
                };
                self.tokens.next();
                name
            }
            _ => return,
        };
        let Some(path) = self
            .tokens
            .peek()
            .filter(|token| token.kind == TokenKind::String)
            .cloned()
        else {
            return;
        };
        self.tokens.next();
        if let Some(path) = literal::import_path(self.spelling(&path)) {
            self.result.imports.push(Import { name, path });
        }
    }

    fn function(&mut self, keyword: Token) {
        let receiver_list = self.eat(TokenKind::LParen);
        let mut receiver = None;
        if receiver_list {
            let mut bracket_depth = 0usize;
            let mut parentheses = 1usize;
            for token in self.tokens.by_ref() {
                match token.kind {
                    TokenKind::LParen => parentheses += 1,
                    TokenKind::RParen => {
                        parentheses -= 1;
                        if parentheses == 0 {
                            break;
                        }
                    }
                    TokenKind::LBrack => bracket_depth += 1,
                    TokenKind::RBrack => bracket_depth = bracket_depth.saturating_sub(1),
                    TokenKind::Ident if bracket_depth == 0 => {
                        receiver = Some(self.source[token.span].to_owned())
                    }
                    _ => {}
                }
            }
        }
        let Some(name) = self.tokens.next().filter(|t| t.kind == TokenKind::Ident) else {
            return;
        };
        // A function name is followed by a type parameter list or the input
        // parameter list; an array/slice type cannot appear in that position, so
        // `[` proves genericity without the declaration ambiguity heuristic.
        let generic = self
            .tokens
            .peek()
            .is_some_and(|token| token.kind == TokenKind::LBrack);
        let mut end = name.span.end;
        let mut body = None;
        let mut type_body = false;
        while let Some(token) = self.tokens.next() {
            if token.kind == TokenKind::Semicolon {
                break;
            }
            end = token.span.end;
            match token.kind {
                TokenKind::LParen | TokenKind::LBrack => end = self.skip_group(token.kind),
                TokenKind::LBrace => {
                    end = self.skip_group(TokenKind::LBrace);
                    if !type_body {
                        body = Some(token.span.start..end);
                        break;
                    }
                    type_body = false;
                }
                _ => type_body = matches!(self.spelling(&token), "struct" | "interface"),
            }
        }
        // The receiver list presence, not a parsed receiver name, makes a method.
        let kind = if receiver_list {
            SymbolKind::Method
        } else {
            SymbolKind::Function
        };
        self.result.symbols.push(Symbol {
            name: self.spelling(&name).to_owned(),
            kind,
            generic,
            alias: false,
            leaf: false,
            constructor: TypeConstructor::Unknown,
            selection: name.span,
            range: keyword.span.start..end,
            receiver: receiver.map(String::into_boxed_str),
            body,
            type_info: None,
        });
    }

    fn declarations(&mut self, kind: SymbolKind, keyword: usize) {
        let grouped = self.eat(TokenKind::LParen);
        loop {
            if self.eat(TokenKind::Semicolon) {
                if grouped {
                    continue;
                } else {
                    break;
                }
            }
            if grouped
                && self
                    .tokens
                    .peek()
                    .is_some_and(|token| token.kind == TokenKind::RParen)
            {
                let end = self
                    .tokens
                    .next()
                    .expect("peeked closing parenthesis")
                    .span
                    .end;
                if kind == SymbolKind::Constant {
                    self.result.constant_groups.push(keyword..end);
                }
                break;
            }
            let Some(first) = self.tokens.next() else {
                break;
            };
            if first.kind != TokenKind::Ident {
                if !grouped {
                    break;
                } else {
                    continue;
                }
            }
            let start = first.span.start;
            let mut end = first.span.end;
            let declaration_start = end;
            let mut names = vec![first];
            if kind != SymbolKind::Type {
                while self.eat(TokenKind::Comma) {
                    if let Some(name) = self.tokens.next().filter(|t| t.kind == TokenKind::Ident) {
                        end = name.span.end;
                        names.push(name);
                    }
                }
            }
            while let Some(token) = self.tokens.peek() {
                if token.kind == TokenKind::Semicolon
                    || (grouped && token.kind == TokenKind::RParen)
                {
                    break;
                }
                let token = self.tokens.next().expect("peeked token exists");
                end = match token.kind {
                    TokenKind::LParen | TokenKind::LBrack | TokenKind::LBrace => {
                        self.skip_group(token.kind)
                    }
                    _ => token.span.end,
                };
            }
            let facts = if kind == SymbolKind::Type {
                declaration_facts(&self.source[declaration_start..end])
            } else {
                DeclarationFacts::default()
            };
            for name in names {
                self.result.symbols.push(Symbol {
                    name: self.spelling(&name).to_owned(),
                    kind,
                    generic: facts.generic,
                    alias: facts.alias,
                    leaf: facts.leaf,
                    constructor: facts.constructor,
                    type_info: if kind == SymbolKind::Type {
                        super::aggregate::type_info(
                            &self.source[name.span.end..end],
                            facts.constructor,
                            facts.offset,
                        )
                    } else {
                        None
                    },
                    selection: name.span,
                    range: start..end,
                    receiver: None,
                    body: None,
                });
            }
            if !grouped {
                break;
            }
        }
    }

    fn eat(&mut self, kind: TokenKind) -> bool {
        if self.tokens.peek().is_some_and(|token| token.kind == kind) {
            self.tokens.next();
            true
        } else {
            false
        }
    }

    // Iterative skipping keeps memory independent of function-body token count.
    fn skip_group(&mut self, opening: TokenKind) -> usize {
        let closing = match opening {
            TokenKind::LParen => TokenKind::RParen,
            TokenKind::LBrack => TokenKind::RBrack,
            _ => TokenKind::RBrace,
        };
        let mut depth = 1;
        for token in self.tokens.by_ref() {
            if token.kind == opening {
                depth += 1;
            }
            if token.kind == closing {
                depth -= 1;
            }
            if depth == 0 {
                return token.span.end;
            }
        }
        self.source.len()
    }
}

struct DeclarationFacts {
    generic: bool,
    alias: bool,
    leaf: bool,
    constructor: TypeConstructor,
    offset: usize,
}

impl Default for DeclarationFacts {
    fn default() -> Self {
        Self {
            generic: false,
            alias: false,
            leaf: false,
            constructor: TypeConstructor::Unknown,
            offset: 0,
        }
    }
}

fn declaration_facts(source: &str) -> DeclarationFacts {
    let parameters = super::aggregate::type_parameter_end(source);
    let mut tokens = Lexer::new(&source[parameters.unwrap_or(0)..]).peekable();
    let alias = tokens
        .peek()
        .is_some_and(|token| token.kind == TokenKind::Assign);
    if alias {
        tokens.next();
    }
    let mut parenthesized = false;
    let first = tokens
        .by_ref()
        .take(32)
        .inspect(|token| parenthesized |= token.kind == TokenKind::LParen)
        .find(|token| token.kind != TokenKind::LParen);
    let mut facts = DeclarationFacts {
        generic: parameters.is_some(),
        alias,
        ..DeclarationFacts::default()
    };
    if let Some(token) = first {
        facts.offset = if parenthesized {
            usize::MAX
        } else {
            parameters.unwrap_or(0) + token.span.start
        };
        match token.kind {
            TokenKind::Func => {
                facts.leaf = true;
                facts.constructor = TypeConstructor::Func;
            }
            TokenKind::Map => {
                facts.leaf = true;
                facts.constructor = TypeConstructor::Map;
            }
            TokenKind::Chan | TokenKind::Arrow => {
                facts.leaf = true;
                facts.constructor = TypeConstructor::Chan;
            }
            TokenKind::Mul => {
                facts.leaf = true;
                facts.constructor = TypeConstructor::Pointer;
            }
            TokenKind::Struct => facts.constructor = TypeConstructor::Struct,
            TokenKind::Interface => facts.constructor = TypeConstructor::Interface,
            TokenKind::LBrack => match tokens.next().map(|token| token.kind) {
                Some(TokenKind::RBrack) => {
                    facts.leaf = true;
                    facts.constructor = TypeConstructor::Slice;
                }
                Some(_) => facts.constructor = TypeConstructor::Array,
                None => {}
            },
            TokenKind::Ident => facts.constructor = TypeConstructor::Named,
            _ => {}
        }
    }
    facts
}
