//! Hover presents native type, signature and documentation facts as markdown.
use crate::analysis::query::Context;
use crate::analysis::query::hover::{self as hover_query, HoverContent};
use crate::features::documentation;
use serde_json::{Value, json};

pub(super) fn execute(context: &Context<'_>, offset: usize) -> Result<Value, (i32, String)> {
    let Some(content) = hover_query::execute(context, offset)? else {
        return Ok(Value::Null);
    };
    if content.is_empty() {
        return Ok(Value::Null);
    }
    Ok(render(&content))
}

fn render(content: &HoverContent) -> Value {
    let doc = content
        .doc
        .as_deref()
        .map(|text| documentation::markdown(text, &content.doc_links))
        .unwrap_or_default();
    let mut sections: Vec<Vec<String>> = Vec::new();
    sections.push(vec![fenced(content.code.as_deref())]);
    let mut documentation = vec![doc];
    for block in &content.extra {
        documentation.push(fenced(Some(block)));
    }
    sections.push(documentation);
    sections.push(vec![
        content.footer.clone().unwrap_or_default(),
        link(content),
    ]);
    Value::Object(
        [
            (
                "contents".to_owned(),
                json!({"kind": "markdown", "value": assemble(&sections)}),
            ),
            (
                "range".to_owned(),
                json!({
                    "start": {
                        "line": content.range.start.line,
                        "character": content.range.start.utf16_column,
                    },
                    "end": {
                        "line": content.range.end.line,
                        "character": content.range.end.utf16_column,
                    },
                }),
            ),
        ]
        .into_iter()
        .collect(),
    )
}

/// gopls' markdown section assembly: `---` separators between non-empty parts.
fn assemble(sections: &[Vec<String>]) -> String {
    let mut output = String::new();
    for section in sections {
        let start = output.len();
        for part in section {
            if part.is_empty() {
                continue;
            }
            if output.len() == start && start > 0 {
                output.push_str("\n\n---");
            }
            if !output.is_empty() {
                output.push_str("\n\n");
            }
            output.push_str(part);
        }
    }
    output
}

fn fenced(code: Option<&str>) -> String {
    let Some(code) = code else {
        return String::new();
    };
    let trimmed = code.trim_matches('\n');
    if trimmed.is_empty() {
        return String::new();
    }
    format!("```go\n{trimmed}\n```")
}

fn link(content: &HoverContent) -> String {
    let Some(link) = &content.link else {
        return String::new();
    };
    if link.path.is_empty() {
        return String::new();
    }
    let mut url = format!("https://pkg.go.dev/{}", link.path);
    if !link.anchor.is_empty() {
        url.push('#');
        url.push_str(&link.anchor);
    }
    format!("[`{}` on pkg.go.dev]({url})", link.symbol)
}
