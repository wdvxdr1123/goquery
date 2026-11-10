use super::expression::{self, Constraint, ParseError};

/// Reads leading comments only. A directive in a block comment or after the
/// package clause is ignored. Multiple `//go:build` directives are rejected.
/// Legacy `// +build` translation is not implemented by this API.
pub fn from_header(source: &str) -> Result<Option<Constraint>, ParseError> {
    let mut constraint = None;
    let mut in_block = false;
    for line in source.trim_start_matches('\u{feff}').lines() {
        let mut rest = line.trim();
        loop {
            if in_block {
                let Some(end) = rest.find("*/") else {
                    break;
                };
                rest = rest[end + 2..].trim_start();
                in_block = false;
            } else if let Some(comment) = rest.strip_prefix("/*") {
                in_block = true;
                rest = comment;
            } else {
                break;
            }
        }
        if in_block || rest.is_empty() {
            continue;
        }
        if let Some(expression) = rest.strip_prefix("//go:build")
            && (expression.is_empty() || expression.starts_with([' ', '\t']))
        {
            if constraint.is_some() {
                return Err(ParseError {
                    offset: 0,
                    message: "multiple //go:build directives",
                });
            }
            constraint = Some(expression::parse(expression.trim())?);
        } else if !rest.starts_with("//") {
            break;
        }
    }
    Ok(constraint)
}
