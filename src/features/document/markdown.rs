//! Go doc-comment to Markdown conversion matching gopls' `DocCommentToMarkdown`.
//!
//! This is a focused port of `go/doc/comment` (parser and Markdown printer)
//! covering the syntax that appears in Go documentation comments.
use std::collections::HashMap;

const STD_PACKAGES: &[&str] = &[
    "bufio", "bytes", "cmp", "context", "crypto", "embed", "encoding", "errors", "expvar", "flag",
    "fmt", "hash", "html", "image", "io", "iter", "log", "maps", "math", "mime", "net", "os",
    "path", "plugin", "reflect", "regexp", "runtime", "slices", "sort", "strconv", "strings",
    "structs", "sync", "syscall", "testing", "time", "unicode", "unique", "unsafe", "uuid", "weak",
];

/// Render one documentation comment group as gopls-compatible Markdown.
pub(crate) fn markdown(text: &str, links: &[(String, String)]) -> String {
    let definitions: HashMap<&str, &str> = links
        .iter()
        .map(|(name, url)| (name.as_str(), url.as_str()))
        .collect();
    let mut parser = Parser {
        links: definitions,
        defined: HashMap::new(),
    };
    let lines: Vec<String> = unindent(text);
    let mut blocks = parser.blocks(&lines);
    parser.resolve_links(&mut blocks);
    let mut printer = Printer {
        heading_prefix: "### ",
        out: String::new(),
    };
    printer.doc(&blocks);
    printer.out
}

fn unindent(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = text.split('\n').map(str::to_owned).collect();
    while lines.first().is_some_and(|line| blank(line)) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|line| blank(line)) {
        lines.pop();
    }
    if lines.is_empty() {
        return lines;
    }
    let mut prefix = leading_space(&lines[0]);
    for line in &lines[1..] {
        if !blank(line) {
            prefix = common_prefix(&prefix, &leading_space(line));
        }
    }
    let mut out: Vec<String> = lines
        .iter()
        .map(|line| {
            let line = line.strip_prefix(prefix.as_str()).unwrap_or(line);
            if line.trim().is_empty() {
                String::new()
            } else {
                line.to_owned()
            }
        })
        .collect();
    while out.first().is_some_and(|line| line.is_empty()) {
        out.remove(0);
    }
    while out.last().is_some_and(|line| line.is_empty()) {
        out.pop();
    }
    out
}

fn blank(line: &str) -> bool {
    line.is_empty() || (line.len() == 1 && line == "\n")
}

fn leading_space(line: &str) -> String {
    line.chars()
        .take_while(|ch| *ch == ' ' || *ch == '\t')
        .collect()
}

fn common_prefix(left: &str, right: &str) -> String {
    left.chars()
        .zip(right.chars())
        .take_while(|(left, right)| left == right)
        .map(|(ch, _)| ch)
        .collect()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SpanKind {
    Code,
    Heading,
    List,
    OldHeading,
    Paragraph,
}

struct Span {
    start: usize,
    end: usize,
    kind: SpanKind,
}

fn parse_spans(lines: &[String]) -> Vec<Span> {
    let mut spans = Vec::new();
    let mut index = 0usize;
    let mut force_indent = 0usize;
    let mut watchdog = lines.len().saturating_mul(2).max(1);
    loop {
        while index < lines.len() && lines[index].is_empty() {
            index += 1;
        }
        if index >= lines.len() {
            break;
        }
        if watchdog == 0 {
            break;
        }
        watchdog -= 1;
        let start = index;
        let mut end;
        let kind;
        if index < force_indent || indented(&lines[index]) {
            let mut unindented_list = is_list(&lines[index]) && index < force_indent;
            index += 1;
            while index < lines.len()
                && (lines[index].is_empty()
                    || index < force_indent
                    || indented(&lines[index])
                    || (unindented_list && is_list(&lines[index])))
            {
                if lines[index].is_empty() {
                    unindented_list = false;
                }
                index += 1;
            }
            end = index;
            while end > start && lines[end - 1].is_empty() {
                end -= 1;
            }
            if end < lines.len() && lines[end].starts_with('}') {
                end += 1;
            }
            kind = if is_list(&lines[start]) {
                SpanKind::List
            } else {
                SpanKind::Code
            };
        } else {
            index += 1;
            while index < lines.len() && !lines[index].is_empty() && !indented(&lines[index]) {
                index += 1;
            }
            end = index;
            if index < lines.len() && !lines[index].is_empty() && !is_list(&lines[index]) {
                let previous = lines[index - 1].as_str();
                if is_list(previous) {
                    force_indent = end;
                    end = end.saturating_sub(1);
                    while end > start && is_list(&lines[end - 1]) {
                        end -= 1;
                    }
                } else if previous.ends_with('{') || previous.ends_with('\\') {
                    force_indent = end;
                    end = end.saturating_sub(1);
                }
                if start == end && force_indent > start {
                    index = start;
                    continue;
                }
            }
            if end - start == 1 && is_heading(&lines[start]) {
                kind = SpanKind::Heading;
            } else if end - start == 1 && is_old_heading(&lines[start], lines, start) {
                kind = SpanKind::OldHeading;
            } else {
                kind = SpanKind::Paragraph;
            }
        }
        spans.push(Span { start, end, kind });
        index = end;
    }
    spans
}

fn indented(line: &str) -> bool {
    line.starts_with([' ', '\t'])
}

fn is_heading(line: &str) -> bool {
    line.len() >= 2
        && line.starts_with('#')
        && (line.as_bytes()[1] == b' ' || line.as_bytes()[1] == b'\t')
        && !line.trim().is_empty()
        && line.trim() != "#"
}

fn is_old_heading(line: &str, all: &[String], offset: usize) -> bool {
    if offset == 0 || !all[offset - 1].is_empty() {
        return false;
    }
    if offset + 2 >= all.len()
        || !all[offset + 1].is_empty()
        || !leading_space(&all[offset + 2]).is_empty()
    {
        return false;
    }
    let line = line.trim();
    let Some(first) = line.chars().next() else {
        return false;
    };
    if !first.is_alphabetic() || !first.is_uppercase() {
        return false;
    }
    let Some(last) = line.chars().next_back() else {
        return false;
    };
    if !last.is_alphanumeric() {
        return false;
    }
    if line
        .chars()
        .any(|ch| ";:!?+*/=[]{}_^°&§~%#@<\">\\".contains(ch))
    {
        return false;
    }
    for (index, _) in line.match_indices('\'') {
        let rest = &line[index + 1..];
        if !(rest == "s" || rest.starts_with("s ")) {
            return false;
        }
    }
    for (index, _) in line.match_indices('.') {
        let rest = &line[index + 1..];
        if rest.is_empty() || rest.starts_with(' ') {
            return false;
        }
    }
    true
}

fn list_marker(line: &str) -> Option<(String, &str)> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    let mut chars = line.char_indices();
    let (first_index, first) = chars.next()?;
    let rest;
    let number;
    if matches!(first, '•' | '*' | '+' | '-') {
        number = String::new();
        rest = &line[first_index + first.len_utf8()..];
    } else if first.is_ascii_digit() {
        let mut end = first_index + 1;
        for (index, ch) in chars {
            if !ch.is_ascii_digit() {
                end = index;
                break;
            }
            end = index + 1;
        }
        let ch = line.as_bytes().get(end).copied()?;
        if ch != b'.' && ch != b')' {
            return None;
        }
        number = line[..end].to_owned();
        rest = &line[end + 1..];
    } else {
        return None;
    }
    if !indented(rest) || rest.trim().is_empty() {
        return None;
    }
    Some((number, rest))
}

fn is_list(line: &str) -> bool {
    list_marker(line).is_some()
}

enum Block {
    Paragraph(Vec<Text>),
    Heading(Vec<Text>),
    Code(String),
    List(List),
}

struct List {
    items: Vec<ListItem>,
    blank_between: bool,
}

struct ListItem {
    number: String,
    content: Vec<Vec<Text>>,
}

enum Text {
    Plain(String),
    Link {
        text: Vec<Text>,
        url: String,
    },
    DocLink {
        import_path: String,
        recv: String,
        name: String,
        text: Vec<Text>,
    },
}

struct Parser<'a> {
    links: HashMap<&'a str, &'a str>,
    defined: HashMap<String, String>,
}

impl Parser<'_> {
    /// First pass: block structure with paragraph text still unresolved.
    fn blocks(&mut self, lines: &[String]) -> Vec<Block> {
        let mut result = Vec::new();
        let mut previous_end = 0usize;
        for span in parse_spans(lines) {
            let block = match span.kind {
                SpanKind::List => {
                    let lines = &lines[span.start..span.end];
                    self.list(lines, previous_end < span.start)
                }
                SpanKind::Code => Some(Block::Code(self.code(&lines[span.start..span.end]))),
                SpanKind::OldHeading => Some(Block::Heading(vec![Text::Plain(
                    lines[span.start].trim().to_owned(),
                )])),
                SpanKind::Heading => Some(Block::Heading(vec![Text::Plain(
                    lines[span.start][1..].trim().to_owned(),
                )])),
                SpanKind::Paragraph => self.paragraph(&lines[span.start..span.end]),
            };
            if let Some(block) = block {
                result.push(block);
            }
            previous_end = span.end;
        }
        result
    }

    fn code(&self, lines: &[String]) -> String {
        let body = unindent(&lines.join("\n"));
        if body.is_empty() {
            String::new()
        } else {
            format!("{}\n", body.join("\n"))
        }
    }

    fn paragraph(&mut self, lines: &[String]) -> Option<Block> {
        let mut definitions = Vec::new();
        for line in lines {
            match parse_link(line) {
                Some(definition) => definitions.push(definition),
                None => {
                    return Some(Block::Paragraph(vec![Text::Plain(lines.join("\n"))]));
                }
            }
        }
        for (text, url) in definitions {
            self.defined.entry(text).or_insert(url);
        }
        None
    }

    fn list(&mut self, lines: &[String], force_blank_before: bool) -> Option<Block> {
        let (first_number, _) = list_marker(&lines[0])?;
        let bullet = first_number.is_empty();
        let mut items: Vec<ListItem> = Vec::new();
        let mut blank_between = false;
        let mut text: Vec<String> = Vec::new();
        let flush = |items: &mut Vec<ListItem>, text: &mut Vec<String>| {
            let joined = text.join("\n");
            text.clear();
            if joined.is_empty() {
                return;
            }
            if let Some(item) = items.last_mut() {
                item.content.push(vec![Text::Plain(joined)]);
            }
        };
        for line in lines {
            let mut rest = line.as_str();
            if let Some((marker, after)) = list_marker(line)
                && marker.is_empty() == bullet
            {
                flush(&mut items, &mut text);
                rest = after;
                items.push(ListItem {
                    number: marker,
                    content: Vec::new(),
                });
            }
            let trimmed = rest.trim();
            if trimmed.is_empty() {
                blank_between = true;
                flush(&mut items, &mut text);
                continue;
            }
            text.push(trimmed.to_owned());
        }
        flush(&mut items, &mut text);
        let _ = force_blank_before;
        Some(Block::List(List {
            items,
            blank_between,
        }))
    }

    /// Second pass: interpret links now that all definitions are known.
    fn resolve_links(&mut self, blocks: &mut [Block]) {
        for block in blocks {
            match block {
                Block::Paragraph(text) => {
                    let plain = take_plain(text);
                    *text = self.linked_text(&plain);
                }
                Block::List(list) => {
                    for item in &mut list.items {
                        for content in &mut item.content {
                            let plain = take_plain(content);
                            *content = self.linked_text(&plain);
                        }
                    }
                }
                Block::Heading(_) | Block::Code(_) => {}
            }
        }
    }

    fn linked_text(&self, text: &str) -> Vec<Text> {
        let mut out = Vec::new();
        let mut wrote = 0usize;
        let mut start: Option<usize> = None;
        let mut buffer = String::new();
        for (index, character) in text.char_indices() {
            let character = if character == '\n' || character == '\t' {
                ' '
            } else {
                character
            };
            match character {
                '[' => start = Some(index),
                ']' => {
                    if let Some(begin) = start.take() {
                        if let Some(url) = self
                            .defined
                            .get(buffer.as_str())
                            .map(String::as_str)
                            .or_else(|| self.links.get(buffer.as_str()).copied())
                        {
                            self.flush(&mut out, text, wrote, begin);
                            let mut inner = Vec::new();
                            self.text(&mut inner, &text[begin + 1..index], false);
                            out.push(Text::Link {
                                text: inner,
                                url: (*url).to_owned(),
                            });
                            wrote = index + 1;
                        } else if let Some(mut link) = self.doc_link(
                            &text[begin + 1..index],
                            &text[..begin],
                            &text[index + 1..],
                        ) {
                            self.flush(&mut out, text, wrote, begin);
                            let mut inner = Vec::new();
                            self.text(&mut inner, &text[begin + 1..index], false);
                            if let Text::DocLink { text, .. } = &mut link {
                                *text = inner;
                            }
                            out.push(link);
                            wrote = index + 1;
                        }
                    }
                    buffer.clear();
                }
                _ => {}
            }
            if start.is_some_and(|start| start != index) {
                buffer.push(character);
            }
        }
        self.flush(&mut out, text, wrote, text.len());
        out
    }

    fn flush(&self, out: &mut Vec<Text>, text: &str, wrote: usize, end: usize) {
        if wrote < end {
            self.text(out, &text[wrote..end], true);
        }
    }

    fn doc_link(&self, inner: &str, before: &str, after: &str) -> Option<Text> {
        if let Some(previous) = before.chars().next_back()
            && (previous.is_alphanumeric() || previous == '_')
        {
            return None;
        }
        if let Some(next) = after.chars().next()
            && (next.is_alphanumeric() || next == '_')
        {
            return None;
        }
        let inner = inner.strip_prefix('*').unwrap_or(inner);
        let (first, name, found) = split_doc_name(inner);
        let (package, receiver, name) = if found {
            let (prefix, recv, recv_found) = split_doc_name(first);
            if recv_found && !first.is_empty() {
                (prefix.to_owned(), recv.to_owned(), name.to_owned())
            } else {
                (first.to_owned(), String::new(), name.to_owned())
            }
        } else {
            (inner.to_owned(), String::new(), String::new())
        };
        if package.is_empty() || package_path(&package).is_none() {
            return None;
        }
        Some(Text::DocLink {
            import_path: package,
            recv: receiver,
            name,
            text: Vec::new(),
        })
    }

    /// Parse inline text, converting bare URLs and quote pairs like Go.
    fn text(&self, out: &mut Vec<Text>, input: &str, auto_link: bool) {
        let mut plain = String::new();
        let mut wrote = 0usize;
        let mut index = 0usize;
        while index < input.len() {
            let rest = &input[index..];
            if auto_link && let Some(url) = auto_url(rest) {
                if wrote < index {
                    plain.push_str(&input[wrote..index]);
                }
                if !plain.is_empty() {
                    out.push(Text::Plain(std::mem::take(&mut plain)));
                }
                push_plain(out, &mut plain);
                out.push(Text::Link {
                    text: vec![Text::Plain(url.to_owned())],
                    url: url.to_owned(),
                });
                index += url.len();
                wrote = index;
                continue;
            }
            if auto_link && let Some(id) = ident(rest) {
                index += id.len();
                continue;
            }
            if rest.starts_with("``") {
                if rest.starts_with("```") {
                    let ticks = rest.bytes().take_while(|byte| *byte == b'`').count();
                    index += ticks;
                    continue;
                }
                if wrote < index {
                    plain.push_str(&input[wrote..index]);
                }
                plain.push('\u{201c}');
                index += 2;
                wrote = index;
                continue;
            }
            if rest.starts_with("''") {
                if wrote < index {
                    plain.push_str(&input[wrote..index]);
                }
                plain.push('\u{201d}');
                index += 2;
                wrote = index;
                continue;
            }
            index += input[index..].chars().next().map_or(1, char::len_utf8);
        }
        if wrote < input.len() {
            plain.push_str(&input[wrote..]);
        }
        if !plain.is_empty() {
            out.push(Text::Plain(plain));
        }
    }
}

fn push_plain(out: &mut Vec<Text>, plain: &mut String) {
    if !plain.is_empty() {
        out.push(Text::Plain(std::mem::take(plain)));
    }
}

fn take_plain(text: &mut [Text]) -> String {
    if let Some(Text::Plain(plain)) = text.first_mut() {
        std::mem::take(plain)
    } else {
        String::new()
    }
}

fn split_doc_name(text: &str) -> (&str, &str, bool) {
    let index = text.rfind('.');
    let name = match index {
        Some(index) => &text[index + 1..],
        None => text,
    };
    if !is_name(name) {
        return (text, "", false);
    }
    match index {
        Some(index) => (&text[..index], name, true),
        None => ("", name, true),
    }
}

fn is_name(text: &str) -> bool {
    let Some(first) = text.chars().next() else {
        return false;
    };
    if !first.is_uppercase() {
        return false;
    }
    text.chars().all(|ch| ch.is_alphanumeric() || ch == '_')
}

fn ident(text: &str) -> Option<&str> {
    let mut end = 0usize;
    for (index, ch) in text.char_indices() {
        let ok = ch.is_alphanumeric() || ch == '_' || ch == '\u{2026}';
        if ok || index == 0 && (ch.is_alphabetic() || ch == '_') {
            end = index + ch.len_utf8();
            continue;
        }
        break;
    }
    (end > 0).then(|| &text[..end])
}

fn parse_link(line: &str) -> Option<(String, String)> {
    if line.is_empty() || !line.starts_with('[') {
        return None;
    }
    let index = line.find("]:")?;
    let schemed = line.get(index + 2..)?;
    if !schemed.starts_with(' ') && !schemed.starts_with('\t') {
        return None;
    }
    let text = &line[1..index];
    let url = line[index + 3..].trim();
    let scheme = url.split_once("://")?.0;
    if !is_scheme(scheme) {
        return None;
    }
    Some((text.to_owned(), url.to_owned()))
}

fn is_scheme(text: &str) -> bool {
    matches!(
        text,
        "file" | "ftp" | "gopher" | "http" | "https" | "mailto" | "nntp"
    )
}

/// `go/doc/comment.lookupPkg`: slashed names are full import paths.
fn package_path(package: &str) -> Option<&str> {
    if package.contains('/') {
        valid_import_path(package).then_some(package)
    } else {
        STD_PACKAGES.contains(&package).then_some(package)
    }
}

fn valid_import_path(path: &str) -> bool {
    if path.is_empty() || path.starts_with('-') || path.contains("//") || path.ends_with('/') {
        return false;
    }
    path.split('/').all(|element| {
        !element.is_empty()
            && !element.starts_with('.')
            && !element.ends_with('.')
            && element.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'~' | b'_' | b'+')
            })
    })
}

fn auto_url(text: &str) -> Option<&str> {
    if text.len() < 7 {
        return None;
    }
    let colon = [3usize, 4, 5, 6]
        .into_iter()
        .find(|index| text.as_bytes().get(*index) == Some(&b':'))?;
    if !text[colon..].starts_with("://") {
        return None;
    }
    if !is_scheme(&text[..colon]) {
        return None;
    }
    let mut index = colon + 3;
    let bytes = text.as_bytes();
    if index >= bytes.len() || !is_host(bytes[index]) || is_punct(bytes[index]) {
        return None;
    }
    index += 1;
    let mut end = index;
    while index < bytes.len() && is_host(bytes[index]) {
        if !is_punct(bytes[index]) {
            end = index + 1;
        }
        index += 1;
    }
    let mut stack: Vec<u8> = Vec::new();
    while index < bytes.len() {
        if is_punct(bytes[index]) {
            index += 1;
            continue;
        }
        if !is_path(bytes[index]) {
            break;
        }
        match bytes[index] {
            b'(' => stack.push(b')'),
            b'{' => stack.push(b'}'),
            b'[' => stack.push(b']'),
            close @ (b')' | b'}' | b']') => {
                if stack.last() != Some(&close) {
                    break;
                }
                stack.pop();
            }
            _ => {}
        }
        if stack.is_empty() {
            end = index + 1;
        }
        index += 1;
    }
    Some(&text[..end])
}

fn is_host(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'@' | b'-' | b'.' | b'[' | b']' | b':')
}

fn is_punct(byte: u8) -> bool {
    matches!(byte, b'.' | b',' | b':' | b';' | b'?' | b'!')
}

fn is_path(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'$' | b'\''
                | b'('
                | b')'
                | b'*'
                | b'+'
                | b'&'
                | b'#'
                | b'='
                | b'@'
                | b'~'
                | b'_'
                | b'/'
                | b'-'
                | b'['
                | b']'
                | b'{'
                | b'}'
                | b'%'
        )
}

struct Printer {
    heading_prefix: &'static str,
    out: String,
}

impl Printer {
    fn doc(&mut self, blocks: &[Block]) {
        for (index, block) in blocks.iter().enumerate() {
            if index > 0 {
                self.out.push('\n');
            }
            self.block(block);
        }
    }

    fn block(&mut self, block: &Block) {
        match block {
            Block::Paragraph(text) => {
                self.text(text);
                self.out.push('\n');
            }
            Block::Heading(text) => {
                self.out.push_str(self.heading_prefix);
                self.text(text);
                self.out.push('\n');
            }
            Block::Code(text) => {
                let mut rest = text.as_str();
                while !rest.is_empty() {
                    let (line, remaining) = match rest.split_once('\n') {
                        Some((line, remaining)) => (line, remaining),
                        None => (rest, ""),
                    };
                    if !line.is_empty() {
                        self.out.push('\t');
                        self.out.push_str(line);
                    }
                    self.out.push('\n');
                    rest = remaining;
                }
            }
            Block::List(list) => {
                for (index, item) in list.items.iter().enumerate() {
                    if index > 0 && list.blank_between {
                        self.out.push('\n');
                    }
                    if !item.number.is_empty() {
                        self.out.push(' ');
                        self.out.push_str(&item.number);
                        self.out.push_str(". ");
                    } else {
                        self.out.push_str("  - ");
                    }
                    for (position, content) in item.content.iter().enumerate() {
                        if position > 0 {
                            self.out.push_str("\n    ");
                        }
                        self.text(content);
                        self.out.push('\n');
                    }
                }
            }
        }
    }

    fn text(&mut self, text: &[Text]) {
        let mut raw = String::new();
        self.raw_text(&mut raw, text);
        let line = raw.trim();
        if line.is_empty() {
            return;
        }
        let bytes = line.as_bytes();
        match bytes[0] {
            b'+' | b'-' | b'*' | b'#' => self.out.push('\\'),
            b'0'..=b'9' => {
                let mut index = 1;
                while index < bytes.len() && bytes[index].is_ascii_digit() {
                    index += 1;
                }
                if index < bytes.len() && (bytes[index] == b'.' || bytes[index] == b')') {
                    self.out.push_str(&line[..index]);
                    self.out.push('\\');
                    self.out.push_str(&line[index..]);
                    return;
                }
            }
            _ => {}
        }
        self.out.push_str(line);
    }

    fn raw_text(&mut self, out: &mut String, text: &[Text]) {
        for item in text {
            match item {
                Text::Plain(plain) => escape(out, plain),
                Text::Link { text, url } => {
                    out.push('[');
                    self.raw_text(out, text);
                    out.push_str("](");
                    out.push_str(url);
                    out.push(')');
                }
                Text::DocLink {
                    import_path,
                    recv,
                    name,
                    text,
                } => {
                    let mut url = format!("https://pkg.go.dev/{import_path}");
                    if !name.is_empty() {
                        url.push('#');
                        if !recv.is_empty() {
                            url.push_str(recv);
                            url.push('.');
                        }
                        url.push_str(name);
                    }
                    if !url.is_empty() {
                        out.push('[');
                    }
                    self.raw_text(out, text);
                    if !url.is_empty() {
                        out.push_str("](");
                        out.push_str(&url.replace('(', "%28").replace(')', "%29"));
                        out.push(')');
                    }
                }
            }
        }
    }
}

fn escape(out: &mut String, text: &str) {
    let mut start = 0usize;
    for (index, byte) in text.bytes().enumerate() {
        match byte {
            b'\n' => {
                out.push_str(&text[start..index]);
                out.push(' ');
                start = index + 1;
            }
            b'`' | b'_' | b'*' | b'[' | b'<' | b'\\' => {
                out.push_str(&text[start..index]);
                out.push('\\');
                out.push(byte as char);
                start = index + 1;
            }
            _ => {}
        }
    }
    out.push_str(&text[start..]);
}
