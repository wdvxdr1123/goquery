use std::fmt;

#[derive(Debug)]
enum Node {
    Tag(String),
    Not(usize),
    And(usize, usize),
    Or(usize, usize),
}

/// A flat constraint arena with bounded input and nesting.
#[derive(Debug)]
pub struct Constraint {
    nodes: Vec<Node>,
    root: usize,
}

impl Constraint {
    /// Evaluates tags supplied by a target/build context. No host assumptions.
    pub fn matches(&self, mut tag: impl FnMut(&str) -> bool) -> bool {
        let mut values: Vec<bool> = Vec::with_capacity(self.nodes.len());
        for node in &self.nodes {
            let value = match node {
                Node::Tag(name) => tag(name),
                Node::Not(id) => !values[*id],
                Node::And(left, right) => values[*left] && values[*right],
                Node::Or(left, right) => values[*left] || values[*right],
            };
            values.push(value);
        }
        values[self.root]
    }
}

/// Invalid build syntax/configuration or a resource limit, at a UTF-8 byte offset.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ParseError {
    pub offset: usize,
    pub message: &'static str,
}
impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at byte {}", self.message, self.offset)
    }
}
impl std::error::Error for ParseError {}

/// Parses the expression after `//go:build`. Empty and malformed input is an
/// error. Input is capped at 8 KiB and nested parentheses/negations at 128.
pub fn parse(source: &str) -> Result<Constraint, ParseError> {
    let mut parser = Parser {
        source,
        offset: 0,
        nodes: Vec::new(),
    };
    if source.len() > 8192 {
        return Err(parser.error("constraint exceeds 8 KiB"));
    }
    let root = parser.or(0)?;
    parser.space();
    if parser.offset != source.len() {
        return Err(parser.error("unexpected constraint token"));
    }
    Ok(Constraint {
        nodes: parser.nodes,
        root,
    })
}

struct Parser<'a> {
    source: &'a str,
    offset: usize,
    nodes: Vec<Node>,
}
impl Parser<'_> {
    fn error(&self, message: &'static str) -> ParseError {
        ParseError {
            offset: self.offset,
            message,
        }
    }
    fn add(&mut self, node: Node) -> usize {
        let id = self.nodes.len();
        self.nodes.push(node);
        id
    }
    fn space(&mut self) {
        while self.source[self.offset..].starts_with([' ', '\t', '\r']) {
            self.offset += 1;
        }
    }
    fn eat(&mut self, text: &str) -> bool {
        self.space();
        if self.source[self.offset..].starts_with(text) {
            self.offset += text.len();
            true
        } else {
            false
        }
    }
    fn or(&mut self, depth: usize) -> Result<usize, ParseError> {
        let mut left = self.and(depth)?;
        while self.eat("||") {
            let right = self.and(depth)?;
            left = self.add(Node::Or(left, right));
        }
        Ok(left)
    }
    fn and(&mut self, depth: usize) -> Result<usize, ParseError> {
        let mut left = self.unary(depth)?;
        while self.eat("&&") {
            let right = self.unary(depth)?;
            left = self.add(Node::And(left, right));
        }
        Ok(left)
    }
    fn unary(&mut self, depth: usize) -> Result<usize, ParseError> {
        if depth >= 128 {
            return Err(self.error("constraint nesting limit exceeded"));
        }
        if self.eat("!") {
            let operand = self.unary(depth + 1)?;
            return Ok(self.add(Node::Not(operand)));
        }
        if self.eat("(") {
            let value = self.or(depth + 1)?;
            if !self.eat(")") {
                return Err(self.error("expected ')'"));
            }
            return Ok(value);
        }
        self.space();
        let start = self.offset;
        for ch in self.source[start..].chars() {
            if !ch.is_alphanumeric() && ch != '_' && ch != '.' {
                break;
            }
            self.offset += ch.len_utf8();
        }
        if start == self.offset {
            return Err(self.error("expected build tag"));
        }
        Ok(self.add(Node::Tag(self.source[start..self.offset].to_owned())))
    }
}
