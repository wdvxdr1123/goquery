//! Small Go constant and untyped-literal projections used by hover descriptions.
use crate::syntax::{ExprKind, TokenKind};

/// Source text and evaluated value of a constant literal initializer.
pub(super) struct Literal {
    /// Canonical `go/constant` value rendering.
    pub(super) value: String,
    /// Original source declaration of the value, when it differs.
    pub(super) declaration: Option<String>,
    /// Default type of an untyped constant/literal expression.
    pub(super) untyped: Option<&'static str>,
}

impl Literal {
    /// Text after `=`; a differing canonical value becomes an inline comment.
    pub(super) fn assigned(&self) -> String {
        match &self.declaration {
            Some(declaration) if declaration != &self.value => {
                format!("{declaration} // {}", self.value)
            }
            _ => self.value.clone(),
        }
    }
}

/// Decode a single literal expression into constant form.
pub(super) fn literal(text: &str) -> Option<Literal> {
    let parsed = crate::syntax::parse_expression(text);
    if !parsed.diagnostics.is_empty() {
        return None;
    }
    let expression = parsed.nodes.get(parsed.root.0)?;
    if let ExprKind::Unary { operator, operand } = &expression.kind {
        let sign = text.get(operator.clone())?;
        if sign == "-" || sign == "+" {
            let inner = parsed.nodes.get(operand.0)?;
            let mut literal = literal(text.get(inner.span.clone())?)?;
            if sign == "-" {
                literal.value = negate(&literal.value, &literal.untyped)?;
            }
            literal.declaration = None;
            return Some(literal);
        }
        return None;
    }
    let token = crate::syntax::Lexer::new(text).next()?;
    match expression.kind {
        ExprKind::Literal => {}
        ExprKind::Name => {
            return matches!(text.trim(), "true" | "false").then(|| Literal {
                value: text.trim().to_owned(),
                declaration: None,
                untyped: Some("bool"),
            });
        }
        _ => return None,
    }
    match token.kind {
        TokenKind::Int => {
            let (value, canonical) = integer(&text[token.span.clone()])?;
            Some(Literal {
                value,
                declaration: canonical.then(|| text[token.span.clone()].to_owned()),
                untyped: Some("int"),
            })
        }
        TokenKind::Char => {
            let value = crate::syntax::decode_rune_literal(&text[token.span.clone()])?;
            Some(Literal {
                value: value.to_string(),
                declaration: Some(text[token.span].to_owned()),
                untyped: Some("rune"),
            })
        }
        TokenKind::String => {
            let value = crate::syntax::decode_string_literal(&text[token.span.clone()])?;
            Some(Literal {
                value: truncate_constant_string(quote_bytes(&value)),
                declaration: None,
                untyped: Some("string"),
            })
        }
        TokenKind::Float => {
            let source = text[token.span].to_owned();
            Some(Literal {
                value: float_value(&source)?,
                declaration: None,
                untyped: Some("float"),
            })
        }
        TokenKind::Imag => Some(Literal {
            value: text[token.span].to_owned(),
            declaration: None,
            untyped: Some("complex"),
        }),
        _ => None,
    }
}

/// Default type of a single non-constant expression, using only syntax.
pub(super) fn default_type(text: &str) -> Option<String> {
    let parsed = crate::syntax::parse_expression(text);
    if !parsed.diagnostics.is_empty() {
        return None;
    }
    let expression = parsed.nodes.get(parsed.root.0)?;
    match &expression.kind {
        ExprKind::Literal => {
            let token = crate::syntax::Lexer::new(&text[expression.span.clone()]).next()?;
            Some(
                match token.kind {
                    TokenKind::Int => "int",
                    TokenKind::Char => "rune",
                    TokenKind::Float => "float64",
                    TokenKind::Imag => "complex128",
                    TokenKind::String => "string",
                    _ => return None,
                }
                .to_owned(),
            )
        }
        ExprKind::Name if matches!(text[expression.span.clone()].trim(), "true" | "false") => {
            Some("bool".to_owned())
        }
        ExprKind::Composite {
            literal_type: Some(literal_type),
            ..
        } => {
            let span = &parsed.nodes.get(literal_type.0)?.span;
            Some(text.get(span.clone())?.trim().to_owned())
        }
        _ => None,
    }
}

/// Decimal value and whether the source literal is already canonical.
fn integer(literal: &str) -> Option<(String, bool)> {
    let text = literal.replace('_', "");
    let (digits, radix) =
        if let Some(rest) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
            (rest, 16)
        } else if let Some(rest) = text.strip_prefix("0b").or_else(|| text.strip_prefix("0B")) {
            (rest, 2)
        } else if let Some(rest) = text.strip_prefix("0o").or_else(|| text.strip_prefix("0O")) {
            (rest, 8)
        } else if text.len() > 1 && text.starts_with('0') {
            (&text[1..], 8)
        } else {
            (text.as_str(), 10)
        };
    let mut value: u128 = 0;
    for byte in digits.bytes() {
        let digit = (byte as char).to_digit(radix)? as u128;
        value = value.checked_mul(radix as u128)?.checked_add(digit)?;
    }
    let canonical = radix == 10 && digits == value.to_string() && !literal.starts_with('0');
    Some((value.to_string(), canonical))
}

/// Replace bare `iota` identifiers with the current spec ordinal.
pub(super) fn substitute_iota(text: &str, iota: usize) -> String {
    let mut output = String::with_capacity(text.len());
    let mut index = 0usize;
    for token in crate::syntax::Lexer::new(text) {
        if token.inserted {
            continue;
        }
        let Some(prefix) = text.get(index..token.span.start) else {
            return text.to_owned();
        };
        output.push_str(prefix);
        if token.kind == TokenKind::Ident && &text[token.span.clone()] == "iota" {
            output.push_str(&iota.to_string());
        } else if let Some(part) = text.get(token.span.clone()) {
            output.push_str(part);
        }
        index = token.span.end;
    }
    match text.get(index..) {
        Some(rest) => output.push_str(rest),
        None => return text.to_owned(),
    }
    output
}

/// Evaluate a bounded integer constant expression (already iota-substituted).
pub(super) fn integer_expression(text: &str) -> Option<i64> {
    let tokens: Vec<_> = crate::syntax::Lexer::new(text)
        .filter(|token| !token.inserted)
        .collect();
    let mut index = 0usize;
    let value = expression(text, &tokens, &mut index, 0)?;
    (index == tokens.len()).then_some(value)
}

fn expression(
    text: &str,
    tokens: &[crate::syntax::Token],
    index: &mut usize,
    depth: usize,
) -> Option<i64> {
    if depth >= 16 {
        return None;
    }
    let mut value = term(text, tokens, index, depth)?;
    while let Some(token) = tokens.get(*index) {
        let operator = token.kind;
        if !matches!(
            operator,
            TokenKind::Add
                | TokenKind::Sub
                | TokenKind::Or
                | TokenKind::Xor
                | TokenKind::Shl
                | TokenKind::Shr
                | TokenKind::And
                | TokenKind::AndNot
        ) {
            break;
        }
        *index += 1;
        let right = term(text, tokens, index, depth)?;
        value = match operator {
            TokenKind::Add => value.checked_add(right)?,
            TokenKind::Sub => value.checked_sub(right)?,
            TokenKind::Or => value | right,
            TokenKind::Xor => value ^ right,
            TokenKind::Shl => value.checked_shl(u32::try_from(right).ok()?)?,
            TokenKind::Shr => value.checked_shr(u32::try_from(right).ok()?)?,
            TokenKind::And => value & right,
            TokenKind::AndNot => value & !right,
            _ => return None,
        };
    }
    Some(value)
}

fn term(
    text: &str,
    tokens: &[crate::syntax::Token],
    index: &mut usize,
    depth: usize,
) -> Option<i64> {
    let mut value = primary(text, tokens, index, depth)?;
    while let Some(token) = tokens.get(*index) {
        let operator = token.kind;
        if !matches!(operator, TokenKind::Mul | TokenKind::Quo | TokenKind::Rem) {
            break;
        }
        *index += 1;
        let right = primary(text, tokens, index, depth)?;
        value = match operator {
            TokenKind::Mul => value.checked_mul(right)?,
            TokenKind::Quo => {
                if right == 0 {
                    return None;
                }
                value / right
            }
            TokenKind::Rem => {
                if right == 0 {
                    return None;
                }
                value % right
            }
            _ => return None,
        };
    }
    Some(value)
}

fn primary(
    text: &str,
    tokens: &[crate::syntax::Token],
    index: &mut usize,
    depth: usize,
) -> Option<i64> {
    let token = tokens.get(*index)?;
    match token.kind {
        TokenKind::Int => {
            *index += 1;
            let (value, _) = integer(text.get(token.span.clone())?)?;
            value.parse().ok()
        }
        TokenKind::Sub => {
            *index += 1;
            Some(primary(text, tokens, index, depth)?.checked_neg()?)
        }
        TokenKind::Add => {
            *index += 1;
            primary(text, tokens, index, depth)
        }
        TokenKind::LParen => {
            *index += 1;
            let value = expression(text, tokens, index, depth + 1)?;
            if tokens.get(*index)?.kind != TokenKind::RParen {
                return None;
            }
            *index += 1;
            Some(value)
        }
        _ => None,
    }
}

/// Negated constant value in go/constant's decimal presentation.
fn negate(value: &str, untyped: &Option<&'static str>) -> Option<String> {
    match untyped {
        Some("int") => value.parse::<i64>().ok().map(|value| (-value).to_string()),
        Some("float") => crate::analysis::query::hover::value::float_value(value)
            .and_then(|_| value.parse::<f64>().ok())
            .map(negated_float),
        _ => None,
    }
}

fn negated_float(value: f64) -> String {
    format_float(-value)
}

/// go/constant's decimal presentation of a float literal.
pub(super) fn float_value(source: &str) -> Option<String> {
    let clean = source.replace('_', "");
    let value: f64 = clean.parse().ok()?;
    (!value.is_nan() && !value.is_infinite()).then(|| format_float(value))
}

fn format_float(value: f64) -> String {
    if value == 0.0 {
        return "0".to_owned();
    }
    if value.fract() == 0.0 && value.abs() < 1e21 {
        return format!("{value:.0}");
    }
    let text = format!("{value}");
    if let Some((mantissa, exponent)) = text.split_once('e') {
        let exponent: i32 = exponent.parse().unwrap_or(0);
        return format!("{mantissa}e{:+03}", exponent);
    }
    text
}

/// go/constant's 72-rune truncation for quoted string constants.
fn truncate_constant_string(quoted: String) -> String {
    const MAX_LEN: usize = 72;
    if quoted.chars().count() <= MAX_LEN {
        return quoted;
    }
    let mut truncated: String = quoted.chars().take(MAX_LEN - 3).collect();
    truncated.push_str("...");
    truncated
}

/// Go `time.Duration.String` presentation of a nanosecond integer.
pub(super) fn duration_string(nanoseconds: i64) -> String {
    let negative = nanoseconds < 0;
    let mut value = u128::from(nanoseconds.unsigned_abs());
    let sign = if negative { "-" } else { "" };
    if value < 1_000 {
        return format!("{sign}{value}ns");
    }
    if value < 1_000_000 {
        return sub_second(value, 1_000, "µs", sign);
    }
    if value < 1_000_000_000 {
        return sub_second(value, 1_000_000, "ms", sign);
    }
    let seconds = value / 1_000_000_000;
    value %= 1_000_000_000;
    let (hours, minutes, seconds) = (seconds / 3600, (seconds / 60) % 60, seconds % 60);
    let mut fractional = String::new();
    if value > 0 {
        let mut rest = value;
        for _ in 0..9 {
            fractional.push(char::from(b'0' + (rest * 10 / 1_000_000_000) as u8));
            rest = (rest * 10) % 1_000_000_000;
        }
        while fractional.ends_with('0') {
            fractional.pop();
        }
    }
    let seconds = if fractional.is_empty() {
        format!("{seconds}")
    } else {
        format!("{seconds}.{fractional}")
    };
    if hours > 0 {
        format!("{sign}{hours}h{minutes}m{seconds}s")
    } else if minutes > 0 {
        format!("{sign}{minutes}m{seconds}s")
    } else {
        format!("{sign}{seconds}s")
    }
}

fn sub_second(mut value: u128, unit: u128, suffix: &str, sign: &str) -> String {
    let whole = value / unit;
    value %= unit;
    if value == 0 {
        return format!("{sign}{whole}{suffix}");
    }
    let mut fractional = String::new();
    for _ in 0..3 {
        fractional.push(char::from(b'0' + (value * 10 / unit) as u8));
        value = (value * 10) % unit;
    }
    while fractional.ends_with('0') {
        fractional.pop();
    }
    format!("{sign}{whole}.{fractional}{suffix}")
}

/// Go `strconv.Quote` rendering of a decoded string constant.
pub(super) fn quote_bytes(value: &[u8]) -> String {
    let text = String::from_utf8_lossy(value);
    quote(&text)
}

/// Go `strconv.Quote` rendering of a decoded string constant.
pub(super) fn quote(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{7}' => out.push_str("\\a"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\u{b}' => out.push_str("\\v"),
            ch if ch.is_control() => out.push_str(&format!("\\x{:02x}", ch as u32)),
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
}
