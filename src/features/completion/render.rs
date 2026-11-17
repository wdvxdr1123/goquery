//! Client-aware insertion edits, rendered only after candidate collection is capped.
use crate::analysis::query::completion::{Item, Kind, ParameterLabel};
use crate::syntax::{Lexer, TokenKind};
use crate::text::SourceMap;
use serde_json::{Value, json};
use std::fmt::Write;
use std::ops::Range;

#[derive(Clone, Copy)]
pub(crate) struct CompletionConfig {
    pub(super) snippets: bool,
    pub(super) calls: bool,
    placeholders: bool,
    insert_replace: bool,
    pub(super) documentation: bool,
    pub(super) markdown: bool,
}

impl Default for CompletionConfig {
    fn default() -> Self {
        Self {
            snippets: false,
            calls: true,
            placeholders: false,
            insert_replace: false,
            documentation: true,
            markdown: true,
        }
    }
}

impl CompletionConfig {
    pub(crate) fn from_initialize(params: &Value) -> Result<Self, (i32, String)> {
        let mut result = Self {
            snippets: params["capabilities"]["textDocument"]["completion"]["completionItem"]["snippetSupport"].as_bool() == Some(true),
            insert_replace: params["capabilities"]["textDocument"]["completion"]["completionItem"]["insertReplaceSupport"].as_bool() == Some(true),
            ..Self::default()
        };
        if let Some(formats) = params["capabilities"]["textDocument"]["completion"]["completionItem"]["documentationFormat"].as_array() {
            result.markdown = formats.iter().filter_map(Value::as_str)
                .find(|format| matches!(*format, "markdown" | "plaintext")) != Some("plaintext");
        }
        for (name, target) in [
            ("usePlaceholders", &mut result.placeholders),
            ("completeFunctionCalls", &mut result.calls),
            ("completionDocumentation", &mut result.documentation),
        ] {
            if let Some(value) = params["initializationOptions"].get(name) {
                *target = value.as_bool().ok_or((
                    crate::error::INVALID_PARAMS,
                    format!("{name} must be a boolean"),
                ))?;
            }
        }
        Ok(result)
    }
}

pub(super) struct EditContext<'a> {
    pub config: &'a CompletionConfig,
    pub source: &'a str,
    pub map: &'a SourceMap<'a>,
    pub span: Range<usize>,
    pub cursor: usize,
    pub calls: bool,
}

pub(super) fn completion_item(item: Item, context: &EditContext<'_>) -> Value {
    use tower_lsp::lsp_types::CompletionItemKind as LspKind;
    let kind = match item.kind {
        Kind::Struct => LspKind::STRUCT,
        Kind::Interface => LspKind::INTERFACE,
        Kind::TypeName => LspKind::CLASS,
        Kind::Function => LspKind::FUNCTION,
        Kind::Method => LspKind::METHOD,
        Kind::Variable => LspKind::VARIABLE,
        Kind::Constant => LspKind::CONSTANT,
        Kind::Field => LspKind::FIELD,
    };
    let config = context.config;
    let existing_call = Lexer::new(&context.source[context.span.end..])
        .next()
        .is_some_and(|token| {
            token.kind == TokenKind::LParen
                || item.type_parameters.is_some() && token.kind == TokenKind::LBrack
        });
    let new_text = if config.snippets {
        if let Some(key) = item.field_key
            && key.colon
        {
            format!(
                "{}: ${{1:{}}}{}",
                escape(&item.label),
                if config.placeholders {
                    escape(item.detail.as_deref().unwrap_or(""))
                } else {
                    String::new()
                },
                if key.comma
                    && !Lexer::new(&context.source[context.span.end..])
                        .next()
                        .is_some_and(|token| token.kind == TokenKind::Comma)
                {
                    ","
                } else {
                    ""
                }
            )
        } else if let Some(signature) = &item.signature
            && context.calls
            && !existing_call
            && matches!(item.kind, Kind::Function | Kind::Method)
        {
            snippet(
                &item.label,
                item.type_parameters.as_deref().unwrap_or(&[]),
                &signature.parameters,
                config.placeholders,
            )
        } else {
            escape(&item.label)
        }
    } else {
        item.label.clone()
    };
    let replace = context.map.range(context.span.start, context.span.end);
    let edit = if config.insert_replace {
        let suffix = &context.source[context.cursor..context.span.end];
        let end = if suffix.is_empty() || new_text.contains(suffix) {
            context.span.end
        } else {
            context.cursor
        };
        json!({"newText":new_text,"insert":context.map.range(context.span.start,end),"replace":replace})
    } else {
        json!({"range":replace,"newText":new_text})
    };
    let mut result = json!({"label":item.label,"kind":kind,"filterText":item.label,
        "insertTextFormat":if config.snippets {2} else {1}, "textEdit":edit});
    if let Some(detail) = item.detail {
        result["detail"] = detail.into();
    }
    if let Some(documentation) = item.documentation {
        result["documentation"] = if config.markdown {
            json!({"kind":"markdown","value":documentation})
        } else {
            documentation.into()
        };
    }
    result
}
fn snippet(
    label: &str,
    types: &[String],
    parameters: &[ParameterLabel],
    placeholders: bool,
) -> String {
    let mut result = escape(label);
    let mut next = 1;
    if !types.is_empty() {
        result.push('[');
        if placeholders {
            for (index, ty) in types.iter().enumerate() {
                if index > 0 {
                    result.push_str(", ");
                }
                write!(result, "${{{}:{}}}", next, escape(ty)).expect("string write");
                next += 1;
            }
        } else {
            result.push_str("${1:}");
            next += 1;
        }
        result.push(']');
    }
    result.push('(');
    if placeholders {
        for (index, parameter) in parameters.iter().enumerate() {
            if index > 0 {
                result.push_str(", ");
            }
            write!(result, "${{{}:{}}}", next + index, escape(&parameter.label))
                .expect("string write");
        }
    } else if !parameters.is_empty() {
        write!(result, "${{{next}:}}").expect("string write");
    }
    result.push(')');
    result
}

fn escape(text: &str) -> String {
    let mut result = String::new();
    for character in text.chars() {
        if matches!(character, '\\' | '$' | '}') {
            result.push('\\');
        }
        result.push(character);
    }
    result
}
