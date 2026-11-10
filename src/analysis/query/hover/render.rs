//! Native descriptions for lexical bindings, fields and methods.

/// gopls only attaches assignment docs to single-name short declarations.
fn multi_name_short_declaration(source: &str, offset: usize) -> bool {
    let begin = source
        .get(..offset)
        .and_then(|prefix| prefix.rfind('\n'))
        .map_or(0, |index| index + 1);
    let end = source
        .get(offset..)
        .and_then(|rest| rest.find('\n'))
        .map_or(source.len(), |index| offset + index);
    let Some(line) = source.get(begin..end) else {
        return false;
    };
    let Some((before, _)) = line.split_once(":=") else {
        return false;
    };
    let lhs = before
        .trim()
        .strip_prefix("if ")
        .or_else(|| before.trim().strip_prefix("for "))
        .or_else(|| before.trim().strip_prefix("switch "))
        .unwrap_or(before.trim());
    lhs.matches(',').count() > 0
}

/// Whether `offset` is inside a function/method declaration header.
fn in_function_header(context: &Context<'_>, offset: usize) -> bool {
    context
        .workspace()
        .files
        .get(context.file())
        .is_some_and(|stub| {
            stub.symbols.iter().any(|symbol| {
                symbol
                    .body
                    .as_ref()
                    .zip(symbol.declaration())
                    .is_some_and(|(body, declaration)| {
                        declaration.start <= offset && offset < body.start
                    })
            })
        })
}

/// Start of the struct field declaration containing relative `selection`.
pub(super) fn field_start(declaration: &str, selection: usize) -> Option<usize> {
    let prefix = declaration.get(..selection)?;
    let boundary = prefix.rfind(['{', '\n']).map_or(0, |index| index + 1);
    prefix
        .get(boundary..)?
        .char_indices()
        .find_map(|(index, ch)| (ch != ' ' && ch != '\t').then_some(boundary + index))
}

/// First non-blank token offset of the line containing `offset`.
pub(super) fn line_start_token(source: &str, offset: usize) -> usize {
    let begin = source
        .get(..offset)
        .and_then(|prefix| prefix.rfind('\n'))
        .map_or(0, |index| index + 1);
    let Some(rest) = source.get(begin..offset) else {
        return offset;
    };
    match rest
        .char_indices()
        .find(|(_, ch)| *ch != ' ' && *ch != '\t')
    {
        Some((index, _)) => begin + index,
        None => offset,
    }
}

/// Requalified type text normalized to gopls/`types.TypeString` presentation.
pub(super) fn type_text(context: &Context<'_>, origin: &Path, annotation: &str) -> Option<String> {
    if let Some(alias) = alias_text(annotation) {
        return Some(alias.to_owned());
    }
    context
        .workspace()
        .signature_type(context.file(), origin, annotation)
        .map(|text| normalize_type(&text))
}

/// Universe aliases keep the source spelling in `types.TypeString`.
pub(super) fn alias_text(annotation: &str) -> Option<&'static str> {
    match annotation.trim() {
        "any" => Some("any"),
        "byte" => Some("byte"),
        "rune" => Some("rune"),
        _ => None,
    }
}

/// Effective alias of an import, including the path suffix fallback.
fn import_alias(workspace: &Workspace, import: &crate::syntax::Import) -> Option<String> {
    if let Some(name) = workspace.import_name(import) {
        return Some(name.to_owned());
    }
    import
        .explicit_name()
        .map(str::to_owned)
        .or_else(|| import.path.rsplit('/').next().map(str::to_owned))
}

/// Whether an import alias resolves to a package that gopls could load.
///
/// Indexed packages resolve directly. Otherwise the module mount directory is
/// inspected for a host-active, non-cgo Go file: cgo-disabled contexts treat
/// cgo-only packages as unresolvable, exactly like go/types' invalid imports.
fn import_resolves(workspace: &Workspace, alias_file: &Path, alias: &str) -> bool {
    if workspace
        .imported_symbol_iter(alias_file, alias)
        .next()
        .is_some()
    {
        return true;
    }
    let Some(stub) = workspace.files.get(alias_file) else {
        return true;
    };
    let Some(import) = stub
        .imports
        .iter()
        .find(|import| import_alias(workspace, import).as_deref() == Some(alias))
    else {
        return true;
    };
    let mut directories = Vec::new();
    for (root, module) in &workspace.modules {
        let suffix = if module == "std" {
            Some(import.path.clone())
        } else if import.path == *module {
            Some(String::new())
        } else {
            import
                .path
                .strip_prefix(module.as_str())
                .and_then(|rest| rest.strip_prefix('/'))
                .map(str::to_owned)
        };
        if let Some(suffix) = suffix {
            directories.push(if suffix.is_empty() {
                root.clone()
            } else {
                root.join(&suffix)
            });
        }
    }
    for directory in directories {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.take(256).filter_map(Result::ok) {
            let path = entry.path();
            if path.extension().is_none_or(|extension| extension != "go") {
                continue;
            }
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if !host_file_active(name) {
                continue;
            }
            let Ok(source) = std::fs::read_to_string(&path) else {
                continue;
            };
            if source.get(..4 * 1024).is_some_and(|head| {
                head.contains("import \"C\"") || head.contains("import _ \"C\"")
            }) {
                continue;
            }
            return true;
        }
    }
    false
}

const KNOWN_GOOS: &[&str] = &[
    "aix",
    "android",
    "darwin",
    "dragonfly",
    "freebsd",
    "hurd",
    "illumos",
    "ios",
    "js",
    "linux",
    "netbsd",
    "openbsd",
    "plan9",
    "solaris",
    "wasip1",
    "windows",
    "zos",
];
const KNOWN_GOARCH: &[&str] = &[
    "386", "amd64", "amd64p32", "arm", "arm64", "arm64be", "armbe", "loong64", "mips", "mipsle",
    "mips64", "mips64le", "ppc64", "ppc64le", "riscv64", "s390x", "sparc64", "wasm",
];

/// Conservative host build-context filename filter (GOOS/GOARCH suffixes).
fn host_file_active(name: &str) -> bool {
    let stem = name.strip_suffix(".go").unwrap_or(name);
    let host_goos = if cfg!(windows) { "windows" } else { "linux" };
    let host_goarch = if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "amd64"
    };
    let mut parts: Vec<&str> = stem.rsplit('_').take(2).collect();
    parts.reverse();
    match parts.len() {
        2 => {
            let (goos, goarch) = (parts[0], parts[1]);
            if KNOWN_GOOS.contains(&goos) && KNOWN_GOARCH.contains(&goarch) {
                return goos == host_goos && goarch == host_goarch;
            }
            if KNOWN_GOOS.contains(&goarch) {
                return goarch == host_goos;
            }
            true
        }
        1 => {
            if KNOWN_GOOS.contains(&parts[0]) {
                return parts[0] == host_goos;
            }
            if KNOWN_GOARCH.contains(&parts[0]) {
                return parts[0] == host_goarch;
            }
            true
        }
        _ => true,
    }
}

/// Render a type/signature text with go/types' invalid-type replacement.
pub(super) fn replace_invalid_qualified(
    workspace: &Workspace,
    alias_file: &Path,
    text: &str,
) -> String {
    let Some(stub) = workspace.files.get(alias_file) else {
        return text.to_owned();
    };
    let unresolved: Vec<String> = stub
        .imports
        .iter()
        .filter(|import| !import.is_dot() && !import.is_blank())
        .filter_map(|import| import_alias(workspace, import))
        .filter(|name| !import_resolves(workspace, alias_file, name))
        .collect();
    if unresolved.is_empty() {
        return text.to_owned();
    }
    let mut output = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        let mut matched = None;
        for alias in &unresolved {
            let star = bytes[index] == b'*';
            let base = if star { index + 1 } else { index };
            let previous_word = star
                && bytes
                    .get(index.wrapping_sub(1))
                    .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_');
            let Some(rest) = text.get(base..) else {
                continue;
            };
            if !rest.starts_with(alias.as_str()) || previous_word {
                continue;
            }
            let mut cursor = base + alias.len();
            if bytes.get(cursor) != Some(&b'.') {
                continue;
            }
            cursor += 1;
            if !bytes
                .get(cursor)
                .is_some_and(|byte| byte.is_ascii_alphabetic() || *byte == b'_')
            {
                continue;
            }
            cursor += 1;
            while cursor < bytes.len()
                && (bytes[cursor].is_ascii_alphanumeric() || bytes[cursor] == b'_')
            {
                cursor += 1;
            }
            if bytes.get(cursor) == Some(&b'[')
                && let Some(close) = text[cursor..].find(']')
            {
                cursor += close + 1;
            }
            matched = Some(cursor);
            break;
        }
        if let Some(end) = matched {
            output.push_str("invalid type");
            index = end;
            continue;
        }
        let ch = text[index..].chars().next().unwrap_or('\0');
        output.push(ch);
        index += ch.len_utf8();
    }
    output
}

/// `types.TypeString`-style anonymous aggregate spacing and separator trimming.
pub(super) fn normalize_type(text: &str) -> String {
    let text = strip_comments(text);
    let text = canonical_multiline_aggregates(&text);
    let mut output = collapse_whitespace(&text);
    output = output
        .replace("struct {", "struct{")
        .replace("struct{ ", "struct{")
        .replace("interface {", "interface{")
        .replace("interface{ ", "interface{");
    while output.contains("; }") {
        output = output.replace("; }", "}");
    }
    output = output.replace(" }", "}");
    output = canonical_array_lengths(&output);
    output = canonical_tags(&output);
    output = expand_grouped_fields(&output);
    output.trim().to_owned()
}

/// `types.TypeString` expands grouped struct/interface field names.
fn expand_grouped_fields(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut output = String::with_capacity(text.len());
    let mut index = 0usize;
    while index < bytes.len() {
        let aggregate = text[index..]
            .find("struct{")
            .map(|position| (index + position, "struct{"))
            .into_iter()
            .chain(
                text[index..]
                    .find("interface{")
                    .map(|position| (index + position, "interface{")),
            )
            .min_by_key(|(position, _)| *position);
        let Some((start, keyword)) = aggregate else {
            output.push_str(&text[index..]);
            break;
        };
        output.push_str(&text[index..start]);
        let open = start + keyword.len() - 1;
        let Some(close) = matching_brace(text, open) else {
            output.push_str(&text[start..]);
            break;
        };
        output.push_str(keyword);
        let body = &text[open + 1..close];
        let segments: Vec<&str> = split_top_level(body, ';');
        let mut expanded = Vec::new();
        for segment in segments {
            expand_grouped_segment(segment, &mut expanded);
        }
        output.push_str(&expanded.join("; "));
        output.push('}');
        index = close + 1;
    }
    output
}

fn expand_grouped_segment(segment: &str, out: &mut Vec<String>) {
    let trimmed = segment.trim();
    let tokens: Vec<&str> = trimmed.split(' ').filter(|part| !part.is_empty()).collect();
    if tokens.len() < 2 {
        out.push(trimmed.to_owned());
        return;
    }
    let mut names = Vec::new();
    let mut cursor = 0usize;
    while cursor + 1 < tokens.len() {
        let candidate = tokens[cursor].trim_end_matches(',');
        if !candidate
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_alphabetic() || ch == '_')
            || !candidate
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        {
            break;
        }
        if tokens[cursor].ends_with(',') {
            names.push(candidate);
            cursor += 1;
            continue;
        }
        names.push(candidate);
        cursor += 1;
        break;
    }
    if names.len() < 2 {
        out.push(trimmed.to_owned());
        return;
    }
    let rest = tokens[cursor..].join(" ");
    if rest.is_empty() {
        out.push(trimmed.to_owned());
        return;
    }
    for name in names {
        out.push(format!("{name} {rest}"));
    }
}

/// Remove comments from a rendered type expression; `types.TypeString` has none.
fn strip_comments(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut output = String::with_capacity(text.len());
    let mut index = 0usize;
    while index < bytes.len() {
        match bytes[index] {
            b'`' => {
                let Some(close) = text[index + 1..].find('`') else {
                    output.push_str(&text[index..]);
                    break;
                };
                output.push_str(&text[index..index + close + 2]);
                index += close + 2;
            }
            b'"' => {
                let mut end = index + 1;
                while end < bytes.len() {
                    match bytes[end] {
                        b'\\' => end += 2,
                        b'"' => break,
                        _ => end += 1,
                    }
                }
                let end = (end + 1).min(bytes.len());
                output.push_str(&text[index..end]);
                index = end;
            }
            b'/' if bytes.get(index + 1) == Some(&b'/') => {
                index += 2;
                while index < bytes.len() && bytes[index] != b'\n' {
                    index += 1;
                }
            }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                index += 2;
                while index + 1 < bytes.len() && !(bytes[index] == b'*' && bytes[index + 1] == b'/')
                {
                    index += 1;
                }
                index = (index + 2).min(bytes.len());
            }
            _ => {
                let ch = text[index..].chars().next().unwrap_or('\0');
                output.push(ch);
                index += ch.len_utf8();
            }
        }
    }
    output
}

/// `types.TypeString` renders struct tags as interpreted Go strings.
fn canonical_tags(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut output = String::with_capacity(text.len());
    let mut index = 0usize;
    while index < bytes.len() {
        match bytes[index] {
            b'`' => {
                let Some(close) = text[index + 1..].find('`') else {
                    output.push_str(&text[index..]);
                    break;
                };
                let content = text[index + 1..index + 1 + close].replace('\r', "");
                output.push_str(&super::value::quote(&content));
                index += close + 2;
            }
            b'"' => {
                let mut end = index + 1;
                while end < bytes.len() {
                    match bytes[end] {
                        b'\\' => end += 2,
                        b'"' => break,
                        _ => end += 1,
                    }
                }
                let Some(literal) = text.get(index..=end) else {
                    output.push_str(&text[index..]);
                    break;
                };
                match crate::syntax::decode_string_literal(literal) {
                    Some(decoded) => output.push_str(&super::value::quote_bytes(&decoded)),
                    None => output.push_str(literal),
                }
                index = end + 1;
            }
            _ => {
                let ch = text[index..].chars().next().unwrap_or('\0');
                output.push(ch);
                index += ch.len_utf8();
            }
        }
    }
    output
}

/// Rewrite multiline struct/interface bodies as single-line `;`-separated ones.
fn canonical_multiline_aggregates(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut output = String::with_capacity(text.len());
    let mut index = 0usize;
    while index < bytes.len() {
        if text[index..].starts_with("struct")
            && bytes
                .get(index + 6)
                .is_none_or(|byte| !byte.is_ascii_alphanumeric() && *byte != b'_')
        {
            let mut open = index + 6;
            while bytes.get(open) == Some(&b' ') {
                open += 1;
            }
            if bytes.get(open) == Some(&b'{')
                && let Some(close) = matching_brace(text, open)
            {
                let body = &text[open + 1..close];
                output.push_str("struct{");
                output.push_str(&aggregate_body(&canonical_multiline_aggregates(body)));
                output.push('}');
                index = close + 1;
                continue;
            }
        }
        if text[index..].starts_with("interface")
            && bytes
                .get(index + 9)
                .is_none_or(|byte| !byte.is_ascii_alphanumeric() && *byte != b'_')
        {
            let mut open = index + 9;
            while bytes.get(open) == Some(&b' ') {
                open += 1;
            }
            if bytes.get(open) == Some(&b'{')
                && let Some(close) = matching_brace(text, open)
            {
                let body = &text[open + 1..close];
                output.push_str("interface{");
                output.push_str(&aggregate_body(&canonical_multiline_aggregates(body)));
                output.push('}');
                index = close + 1;
                continue;
            }
        }
        let ch = text[index..].chars().next().unwrap_or('\0');
        output.push(ch);
        index += ch.len_utf8();
    }
    output
}

fn aggregate_body(body: &str) -> String {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let parts: Vec<String> = trimmed
        .split('\n')
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| line.trim_end_matches(';').trim_end().to_owned())
        .collect();
    if parts.len() <= 1 {
        return trimmed.trim_end_matches(';').trim_end().to_owned();
    }
    parts.join("; ")
}

fn matching_brace(text: &str, open: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for (offset, ch) in text[open..].char_indices() {
        if let Some(active) = quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' && active != '`' {
                escaped = true;
            } else if ch == active {
                quote = None;
            }
            continue;
        }
        match ch {
            '"' | '\'' | '`' => quote = Some(ch),
            '{' => depth += 1,
            '}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(open + offset);
                }
            }
            _ => {}
        }
    }
    None
}

/// `types.TypeString` prints array lengths as decimal constants.
fn canonical_array_lengths(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] == b'['
            && let Some(close) = text[index + 1..].find(']')
        {
            let inner = &text[index + 1..index + 1 + close];
            if let Some(value) = array_length(inner) {
                output.push('[');
                output.push_str(&value.to_string());
                output.push(']');
                index += close + 2;
                continue;
            }
        }
        let ch = text[index..].chars().next().unwrap_or('\0');
        output.push(ch);
        index += ch.len_utf8();
    }
    output
}

fn array_length(text: &str) -> Option<i64> {
    let text = text.trim();
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
        return None;
    }
    let (digits, radix) =
        if let Some(rest) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
            (rest, 16)
        } else if let Some(rest) = text.strip_prefix("0b").or_else(|| text.strip_prefix("0B")) {
            (rest, 2)
        } else if let Some(rest) = text.strip_prefix("0o").or_else(|| text.strip_prefix("0O")) {
            (rest, 8)
        } else if text.len() > 1 && text.starts_with('0') {
            (&text[1..], 8)
        } else if text.bytes().all(|byte| byte.is_ascii_digit()) {
            (text, 10)
        } else {
            return None;
        };
    i64::from_str_radix(&digits.replace('_', ""), radix).ok()
}

/// Collapse source line breaks/tabs outside string and rune literals.
fn collapse_whitespace(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut quote: Option<char> = None;
    let mut pending_space = false;
    for ch in text.chars() {
        if let Some(active) = quote {
            output.push(ch);
            if ch == active {
                quote = None;
            }
            continue;
        }
        match ch {
            '"' | '\'' | '`' => {
                if pending_space && !output.is_empty() {
                    output.push(' ');
                }
                pending_space = false;
                quote = Some(ch);
                output.push(ch);
            }
            ch if ch.is_whitespace() => pending_space = true,
            ch => {
                if pending_space && !output.is_empty() {
                    output.push(' ');
                }
                pending_space = false;
                output.push(ch);
            }
        }
    }
    output
}

/// Expand grouped parameter names inside function types like go/types does.
pub(super) fn normalize_signature(text: &str) -> String {
    normalize_type(&expand_function_types(text))
}

fn expand_function_types(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        if text[index..].starts_with("func(") {
            let open = index + 4;
            if let Some(close) = matching_paren(text, open) {
                output.push_str("func(");
                output.push_str(&expand_parameter_list(&text[open + 1..close]));
                output.push(')');
                index = close + 1;
                continue;
            }
        }
        let ch = text[index..].chars().next().unwrap_or('\0');
        output.push(ch);
        index += ch.len_utf8();
    }
    output
}

fn matching_paren(text: &str, open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (offset, ch) in text[open..].char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(open + offset);
                }
            }
            _ => {}
        }
    }
    None
}

fn expand_parameter_list(inner: &str) -> String {
    let segments = split_top_level(inner, ',');
    let typed = segments
        .iter()
        .any(|segment| segment.split_whitespace().count() > 1);
    if !typed {
        return inner.to_owned();
    }
    let mut output: Vec<String> = Vec::new();
    let mut pending: Vec<String> = Vec::new();
    for segment in segments {
        let segment = segment.trim();
        if segment.is_empty() {
            continue;
        }
        if simple_identifier(segment) {
            pending.push(segment.to_owned());
            continue;
        }
        let (name, ty) = match segment.split_once(char::is_whitespace) {
            Some((name, ty)) if simple_identifier(name) && !ty.trim().is_empty() => {
                (Some(name), ty.trim())
            }
            _ => (None, segment),
        };
        let ty = expand_function_types(ty);
        for pending_name in pending.drain(..) {
            output.push(format!("{pending_name} {ty}"));
        }
        match name {
            Some(name) => output.push(format!("{name} {ty}")),
            None => output.push(ty),
        }
    }
    for pending_name in pending.drain(..) {
        output.push(pending_name);
    }
    output.join(", ")
}

fn simple_identifier(text: &str) -> bool {
    let mut chars = text.chars();
    chars
        .next()
        .is_some_and(|first| first.is_alphabetic() || first == '_')
        && chars.all(|ch| ch.is_alphanumeric() || ch == '_')
}

fn split_top_level(text: &str, separator: char) -> Vec<&str> {
    let mut segments = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    for (index, ch) in text.char_indices() {
        match ch {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            ch if ch == separator && depth == 0 => {
                segments.push(&text[start..index]);
                start = index + ch.len_utf8();
            }
            _ => {}
        }
    }
    segments.push(&text[start..]);
    segments
}

use super::content::HoverContent;
use super::{HoverLink, doc};
use crate::analysis::check;
use crate::analysis::member::MemberDeclaration;
use crate::analysis::query::Context;
use crate::analysis::scope::{Binding, DeclarationKind, Locals, ObjectKind};
use crate::syntax::TokenKind;
use crate::text::SourceMap;
use crate::workspace::Workspace;
use std::ops::Range;
use std::path::Path;

const MAX_SIGNATURE_BYTES: usize = 16 * 1024;

pub(super) enum SourceText<'a> {
    Borrowed(&'a str),
    Owned(String),
}

impl SourceText<'_> {
    pub(super) fn text(&self) -> &str {
        match self {
            Self::Borrowed(text) => text,
            Self::Owned(text) => text,
        }
    }
}

/// One bounded read of a declaration file, borrowing the current document.
pub(super) fn declaration_source<'a>(
    context: &'a Context<'_>,
    file: &Path,
) -> Option<SourceText<'a>> {
    if file == context.file() {
        Some(SourceText::Borrowed(context.source()))
    } else {
        context.workspace().source(file).map(SourceText::Owned)
    }
}

/// Rendered type text for one lexical binding, preferring native inference.
pub(super) fn binding_text(
    workspace: &Workspace,
    file: &Path,
    source: &str,
    locals: &Locals,
    binding: &Binding,
) -> Option<String> {
    if binding.declaration == DeclarationKind::TypeParameter {
        return locals
            .type_parameter_constraint(binding)
            .and_then(|span| source.get(span.clone()))
            .and_then(|text| workspace.signature_type(file, file, text))
            .or_else(|| binding.declared_type.clone());
    }
    let native = check::binding_detail(workspace, file, source, locals, binding)
        .or_else(|| workspace.binding_type(file, source, locals, binding))
        .map(|text| normalize_type(&text));
    if binding.declaration == DeclarationKind::Variadic {
        return native.map(|text| {
            if text.trim_start().starts_with("[]") {
                text
            } else {
                format!("[]{text}")
            }
        });
    }
    let written = binding
        .annotation()
        .and_then(|span| source.get(span.clone()))
        .map(|annotation| {
            if let Some(alias) = alias_text(annotation) {
                return alias.to_owned();
            }
            workspace
                .signature_type(file, file, annotation)
                .map(|text| normalize_signature(&text))
                .unwrap_or_default()
        })
        .filter(|text| !text.is_empty());
    match binding.declaration {
        DeclarationKind::Value | DeclarationKind::Constant => written.or(native),
        _ => native.or(written),
    }
}

/// Type of a binding inferred from its initializer expression, including field
/// and index chains and calls to closed same-directory test-file functions.
pub(super) fn binding_initializer_type(
    context: &Context<'_>,
    source: &str,
    locals: &Locals,
    binding: &Binding,
    depth: usize,
) -> Option<String> {
    if depth >= 8 {
        return None;
    }
    let workspace = context.workspace();
    if let Some(declared) = binding.declared_type.as_ref() {
        return Some(normalize_type(declared));
    }
    if let Some(annotation) = binding
        .annotation()
        .and_then(|span| source.get(span.clone()))
    {
        let annotation = annotation.trim();
        if !annotation.is_empty() {
            return Some(normalize_type(annotation));
        }
    }
    if let Some(rendered) = binding_text(workspace, context.file(), source, locals, binding) {
        return Some(rendered);
    }
    if let Some((span, result, count)) = binding.initializer()
        && count == 1
        && result == 0
        && span.len() <= MAX_SIGNATURE_BYTES
    {
        let text = source.get(span.clone())?;
        return initializer_expression_type(context, source, locals, text, span.start, depth + 1);
    }
    if let Some(span) = syntactic_initializer(source, binding)
        && span.len() <= MAX_SIGNATURE_BYTES
    {
        let text = source.get(span.clone())?;
        return initializer_expression_type(context, source, locals, text, span.start, depth + 1);
    }
    let (span, slot, _) = binding.range_initializer()?;
    let text = source.get(span.clone())?;
    let ty = initializer_expression_type(context, source, locals, text, span.start, depth + 1)?;
    range_slot_type(&ty, slot)
}

/// Initializer span reconstructed from the declaration line when the cursor
/// scope does not retain an `Initializer` record (e.g. closure-local short
/// declarations).
fn syntactic_initializer(source: &str, binding: &Binding) -> Option<Range<usize>> {
    let rest = source.get(binding.selection.end..)?;
    let equals = rest.find('=')?;
    if equals > 24 {
        return None;
    }
    let start = binding.selection.end + equals + 1;
    let after = source.get(start..)?;
    let trimmed = after.trim_start();
    let offset = start + (after.len() - trimmed.len());
    let end = trimmed
        .find(['\n', ';'])
        .map_or(source.len(), |end| offset + end);
    let text = source.get(offset..end)?.trim();
    if text.is_empty() || text.contains("==") || text.starts_with("range ") {
        return None;
    }
    let text_end = offset + text.len();
    Some(offset..text_end)
}

/// Field type of a locally declared named struct type.
fn local_struct_field_type(source: &str, type_name: &str, field: &str) -> Option<String> {
    let tokens: Vec<_> = crate::syntax::Lexer::new(source)
        .filter(|token| !token.inserted)
        .take(200_000)
        .collect();
    for (index, token) in tokens.iter().enumerate() {
        if token.kind != TokenKind::Type {
            continue;
        }
        let Some(name) = tokens
            .get(index + 1)
            .filter(|token| token.kind == TokenKind::Ident)
        else {
            continue;
        };
        if source.get(name.span.clone())? != type_name {
            continue;
        }
        let Some(keyword) = tokens
            .get(index + 2)
            .filter(|token| token.kind == TokenKind::Struct)
        else {
            continue;
        };
        let Some(brace) = tokens
            .get(index + 3)
            .filter(|token| token.kind == TokenKind::LBrace)
        else {
            continue;
        };
        let close = matching_brace(source, brace.span.start)?;
        let text = source.get(keyword.span.start..=close)?;
        for member in crate::syntax::aggregate_members(text) {
            if member.symbol.kind == crate::syntax::MemberKind::Field && member.symbol.name == field
            {
                let annotation = text.get(member.annotation)?;
                return Some(normalize_type(annotation));
            }
        }
    }
    None
}

/// Nearest preceding anonymous struct field declaration with this name.
pub(super) fn anonymous_field_near(
    context: &Context<'_>,
    offset: usize,
    name: &str,
) -> Option<String> {
    let source = context.source();
    let tokens: Vec<_> = crate::syntax::Lexer::new(source)
        .filter(|token| !token.inserted)
        .take(200_000)
        .collect();
    let mut best: Option<String> = None;
    for (index, token) in tokens.iter().enumerate() {
        if token.kind != TokenKind::Struct || token.span.start >= offset {
            continue;
        }
        let Some(brace) = tokens
            .get(index + 1)
            .filter(|token| token.kind == TokenKind::LBrace)
        else {
            continue;
        };
        let Some(close) = matching_brace(source, brace.span.start) else {
            continue;
        };
        let Some(text) = source.get(token.span.start..=close) else {
            continue;
        };
        let entries = crate::syntax::aggregate_members(text);
        let Some(entry) = entries.iter().find(|entry| {
            entry.symbol.kind == crate::syntax::MemberKind::Field && entry.symbol.name == name
        }) else {
            continue;
        };
        let Some(annotation) = text.get(entry.annotation.clone()) else {
            continue;
        };
        let workspace = context.workspace();
        let ty = alias_text(annotation)
            .map(str::to_owned)
            .or_else(|| workspace.signature_type(context.file(), context.file(), annotation))
            .map(|text| normalize_type(&text))
            .unwrap_or_else(|| normalize_type(annotation));
        best = Some(format!("field {name} {ty}"));
    }
    best
}

/// Field of an anonymous struct type spelling (for selector access).
pub(super) fn anonymous_struct_member(
    context: &Context<'_>,
    receiver: &str,
    name: &str,
    range: super::SourceRange,
) -> Option<HoverContent> {
    let start = receiver.find("struct{")?;
    let text = &receiver[start..];
    let entries = crate::syntax::aggregate_members(text);
    let entry = entries.iter().find(|entry| {
        entry.symbol.kind == crate::syntax::MemberKind::Field && entry.symbol.name == name
    })?;
    let annotation = text.get(entry.annotation.clone())?;
    let workspace = context.workspace();
    let ty = alias_text(annotation)
        .map(str::to_owned)
        .or_else(|| workspace.signature_type(context.file(), context.file(), annotation))
        .map(|text| normalize_type(&text))
        .unwrap_or_else(|| normalize_type(annotation));
    let mut content = HoverContent::new(range);
    content.code = Some(format!("field {name} {ty}"));
    Some(content)
}

/// Iteration slot type for a range operand spelling.
fn range_slot_type(ty: &str, slot: usize) -> Option<String> {
    let mut ty = ty.trim();
    while let Some(rest) = ty.strip_prefix('*') {
        ty = rest.trim();
    }
    if ty.starts_with("map[")
        && let Some(close) = matching_brace(ty, 3)
    {
        let key = normalize_type(&ty[4..close]);
        if slot == 0 {
            return Some(key);
        }
        return Some(normalize_type(&ty[close + 1..]));
    }
    if let Some(rest) = ty.strip_prefix("<-chan ") {
        return Some(normalize_type(rest));
    }
    if let Some(rest) = ty.strip_prefix("chan ") {
        return Some(normalize_type(rest));
    }
    indexed_element_type(ty)
}

fn initializer_expression_type(
    context: &Context<'_>,
    source: &str,
    locals: &Locals,
    text: &str,
    base: usize,
    depth: usize,
) -> Option<String> {
    if depth >= 8 {
        return None;
    }
    let parsed = crate::syntax::parse_expression(text);
    if !parsed.diagnostics.is_empty() {
        return None;
    }
    let expression = parsed.nodes.get(parsed.root.0)?;
    match &expression.kind {
        crate::syntax::ExprKind::Parenthesized { expression: inner } => {
            let inner = parsed.nodes.get(inner.0)?;
            initializer_expression_type(
                context,
                source,
                locals,
                text.get(inner.span.clone())?,
                base + inner.span.start,
                depth + 1,
            )
        }
        crate::syntax::ExprKind::Name => {
            let name = text.get(expression.span.clone())?;
            let binding = locals.resolve_at(base + expression.span.start, name);
            let binding = binding?;
            binding_initializer_type(context, source, locals, binding, depth + 1)
        }
        crate::syntax::ExprKind::Selector { operand, field } => {
            let operand_node = parsed.nodes.get(operand.0)?;
            let operand_text = text.get(operand_node.span.clone())?;
            let operand_type = initializer_expression_type(
                context,
                source,
                locals,
                operand_text,
                base + operand_node.span.start,
                depth + 1,
            )?;
            let field = text.get(field.clone())?;
            let workspace = context.workspace();
            let member = workspace.scoped_receiver_member(
                context.file(),
                context.file(),
                &operand_type,
                field,
            );
            if let Some(member) = member {
                return match member {
                    crate::analysis::method_set::ReceiverMember::Declaration(declaration) => {
                        let data = workspace.member_data(declaration)?;
                        let annotation = data.annotation?;
                        type_text(context, declaration.file, &annotation)
                    }
                    crate::analysis::method_set::ReceiverMember::Method(..) => None,
                };
            }
            local_struct_field_type(source, &operand_type, field)
        }
        crate::syntax::ExprKind::Index { operand, .. } => {
            let operand_node = parsed.nodes.get(operand.0)?;
            let operand_text = text.get(operand_node.span.clone())?;
            let operand_type = initializer_expression_type(
                context,
                source,
                locals,
                operand_text,
                base + operand_node.span.start,
                depth + 1,
            )?;
            indexed_element_type(&operand_type)
        }
        crate::syntax::ExprKind::Composite {
            literal_type: Some(literal_type),
            ..
        } => {
            let node = parsed.nodes.get(literal_type.0)?;
            let text = text.get(node.span.clone())?;
            Some(normalize_type(text))
        }
        crate::syntax::ExprKind::Call { .. } => {
            call_result_type(context, context.file(), text, 0, 1)
                .or_else(|| test_function_result(context, text))
        }
        crate::syntax::ExprKind::FunctionLiteral { .. } => function_literal_text(text),
        _ => super::value::default_type(text).map(|text| normalize_type(&text)),
    }
}

/// Closed same-directory test function result type (workspace lookup misses
/// test-file declarations outside an open package view).
fn test_function_result(context: &Context<'_>, text: &str) -> Option<String> {
    let parsed = crate::syntax::parse_expression(text);
    if !parsed.diagnostics.is_empty() {
        return None;
    }
    let expression = parsed.nodes.get(parsed.root.0)?;
    let crate::syntax::ExprKind::Call { function, .. } = &expression.kind else {
        return None;
    };
    let callee = parsed.nodes.get(function.0)?;
    let crate::syntax::ExprKind::Name = callee.kind else {
        return None;
    };
    let name = text.get(callee.span.clone())?;
    let workspace = context.workspace();
    let file = context.file();
    let package = workspace.files.get(file)?.package.clone();
    let location =
        super::view::package_symbol(workspace, None, file.parent()?, &package, name, true)?;
    let source = workspace
        .source(&location.path)
        .or_else(|| std::fs::read_to_string(&location.path).ok())?;
    let stub = crate::syntax::stub(&source);
    let symbol = stub.symbols.iter().find(|symbol| {
        symbol.selection.start == location.offset
            && symbol.kind == crate::syntax::SymbolKind::Function
    })?;
    let end = symbol
        .body
        .as_ref()
        .map_or(symbol.range.end, |body| body.start);
    let header = source.get(symbol.range.start..end)?;
    let parsed = crate::syntax::parse_signature(header);
    if !parsed.diagnostics.is_empty() {
        return None;
    }
    let result = parsed.results.first()?;
    let annotation = header.get(result.annotation.clone())?.trim();
    Some(normalize_type(annotation))
}

fn indexed_element_type(ty: &str) -> Option<String> {
    let mut ty = ty.trim();
    while let Some(rest) = ty.strip_prefix('*') {
        ty = rest.trim();
    }
    if let Some(rest) = ty.strip_prefix("[]") {
        return Some(normalize_type(rest));
    }
    if ty == "string" {
        return Some("byte".to_owned());
    }
    if ty.starts_with('[')
        && let Some(close) = matching_brace(ty, 0)
    {
        return Some(normalize_type(&ty[close + 1..]));
    }
    if ty.starts_with("map[")
        && let Some(close) = matching_brace(ty, 3)
    {
        return Some(normalize_type(&ty[close + 1..]));
    }
    None
}

pub(super) fn local(
    context: &Context<'_>,
    locals: &Locals,
    binding: &Binding,
    range: super::SourceRange,
) -> HoverContent {
    let mut content = HoverContent::new(range);
    let file = context.file();
    let source = context.source();
    let workspace = context.workspace();
    if !in_function_header(context, binding.selection.start)
        && binding.object_kind != ObjectKind::Parameter
        && !multi_name_short_declaration(source, binding.selection.start)
        && !control_header_statement(source, binding.selection.start)
        && let Some(documents) = doc::attached(
            context,
            file,
            source,
            doc::Target {
                start: line_start_token(source, binding.selection.start),
                end: binding.selection.end,
                declaration: None,
                trailing: true,
            },
        )
    {
        content.doc = Some(documents.text);
        content.doc_links = documents.links;
    }
    content.code = match binding.declaration {
        DeclarationKind::TypeParameter => binding_text(workspace, file, source, locals, binding)
            .map(|constraint| format!("type parameter {} {constraint}", binding.name)),
        DeclarationKind::Type => binding
            .annotation()
            .and_then(|span| source.get(span.clone()))
            .and_then(|annotation| workspace.signature_type(file, file, annotation))
            .map(|annotation| format!("type {} {annotation}", binding.name)),
        DeclarationKind::Constant => local_constant(context, binding),
        DeclarationKind::Value | DeclarationKind::Variadic => {
            function_literal_type(source, binding)
                .or_else(|| call_initializer_type(context, source, binding))
                .or_else(|| multi_initializer_type(context, source, binding))
                .or_else(|| initializer_type(source, binding))
                .or_else(|| byte_index_alias(context, source, locals, binding))
                .or_else(|| arithmetic_initializer_type(source, binding))
                .or_else(|| binding_text(workspace, file, source, locals, binding))
                .or_else(|| binding_initializer_type(context, source, locals, binding, 0))
                .or_else(|| {
                    workspace
                        .binding_type(file, source, locals, binding)
                        .map(|text| normalize_type(&text))
                })
                .map(|ty| format!("var {} {ty}", binding.name))
        }
    };
    if let Some(code) = content.code.take() {
        let mut code = replace_invalid_qualified(workspace, file, &code);
        // Preserve the source spelling of the universe `any` alias, which the
        // native inference boundary canonicalizes to `interface{}`.
        if code.contains("interface{}") {
            let start = source[..binding.selection.start]
                .rfind('\n')
                .map_or(0, |index| index + 1);
            let end = source[binding.selection.start..]
                .find('\n')
                .map_or(source.len(), |index| binding.selection.start + index);
            if source.get(start..end).is_some_and(|line| {
                line.split(|ch: char| !ch.is_alphanumeric() && ch != '_')
                    .any(|word| word == "any")
            }) {
                code = code.replace("interface{}", "any");
            }
        }
        content.code = Some(code);
    }
    content
}

/// Type of one slot in a multi-assignment short declaration (`i, j := a, b`).
fn multi_initializer_type(
    context: &Context<'_>,
    source: &str,
    binding: &Binding,
) -> Option<String> {
    let (span, result, count) = binding.initializer()?;
    if count <= 1 || span.len() > MAX_SIGNATURE_BYTES {
        return None;
    }
    let text = source.get(span.clone())?;
    let segments = split_top_level(text, ',');
    if segments.len() == 1 {
        return call_result_type(context, context.file(), segments[0].trim(), result, count);
    }
    let segment = segments.get(result)?.trim();
    if segment.is_empty() {
        return None;
    }
    call_result_type(context, context.file(), segment, 0, 1)
        .or_else(|| super::value::default_type(segment).map(|text| normalize_type(&text)))
}

/// Whether the token sits in a control-statement header (`for x := ...`).
fn control_header_statement(source: &str, offset: usize) -> bool {
    let start = source[..offset].rfind('\n').map_or(0, |index| index + 1);
    let line = source[start..offset].trim_start();
    line.starts_with("for ")
}

/// Written signature of a short-declared function literal initializer.
fn function_literal_type(source: &str, binding: &Binding) -> Option<String> {
    if binding.declaration != DeclarationKind::Value {
        return None;
    }
    let (span, result, count) = binding.initializer()?;
    if result != 0 || count != 1 || span.len() > MAX_SIGNATURE_BYTES {
        return None;
    }
    let text = source.get(span.clone())?;
    let parsed = crate::syntax::parse_expression(text);
    if !parsed.diagnostics.is_empty() {
        return None;
    }
    let expression = parsed.nodes.get(parsed.root.0)?;
    let crate::syntax::ExprKind::FunctionLiteral { signature, .. } = &expression.kind else {
        return None;
    };
    let header = text.get(signature.clone())?;
    let header = if header.trim_start().starts_with("func") {
        header.trim().to_owned()
    } else {
        format!("func{}", header.trim_end())
    };
    Some(normalize_signature(&header))
}

/// Result type of a call used as the binding's initializer.
fn call_initializer_type(context: &Context<'_>, source: &str, binding: &Binding) -> Option<String> {
    let (span, result, count) = binding.initializer()?;
    if span.len() > MAX_SIGNATURE_BYTES {
        return None;
    }
    call_result_type(
        context,
        context.file(),
        source.get(span.clone())?,
        result,
        count,
    )
}

/// Result type of a direct call to a known package-level function.
pub(super) fn call_result_type(
    context: &Context<'_>,
    expression_file: &Path,
    text: &str,
    slot: usize,
    count: usize,
) -> Option<String> {
    let workspace = context.workspace();
    let rendered =
        context.direct_call_result_type(expression_file, text, slot, count, |alias| {
            !import_resolves(workspace, expression_file, alias)
        })?;
    Some(normalize_type(&rendered))
}

/// Syntactic default type of a single literal/composite initializer.
fn initializer_type(source: &str, binding: &Binding) -> Option<String> {
    let (span, result, count) = binding.initializer()?;
    if result != 0 || count != 1 || span.len() > MAX_SIGNATURE_BYTES {
        return None;
    }
    super::value::default_type(source.get(span.clone())?).map(|text| normalize_type(&text))
}

fn local_constant(context: &Context<'_>, binding: &Binding) -> Option<String> {
    let source = context.source();
    let (span, result, count) = binding.initializer()?;
    if result != 0 || count != 1 || span.len() > MAX_SIGNATURE_BYTES {
        return None;
    }
    let literal = super::value::literal(source.get(span.clone())?)?;
    format!(
        "const {} untyped {} = {}",
        binding.name,
        literal.untyped?,
        literal.assigned()
    )
    .into()
}

/// Bindings declared by an enclosing function header: parameters, results,
/// receivers and type parameters.
pub(super) fn header_binding(
    context: &Context<'_>,
    span: &Range<usize>,
    range: super::SourceRange,
) -> Result<Option<HoverContent>, (i32, String)> {
    let source = context.source();
    for symbol in &context.workspace().files[context.file()].symbols {
        let Some(body) = &symbol.body else {
            continue;
        };
        let Some(declaration) = symbol.declaration() else {
            continue;
        };
        if !(declaration.start <= span.start && span.end <= body.start) {
            continue;
        }
        let Some(header) = source.get(declaration.start..body.start) else {
            continue;
        };
        if header.len() > MAX_SIGNATURE_BYTES {
            continue;
        }
        let parsed = crate::syntax::parse_signature(header);
        if !parsed.diagnostics.is_empty() {
            continue;
        }
        let locals = Locals::from_header(source, declaration.start, body.clone(), &parsed);
        if let Some(binding) = locals
            .definition_at(span, &source[span.clone()])
            .filter(|binding| binding.selection == *span)
        {
            return Ok(Some(local(context, &locals, binding, range)));
        }
    }
    Ok(None)
}

pub(super) fn member(
    context: &Context<'_>,
    member: MemberDeclaration<'_>,
    receiver: Option<(&Path, &str)>,
    span: &Range<usize>,
    range: super::SourceRange,
) -> HoverContent {
    let mut content = HoverContent::new(range);
    let Some(source_text) = declaration_source(context, member.file) else {
        return content;
    };
    let source = source_text.text();
    let map = SourceMap::new(source);
    let Some(data) = Workspace::member_data_in_source(member, source, &map) else {
        return content;
    };
    let Some(annotation) = data.annotation.as_deref() else {
        return content;
    };
    let workspace = context.workspace();
    let rendered = if member.owner.generic {
        match receiver {
            Some((scope, receiver)) => workspace
                .scoped_member_types(context.file(), scope, receiver, member)
                .and_then(|types| types.render(annotation)),
            None => type_text(context, member.file, annotation),
        }
    } else {
        // Declaration types are requalified from the consumer's imports.
        type_text(context, member.file, annotation)
    };
    let ty = rendered
        .map(|text| normalize_type(&text))
        .unwrap_or_else(|| normalize_type(annotation.trim()));
    let base = member.owner.end;
    let end = map.offset(member.owner.range.end).unwrap_or(source.len());
    let declaration = source.get(base..end);
    let entries = declaration.map(crate::syntax::aggregate_members);
    let entry = match (&declaration, &entries) {
        (Some(declaration), Some(entries)) => member.entry(declaration, entries),
        _ => None,
    };
    content.code = match member.kind {
        crate::syntax::MemberKind::Field => {
            let mut code = format!("field {} {ty}", member.name);
            if let (Some(declaration), Some(entry)) = (declaration, entry) {
                if let Some(tag) = field_tag(declaration, entry.annotation.end) {
                    code.push(' ');
                    code.push_str(&tag);
                }
                let declaring = member.file == context.file()
                    && map
                        .offset(data.selection.start)
                        .zip(map.offset(data.selection.end))
                        .is_some_and(|(start, end)| (start..end) == *span);
                if declaring {
                    let summary =
                        super::size::field_layout(context, member.file, member.owner, member.name)
                            .map(|field| {
                                let mut summary =
                                    format!("size={}", super::size::number(field.size));
                                if let Some(class) = super::size::size_class(field.size)
                                    .filter(|class| *class > field.size)
                                {
                                    summary.push_str(&format!(
                                        ", class={}",
                                        super::size::number(class)
                                    ));
                                }
                                summary.push_str(&format!(
                                    ", offset={}",
                                    super::size::number(field.offset)
                                ));
                                summary
                            });
                    if let Some(summary) = summary {
                        code.push_str(" // ");
                        code.push_str(&summary);
                    }
                }
            }
            Some(code)
        }
        crate::syntax::MemberKind::Method | crate::syntax::MemberKind::Interface => {
            let owner = workspace
                .result_type(context.file(), member.file, &member.owner.name)
                .or_else(|| {
                    workspace.signature_type(context.file(), member.file, &member.owner.name)
                })
                .unwrap_or_else(|| member.owner.name.clone());
            let label = interface_method_label(context, member, annotation, &ty);
            Some(format!("func ({owner}) {label}"))
        }
    };
    if let Some((declaration, entry)) = declaration.zip(entry) {
        content.doc = doc::attached(
            context,
            member.file,
            source,
            doc::Target {
                start: field_start(declaration, entry.selection.start).map_or(
                    line_start_token(source, base + entry.selection.start),
                    |start| base + start,
                ),
                end: base + entry.annotation.end,
                declaration: None,
                trailing: true,
            },
        )
        .map(|documents| {
            content.doc_links = documents.links;
            documents.text
        });
    }
    if matches!(member.kind, crate::syntax::MemberKind::Field)
        && content.footer.is_none()
        && let Some(version) =
            super::version::added_field(context, member.file, &member.owner.name, member.name)
    {
        content.footer = Some(format!("Added in {version}"));
    }
    if matches!(
        member.kind,
        crate::syntax::MemberKind::Method | crate::syntax::MemberKind::Interface
    ) && content.footer.is_none()
        && let Some(version) =
            super::version::added_method(context, member.file, &member.owner.name, member.name)
    {
        content.footer = Some(format!("Added in {version}"));
    }
    if matches!(member.kind, crate::syntax::MemberKind::Field)
        && let Some((receiver_file, receiver_text)) = receiver
        && let Some((owner_file, owner_symbol)) =
            workspace.type_declaration(member.file, &member.owner.name)
        && let Some(path) = super::promote::promotion_path(
            context,
            receiver_file,
            receiver_text,
            owner_file,
            owner_symbol,
        )
        && !path.is_empty()
        && let Some(code) = content.code.as_mut()
    {
        code.push_str(" // through ");
        code.push_str(&path.join(", "));
    }
    if let Some(code) = content.code.take() {
        content.code = Some(replace_invalid_qualified(workspace, context.file(), &code));
    }
    content.link = member_link(context, member, &member.owner.name);
    content
}

/// Source text of a struct field tag immediately after its type.
fn field_tag(declaration: &str, annotation_end: usize) -> Option<String> {
    let rest = declaration.get(annotation_end..)?;
    let token = crate::syntax::Lexer::new(rest).next()?;
    if token.kind != TokenKind::String {
        return None;
    }
    rest.get(token.span).map(str::to_owned)
}

/// Interface parameters with consumer-relative qualification for exported
/// named types; unexported written names stay verbatim.
fn source_interface_parameters(
    context: &Context<'_>,
    origin: &Path,
    header: &str,
    parameters: &[crate::syntax::Parameter],
) -> Option<String> {
    let mut parts = Vec::with_capacity(parameters.len());
    for parameter in parameters {
        let annotation = header.get(parameter.annotation.clone())?.trim();
        let root = annotation
            .trim_start_matches('*')
            .split(['[', ' '])
            .next()?;
        let rendered = if root
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_uppercase())
        {
            let rendered =
                type_text(context, origin, annotation).unwrap_or_else(|| annotation.to_owned());
            let consumer_package = context
                .workspace()
                .files
                .get(context.file())
                .map(|stub| stub.package.as_str());
            let origin_package = context
                .workspace()
                .files
                .get(origin)
                .map(|stub| stub.package.as_str());
            if !rendered.contains('.')
                && consumer_package != origin_package
                && let Some(package) = origin_package
            {
                format!("{package}.{rendered}")
            } else {
                rendered
            }
        } else {
            annotation.to_owned()
        };
        let rendered = if parameter.variadic {
            format!("...{rendered}")
        } else {
            rendered
        };
        match parameter
            .name
            .as_ref()
            .and_then(|name| header.get(name.clone()))
        {
            Some(name) => parts.push(format!("{} {rendered}", name.trim())),
            None => parts.push(rendered),
        }
    }
    Some(parts.join(", "))
}

/// gopls-shaped method signature for an interface method declaration.
pub(super) fn interface_source_label(
    context: &Context<'_>,
    origin: &Path,
    name: &str,
    annotation: &str,
) -> Option<String> {
    let source = annotation.trim_start();
    let header = format!("func {name}{source}");
    let signature = crate::syntax::parse_signature(&header);
    if !signature.diagnostics.is_empty() {
        return None;
    }
    let parameters = source_interface_parameters(context, origin, &header, &signature.parameters)?;
    let results = source_interface_parameters(context, origin, &header, &signature.results)?;
    let mut label = format!("{name}({parameters})");
    if !results.is_empty() {
        label.push(' ');
        if signature.results.len() == 1 && signature.results[0].name.is_none() {
            label.push_str(&results);
        } else {
            label.push('(');
            label.push_str(&results);
            label.push(')');
        }
    }
    Some(label)
}

fn interface_method_label(
    context: &Context<'_>,
    member: MemberDeclaration<'_>,
    annotation: &str,
    rendered: &str,
) -> String {
    let source = annotation.trim_start();
    if source.starts_with('(') {
        if let Some(label) = interface_source_label(context, member.file, member.name, source) {
            return label;
        }
        return format!("{}{source}", member.name);
    }
    let workspace = context.workspace();
    let header = format!("func {}{}", member.name, annotation.trim_start());
    let signature = crate::syntax::parse_literal_signature(&header);
    if signature.diagnostics.is_empty()
        && let Some((information, _)) = workspace.signature_from_parsed(
            context.file(),
            member.file,
            &header,
            &signature,
            member.name,
        )
    {
        if let Some(label) = source_interface_label(&header, member.name, &signature) {
            return label;
        }
        let label = normalize_signature(&information.label);
        if label.contains("((") {
            return format!("{}{}", member.name, annotation.trim_start());
        }
        return label;
    }
    format!("{}({rendered})", member.name)
}

/// Interface method signatures prefer written unexported named types
/// (`_Socklen`), which `types.TypeString` would resolve to their underlying
/// type when rendered across the declaration boundary.
fn source_interface_label(
    header: &str,
    name: &str,
    signature: &crate::syntax::Signature,
) -> Option<String> {
    let mut spelled = false;
    for parameter in signature.parameters.iter().chain(&signature.results) {
        let annotation = header.get(parameter.annotation.clone())?.trim();
        let root = annotation
            .trim_start_matches('*')
            .split(['[', ' '])
            .next()?;
        if root
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_lowercase() || ch == '_')
            && !matches!(
                root,
                "any"
                    | "bool"
                    | "byte"
                    | "comparable"
                    | "complex64"
                    | "complex128"
                    | "error"
                    | "float32"
                    | "float64"
                    | "int"
                    | "int8"
                    | "int16"
                    | "int32"
                    | "int64"
                    | "rune"
                    | "string"
                    | "uint"
                    | "uint8"
                    | "uint16"
                    | "uint32"
                    | "uint64"
                    | "uintptr"
            )
        {
            spelled = true;
            break;
        }
    }
    if !spelled {
        return None;
    }
    let parameters = interface_parameter_list(header, &signature.parameters)?;
    let results = interface_parameter_list(header, &signature.results)?;
    let mut label = format!("{name}({parameters})");
    if !results.is_empty() {
        label.push(' ');
        if signature.results.len() == 1 && signature.results[0].name.is_none() {
            label.push_str(&results);
        } else {
            label.push('(');
            label.push_str(&results);
            label.push(')');
        }
    }
    Some(label)
}

fn interface_parameter_list(
    header: &str,
    parameters: &[crate::syntax::Parameter],
) -> Option<String> {
    let mut parts = Vec::with_capacity(parameters.len());
    for parameter in parameters {
        let annotation = header.get(parameter.annotation.clone())?.trim();
        let annotation = if parameter.variadic {
            format!("...{annotation}")
        } else {
            annotation.to_owned()
        };
        match parameter
            .name
            .as_ref()
            .and_then(|name| header.get(name.clone()))
        {
            Some(name) => parts.push(format!("{} {annotation}", name.trim())),
            None => parts.push(annotation),
        }
    }
    Some(parts.join(", "))
}

/// gopls-compatible link for a member (field or method), if linkable.
fn member_link(
    context: &Context<'_>,
    member: MemberDeclaration<'_>,
    owner_name: &str,
) -> Option<HoverLink> {
    let workspace = context.workspace();
    if !crate::syntax::is_exported_name(member.name) || !crate::syntax::is_exported_name(owner_name)
    {
        return None;
    }
    if member
        .file
        .file_name()?
        .to_string_lossy()
        .ends_with("_test.go")
    {
        return None;
    }
    if gopls_test_view_link(context) || platform_test_variant(context) {
        return None;
    }
    let consumer = context.file();
    if consumer
        .file_name()
        .is_some_and(|name| name.to_string_lossy().ends_with("_test.go"))
        && super::same_package(context, consumer, member.file)
    {
        return None;
    }
    Some(HoverLink {
        symbol: format!(
            "({}.{}).{}",
            workspace.files[member.file].package, owner_name, member.name
        ),
        path: link_import_path(workspace, member.file)?,
        anchor: format!("{owner_name}.{}", member.name),
    })
}

/// Import path as rendered in pkg.go.dev links: dependency modules keep their
/// resolved `module@version` prefix.
pub(super) fn link_import_path(workspace: &Workspace, origin: &Path) -> Option<String> {
    let path = workspace.import_path(origin)?;
    let (root, module) = workspace
        .modules
        .iter()
        .filter(|(root, _)| origin.starts_with(root))
        .max_by_key(|(root, _)| root.components().count())?;
    if module == "std" {
        return Some(path);
    }
    let Some((_, version)) = root
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.rsplit_once('@'))
    else {
        return Some(path);
    };
    let resolved = format!("{module}@{version}");
    Some(match path.strip_prefix(module.as_str()) {
        Some(rest) => format!("{resolved}{rest}"),
        None => resolved,
    })
}

pub(super) fn package_link(
    context: &Context<'_>,
    origin: &Path,
    name: &str,
    symbol: String,
) -> Option<HoverLink> {
    let workspace = context.workspace();
    if !crate::syntax::is_exported_name(name) {
        return None;
    }
    if origin.file_name()?.to_string_lossy().ends_with("_test.go") {
        return None;
    }
    if gopls_test_view_link(context) || platform_test_variant(context) {
        return None;
    }
    // gopls resolves objects through a test variant package (ForTest), whose
    // declaration links are suppressed. The package under test (same directory)
    // is loaded as its test variant even from a separate `_test` package.
    let consumer = context.file();
    if consumer
        .file_name()
        .is_some_and(|name| name.to_string_lossy().ends_with("_test.go"))
        && (super::same_package(context, consumer, origin)
            || (consumer.parent() == origin.parent()
                && context.workspace().orphan_view_target(consumer).is_none()))
    {
        return None;
    }
    Some(HoverLink {
        symbol,
        path: link_import_path(workspace, origin)?,
        anchor: name.to_owned(),
    })
}

/// Enclosing function literal `(func_start, body_start, body_end)` for a cursor.
fn nested_literal(source: &str, offset: usize) -> Option<(usize, usize, usize)> {
    let tokens: Vec<_> = crate::syntax::Lexer::new(source)
        .filter(|token| !token.inserted)
        .collect();
    let mut best: Option<(usize, usize, usize)> = None;
    for (index, token) in tokens.iter().enumerate() {
        if token.kind != TokenKind::Func || token.span.start >= offset {
            continue;
        }
        let Some(_open) = tokens
            .get(index + 1)
            .filter(|token| token.kind == TokenKind::LParen)
        else {
            continue;
        };
        let mut depth = 0usize;
        let mut close = None;
        for (position, token) in tokens.iter().enumerate().skip(index + 1) {
            match token.kind {
                TokenKind::LParen => depth += 1,
                TokenKind::RParen => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        close = Some(position);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(close) = close else { continue };
        let mut body_index = None;
        for (position, token) in tokens.iter().enumerate().skip(close + 1) {
            match token.kind {
                TokenKind::LBrace => {
                    body_index = Some(position);
                    break;
                }
                TokenKind::Semicolon | TokenKind::RBrace => break,
                _ => {}
            }
        }
        let Some(body_index) = body_index else {
            continue;
        };
        let body_start = tokens[body_index].span.start;
        let mut depth = 0usize;
        let mut body_end = None;
        for token in tokens.iter().skip(body_index) {
            match token.kind {
                TokenKind::LBrace => depth += 1,
                TokenKind::RBrace => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        body_end = Some(token.span.end);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(body_end) = body_end else { continue };
        if token.span.start < offset
            && offset < body_end
            && best.is_none_or(|(start, _, _)| token.span.start > start)
        {
            best = Some((token.span.start, body_start, body_end));
        }
    }
    best
}

/// Locals declared by the innermost enclosing function-literal header.
pub(super) fn nested_locals(context: &Context<'_>, offset: usize) -> Option<Locals> {
    let source = context.source();
    let (func_start, body_start, body_end) = nested_literal(source, offset)?;
    let header = source.get(func_start..body_start)?;
    if header.len() > MAX_SIGNATURE_BYTES {
        return None;
    }
    let parsed = crate::syntax::parse_literal_signature(header);
    if !parsed.diagnostics.is_empty() {
        return None;
    }
    Some(Locals::from_header(
        source,
        func_start,
        body_start..body_end,
        &parsed,
    ))
}

/// Resolve a parameter of an enclosing function literal that the cursor scope
/// did not record (callbacks inside call arguments).
pub(super) fn nested_function_binding(
    context: &Context<'_>,
    offset: usize,
    selection: &Range<usize>,
    range: super::SourceRange,
) -> Option<HoverContent> {
    let source = context.source();
    let (func_start, body_start, body_end) = nested_literal(source, offset)?;
    let header = source.get(func_start..body_start)?;
    if header.len() > MAX_SIGNATURE_BYTES {
        return None;
    }
    let parsed = crate::syntax::parse_literal_signature(header);
    if !parsed.diagnostics.is_empty() {
        return None;
    }
    let locals = Locals::from_header(source, func_start, body_start..body_end, &parsed);
    let binding = locals
        .definition_at(selection, source.get(selection.clone())?)
        .filter(|binding| binding.selection == *selection)?;
    Some(local(context, &locals, binding, range))
}

/// Resolve a value name to an enclosing function literal parameter.
pub(super) fn nested_function_local(
    context: &Context<'_>,
    offset: usize,
    name: &str,
    range: super::SourceRange,
) -> Option<HoverContent> {
    let source = context.source();
    let literal = nested_literal(source, offset);
    let (func_start, body_start, body_end) = literal?;
    let header = source.get(func_start..body_start)?;
    if header.len() > MAX_SIGNATURE_BYTES {
        return None;
    }
    let parsed = crate::syntax::parse_literal_signature(header);
    if !parsed.diagnostics.is_empty() {
        return None;
    }
    let locals = Locals::from_header(source, func_start, body_start..body_end, &parsed);
    let binding = locals
        .visible_bindings(offset)
        .find(|binding| binding.name == name)?;
    Some(local(context, &locals, binding, range))
}

/// A `_test.go` file whose name carries the host GOOS/GOARCH suffix: gopls
/// resolves it through a dynamic platform view whose metadata hides links.
pub(super) fn platform_test_variant(context: &Context<'_>) -> bool {
    let consumer = context.file();
    let Some(name) = consumer.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let Some(stem) = name.strip_suffix("_test.go") else {
        return false;
    };
    let Some(suffix) = stem.rsplit('_').next() else {
        return false;
    };
    let host_goos = if cfg!(windows) { "windows" } else { "linux" };
    let host_goarch = if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "amd64"
    };
    suffix == host_goos || suffix == host_goarch
}

/// gopls suppresses declaration links for host-active detached test packages,
/// which it resolves through their test variant metadata.
pub(super) fn gopls_test_view_link(context: &Context<'_>) -> bool {
    let workspace = context.workspace();
    let consumer = context.file();
    consumer
        .file_name()
        .is_some_and(|name| name.to_string_lossy().ends_with("_test.go"))
        && workspace.is_detached(consumer)
        && workspace.orphan_view_target(consumer).is_none()
        && workspace
            .files
            .get(consumer)
            .is_some_and(|stub| stub.package.ends_with("_test"))
}

/// Type of a conversion expression used as a value initializer.
pub(super) fn conversion_type(context: &Context<'_>, origin: &Path, text: &str) -> Option<String> {
    let parsed = crate::syntax::parse_expression(text);
    if !parsed.diagnostics.is_empty() {
        return None;
    }
    let expression = parsed.nodes.get(parsed.root.0)?;
    let crate::syntax::ExprKind::Call { function, .. } = &expression.kind else {
        return None;
    };
    let callee = parsed.nodes.get(function.0)?;
    let callee_text = text.get(callee.span.clone())?;
    let looks_like_type = callee_text.contains(['[', '*', ']'])
        || callee_text.starts_with("map[")
        || callee_text.starts_with("chan ")
        || callee_text.starts_with("func")
        || callee_text.starts_with("struct")
        || callee_text.starts_with("interface");
    if !looks_like_type
        && context
            .workspace()
            .type_declaration(origin, callee_text)
            .is_none()
    {
        return None;
    }
    let rendered = context
        .workspace()
        .signature_type(context.file(), origin, callee_text)?;
    Some(normalize_type(&rendered))
}

/// Written function type of a named function used as a value.
pub(super) fn function_value_type(
    context: &Context<'_>,
    origin: &Path,
    text: &str,
) -> Option<String> {
    let workspace = context.workspace();
    let tokens: Vec<_> = crate::syntax::Lexer::new(text.trim())
        .filter(|token| !token.inserted)
        .collect();
    let (file, symbol) = match tokens.as_slice() {
        [name] if name.kind == TokenKind::Ident => {
            let name = text.get(name.span.clone())?;
            workspace
                .unqualified_symbol(origin, name)
                .filter(|(_, symbol)| symbol.is_function())?
        }
        [alias, period, name]
            if alias.kind == TokenKind::Ident
                && period.kind == TokenKind::Period
                && name.kind == TokenKind::Ident =>
        {
            let alias = text.get(alias.span.clone())?;
            let name = text.get(name.span.clone())?;
            workspace
                .imported_symbol_iter(origin, alias)
                .find(|(_, symbol)| symbol.is_function() && symbol.name == name)?
        }
        _ => return None,
    };
    let source_text = declaration_source(context, file)?;
    let source = source_text.text();
    let declaration = symbol.declaration()?;
    let end = symbol
        .body
        .as_ref()
        .map_or(declaration.end, |body| body.start);
    let header = source.get(declaration.start..end)?;
    if header.len() > MAX_SIGNATURE_BYTES {
        return None;
    }
    let parsed = crate::syntax::parse_signature(header);
    if !parsed.diagnostics.is_empty() {
        return None;
    }
    let (signature, _) =
        workspace.declared_signature(context.file(), file, header, &parsed, &symbol.name)?;
    let label = normalize_signature(&signature.label);
    let rest = label.strip_prefix(&symbol.name)?;
    Some(format!("func{}", rest.trim_end()))
}

/// Written signature of a function literal expression initializer.
pub(super) fn function_literal_text(text: &str) -> Option<String> {
    let parsed = crate::syntax::parse_expression(text);
    if !parsed.diagnostics.is_empty() {
        return None;
    }
    let expression = parsed.nodes.get(parsed.root.0)?;
    let crate::syntax::ExprKind::FunctionLiteral { signature, .. } = &expression.kind else {
        return None;
    };
    let header = text.get(signature.clone())?;
    let header = if header.trim_start().starts_with("func") {
        header.trim().to_owned()
    } else {
        format!("func{}", header.trim_end())
    };
    Some(normalize_signature(&header))
}

/// Default `int` for an arithmetic expression with an integer literal operand.
fn arithmetic_initializer_type(source: &str, binding: &Binding) -> Option<String> {
    let (span, _, count) = binding.initializer()?;
    if count != 1 {
        return None;
    }
    let text = source.get(span.clone())?;
    let parsed = crate::syntax::parse_expression(text);
    if !parsed.diagnostics.is_empty() {
        return None;
    }
    let expression = parsed.nodes.get(parsed.root.0)?;
    let crate::syntax::ExprKind::Binary { operator, .. } = &expression.kind else {
        return None;
    };
    let operator = text.get(operator.clone())?.trim();
    if !matches!(operator, "+" | "-" | "*" | "/" | "%" | "&" | "|" | "^") {
        return None;
    }
    let mut integers = 0usize;
    for token in crate::syntax::Lexer::new(text) {
        match token.kind {
            crate::syntax::TokenKind::Int => integers += 1,
            crate::syntax::TokenKind::String
            | crate::syntax::TokenKind::Float
            | crate::syntax::TokenKind::Char => return None,
            _ => {}
        }
    }
    (integers > 0).then(|| "int".to_owned())
}

/// A string index yields the universe `byte` alias, which native inference
/// canonicalizes to `uint8`.
fn byte_index_alias(
    context: &Context<'_>,
    source: &str,
    locals: &Locals,
    binding: &Binding,
) -> Option<String> {
    let (span, _, count) = binding.initializer()?;
    if count != 1 {
        return None;
    }
    let text = source.get(span.clone())?;
    let parsed = crate::syntax::parse_expression(text);
    if !parsed.diagnostics.is_empty() {
        return None;
    }
    let expression = parsed.nodes.get(parsed.root.0)?;
    let crate::syntax::ExprKind::Index { operand, .. } = &expression.kind else {
        return None;
    };
    let operand = parsed.nodes.get(operand.0)?;
    let crate::syntax::ExprKind::Name = operand.kind else {
        return None;
    };
    let name = text.get(operand.span.clone())?;
    let receiver = locals.resolve_at(span.start + operand.span.start, name)?;
    let ty = context
        .workspace()
        .binding_type(context.file(), source, locals, receiver)?;
    (normalize_type(&ty) == "string").then(|| "byte".to_owned())
}

/// `[len(packageVAR)]` folded to its declared array literal length.
fn array_literal_length(context: &Context<'_>, origin: &Path, name: &str) -> Option<usize> {
    let workspace = context.workspace();
    let (file, symbol) = workspace.unqualified_symbol(origin, name)?;
    if !symbol.is_var() {
        return None;
    }
    let source_text = declaration_source(context, file)?;
    let source = source_text.text();
    let declaration = symbol.declaration()?;
    let spec = source.get(declaration)?;
    let assign = crate::syntax::Lexer::new(spec).find(|token| token.kind == TokenKind::Assign)?;
    let value = spec.get(assign.span.end..)?.trim_start();
    if let Some(rest) = value.strip_prefix("[...]") {
        let brace = rest.find('{')?;
        let close = matching_brace(rest, brace)?;
        let elements: Vec<&str> = split_top_level(&rest[brace + 1..close], ',')
            .into_iter()
            .filter(|element| !element.trim().is_empty())
            .collect();
        return Some(elements.len());
    }
    let length = value.strip_prefix('[')?.split(']').next()?.trim();
    (!length.is_empty() && length.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| length.parse().ok())
        .flatten()
}

pub(super) fn fold_array_lengths(context: &Context<'_>, origin: &Path, text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut index = 0usize;
    while let Some(offset) = text[index..].find("len(") {
        let start = index + offset;
        output.push_str(&text[index..start]);
        let open = start + "len".len();
        let mut depth = 0usize;
        let mut end = None;
        for (position, ch) in text[open..].char_indices() {
            match ch {
                '(' => depth += 1,
                ')' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        end = Some(open + position);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(end) = end else {
            output.push_str(&text[start..]);
            return output;
        };
        let inner = text[open + 1..end].trim();
        let identifier = !inner.is_empty()
            && inner
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_');
        if identifier && let Some(length) = array_literal_length(context, origin, inner) {
            output.push_str(&length.to_string());
        } else {
            output.push_str(&text[start..=end]);
        }
        index = end + 1;
    }
    output.push_str(&text[index..]);
    output
}

/// Start of the declaration keyword preceding a value/type spec.
pub(super) fn declaration_keyword(source: &str, spec_start: usize) -> Option<usize> {
    let mut braces = 0usize;
    let mut keyword = None;
    for token in crate::syntax::Lexer::new(source) {
        if token.span.start >= spec_start {
            break;
        }
        match token.kind {
            TokenKind::LBrace => braces += 1,
            TokenKind::RBrace => braces = braces.saturating_sub(1),
            TokenKind::Func | TokenKind::Type | TokenKind::Var | TokenKind::Const
                if braces == 0 =>
            {
                keyword = Some(token.span.start);
            }
            _ => {}
        }
    }
    keyword
}

pub(super) fn package_name(workspace: &Workspace, file: &Path) -> String {
    workspace
        .files
        .get(file)
        .map_or_else(String::new, |stub| stub.package.clone())
}
