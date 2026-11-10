//! Client-aware insertion edits, rendered only after candidate collection is capped.
use crate::analysis::check::callable_signature::ParameterLabel;
use crate::analysis::query::Context;
use crate::analysis::query::completion::{Kind, Presentation as MemberPresentation};
use crate::analysis::scope::Locals;
use crate::features::completion::collect::Candidate;
use crate::syntax::{Lexer, TokenKind};
use crate::text::SourceMap;
use serde_json::{Value, json};
use std::fmt::Write;
use std::ops::Range;
use std::path::Path;

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

pub(super) struct EditContext<'a, 'workspace> {
    pub query: &'a Context<'workspace>,
    pub file: &'a Path,
    pub source: &'a str,
    pub locals: &'a Locals,
    pub map: &'a SourceMap<'a>,
    pub span: Range<usize>,
    pub cursor: usize,
    pub calls: bool,
    pub receiver: Option<(&'a Path, &'a str)>,
}

pub(super) fn completion_item(
    label: &str,
    candidate: &Candidate<'_>,
    context: &EditContext<'_, '_>,
    member: Option<MemberPresentation>,
) -> Option<Value> {
    use tower_lsp::lsp_types::CompletionItemKind as LspKind;
    let kind = candidate.kind()?;
    let config = &context.query.workspace().completion_config;
    let (member_detail, member_signature, type_parameters, kind) =
        member.map_or((None, None, None, kind), |member| {
            (
                member.detail,
                member.signature,
                member.type_parameters,
                member
                    .kind
                    .map(|kind| match kind {
                        Kind::Struct => LspKind::STRUCT,
                        Kind::Interface => LspKind::INTERFACE,
                        Kind::TypeName => LspKind::CLASS,
                    })
                    .unwrap_or(kind),
            )
        });
    let information = match candidate {
        Candidate::Symbol(_, symbol) if symbol.is_callable() => member_signature,
        Candidate::Local(binding) => context.query.binding_signature(context.locals, binding),
        Candidate::Member(_) if config.snippets => member_signature,
        _ => None,
    };
    let existing_call = Lexer::new(&context.source[context.span.end..])
        .next()
        .is_some_and(|token| {
            token.kind == TokenKind::LParen
                || type_parameters.is_some() && token.kind == TokenKind::LBrack
        });
    let new_text = if config.snippets {
        if let Some((information, _)) = &information
            && context.calls
            && !existing_call
            && (matches!(candidate, Candidate::Symbol(_, symbol) if symbol.is_callable())
                || matches!(candidate, Candidate::Member(member) if member.kind == crate::syntax::MemberKind::Method))
        {
            snippet(
                label,
                type_parameters.as_deref().unwrap_or(&[]),
                &information.parameters,
                config.placeholders,
            )
        } else {
            escape(label)
        }
    } else {
        label.to_owned()
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
    let mut item = json!({"label":label,"kind":kind,"filterText":label,
            "insertTextFormat":if config.snippets {2} else {1}, "textEdit":edit});
    if let Some((information, _)) = information {
        let opening = information.label.find('(').expect("callable signature");
        item["detail"] = format!("func{}", &information.label[opening..]).into();
    } else if let Some(detail) = member_detail.or_else(|| match candidate {
        Candidate::Local(binding) => context.query.binding_detail(context.locals, binding),
        _ => None,
    }) {
        item["detail"] = detail.into();
    }
    Some(item)
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
