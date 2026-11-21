//! Signature-based format calls, shared by tokens and document highlights.
use super::{MAX_FORMAT_BYTES, MAX_PRINTF_CALLS, PrintfCall};
use crate::analysis::budget::MAX_BYTES;
use crate::analysis::sema::{
    DeclId, MethodOrigin, Object, RegionMode, RegionView, Semantics, TypeRef,
};
use crate::syntax::{self, Lexer, Token, TokenKind};
use crate::workspace::Workspace;
use std::collections::HashMap;
use std::ops::Range;
use std::path::Path;

const MAX_SCOPES: usize = 512;
const MAX_ARGUMENT_TOKEN_VISITS: usize = super::MAX_STATEMENTS * super::MAX_NESTED_SCOPES;

pub(super) fn calls_at(
    workspace: &Workspace,
    sema: &mut Semantics<'_>,
    file: &Path,
    source: &str,
    offset: usize,
) -> Vec<PrintfCall> {
    let scan = super::super::cursor_function(workspace, file, source, offset)
        .map_or(0..source.len(), |(start, body)| start..body.end);
    calls(workspace, sema, file, source, offset, scan)
}

fn calls(
    workspace: &Workspace,
    sema: &mut Semantics<'_>,
    file: &Path,
    source: &str,
    cursor: usize,
    scan: Range<usize>,
) -> Vec<PrintfCall> {
    let trace = workspace.trace.queries.then(std::time::Instant::now);
    let mut region_time = std::time::Duration::ZERO;
    let mut signature_time = std::time::Duration::ZERO;
    let Some(text) = source.get(scan.clone()) else {
        return Vec::new();
    };
    let tokens: Vec<_> = Lexer::new(text)
        .map(|mut token| {
            token.span.start += scan.start;
            token.span.end += scan.start;
            token
        })
        .collect();
    let mut last_region: Option<[u32; 2]> = None;
    let mut scopes = 0usize;
    let mut candidates = 0usize;
    let mut out = Vec::new();
    let mut argument_visits = MAX_ARGUMENT_TOKEN_VISITS;
    for (open, token) in tokens.iter().enumerate() {
        if token.span.start > cursor {
            break;
        }
        if candidates >= MAX_PRINTF_CALLS || scopes >= MAX_SCOPES || argument_visits == 0 {
            break;
        }
        if token.kind != TokenKind::LParen
            || open == 0
            || !matches!(
                tokens[open - 1].kind,
                TokenKind::Ident | TokenKind::RParen | TokenKind::RBrack | TokenKind::RBrace
            )
        {
            continue;
        }
        let Some((close, arguments)) = call_arguments(&tokens, open, &mut argument_visits) else {
            continue;
        };
        // Highlighting needs only enclosing calls. Do not build scopes or
        // resolve signatures for unrelated format calls elsewhere in the file.
        if cursor >= tokens[close].span.end {
            continue;
        }
        // This is only a cost prefilter. The literal's position comes from the
        // resolved signature, never its text or the callable's spelling.
        if !arguments.iter().any(|(start, end)| {
            *end == start + 1
                && tokens[*start].kind == TokenKind::String
                && source.get(tokens[*start].span.clone()).is_some_and(|text| {
                    text.len() <= MAX_FORMAT_BYTES
                        && syntax::decode_string_literal(text)
                            .is_some_and(|bytes| bytes.contains(&b'%'))
                })
        }) {
            continue;
        }
        candidates += 1;
        let Some(open_byte) = u32::try_from(token.span.start).ok() else {
            continue;
        };
        let Some(close_end) = u32::try_from(tokens[close].span.end).ok() else {
            continue;
        };

        let phase = workspace.trace.queries.then(std::time::Instant::now);
        let Some(mut view) = sema.region_at(file, source, open_byte, RegionMode::Full) else {
            if let Some(phase) = phase {
                region_time += phase.elapsed();
            }
            continue;
        };
        if last_region != Some(view.span()) {
            last_region = Some(view.span());
            scopes += 1;
        }
        if let Some(phase) = phase {
            region_time += phase.elapsed();
        }

        let sig_phase = workspace.trace.queries.then(std::time::Instant::now);
        let call = format_call_at(workspace, file, source, &mut view, open_byte, close_end);
        if let Some(phase) = sig_phase {
            signature_time += phase.elapsed();
        }
        let Some(call) = call else {
            continue;
        };
        if !arguments
            .get(call.format_index)
            .is_some_and(|&(ts, te)| te == ts + 1 && tokens[ts].kind == TokenKind::String)
        {
            continue;
        }
        let Some(&fmt_span) = call.arg_spans.get(call.format_index) else {
            continue;
        };
        let format_span = fmt_span[0] as usize..fmt_span[1] as usize;
        if super::format_literal_directives(source, format_span.clone()).is_empty() {
            continue;
        }
        out.push(PrintfCall {
            span: call.span[0] as usize..call.span[1] as usize,
            arguments: call
                .arg_spans
                .iter()
                .map(|s| s[0] as usize..s[1] as usize)
                .collect(),
            format_span,
            format_index: call.format_index,
        });
    }
    if let Some(trace) = trace {
        eprintln!(
            "format calls: candidates {candidates}; scopes {scopes} {region_time:?}; signatures {signature_time:?}; argument tokens {}; total {:?}",
            MAX_ARGUMENT_TOKEN_VISITS - argument_visits,
            trace.elapsed(),
        );
    }
    out
}

fn is_single_string_literal(text: &str) -> bool {
    let text = text.trim();
    text.len() <= MAX_FORMAT_BYTES && matches!(text.as_bytes().first(), Some(b'"' | b'`')) && {
        let mut tokens = Lexer::new(text);
        tokens
            .next()
            .is_some_and(|token| token.kind == TokenKind::String && token.span == (0..text.len()))
    }
}

struct FormatCall<'r> {
    span: [u32; 2],
    arg_spans: &'r [[u32; 2]],
    format_index: usize,
}

fn format_call_at<'r, 'a>(
    workspace: &Workspace,
    file: &Path,
    source: &str,
    view: &mut RegionView<'r, 'a>,
    open_byte: u32,
    close_end: u32,
) -> Option<FormatCall<'r>> {
    let call = view.calls().find(|c| {
        !c.is_conversion
            && c.span[1] == close_end
            && (c.open_paren == open_byte
                || (c.callee_span[1] <= open_byte
                    && source
                        .get(c.callee_span[1] as usize..open_byte as usize)
                        .is_some_and(|gap| Lexer::new(gap).next().is_none())))
    })?;
    let format_index = call_format_parameter(
        workspace,
        file,
        source,
        view,
        call.callee_obj,
        call.signature,
    )?;
    Some(FormatCall {
        span: call.span,
        arg_spans: call.arg_spans,
        format_index,
    })
}

fn call_format_parameter<'r, 'a>(
    workspace: &Workspace,
    file: &Path,
    source: &str,
    view: &mut RegionView<'r, 'a>,
    callee_obj: Option<Object<'a>>,
    signature: Option<TypeRef<'r>>,
) -> Option<usize> {
    let sig_param = |view: &mut RegionView<'r, 'a>| {
        signature.and_then(|ty| view.types().format_parameter_index(ty))
    };
    match callee_obj {
        Some(Object::Func(decl)) | Some(Object::Method(MethodOrigin::Concrete(decl))) => {
            sig_param(view).or_else(|| decl_format_parameter(workspace, file, source, decl))
        }
        _ => sig_param(view),
    }
}

fn decl_format_parameter(
    workspace: &Workspace,
    file: &Path,
    source: &str,
    decl: DeclId<'_>,
) -> Option<usize> {
    if !decl.symbol.is_callable() {
        return None;
    }
    let range = decl.symbol.declaration()?;
    let end = decl.symbol.body().map_or(range.end, |body| body.start);
    if end.checked_sub(range.start)? > MAX_BYTES {
        return None;
    }
    let header = if decl.file == file {
        std::borrow::Cow::Borrowed(source.get(range.start..end)?)
    } else {
        std::borrow::Cow::Owned(workspace.source_slice(decl.file, range.start..end)?)
    };
    let signature = syntax::parse_signature(&header);
    (signature.diagnostics.is_empty() && signature.parameters.last().is_some_and(|p| p.variadic))
        .then(|| signature.parameters.len().checked_sub(2))
        .flatten()
}

pub(crate) fn collect_region_format_directives<'a>(
    workspace: &Workspace,
    file: &Path,
    source: &str,
    view: &mut RegionView<'_, 'a>,
    window: Option<(usize, usize)>,
    candidates: &mut usize,
    out: &mut HashMap<Range<usize>, Vec<Range<usize>>>,
) {
    let mut spans = Vec::new();
    for call in view.calls() {
        if *candidates >= MAX_PRINTF_CALLS {
            break;
        }
        if call.is_conversion || call.arg_spans.is_empty() {
            continue;
        }
        if !call.arg_spans.iter().any(|&[start, end]| {
            source
                .get(start as usize..end as usize)
                .is_some_and(|text| {
                    (text.contains('%') || text.contains('\\'))
                        && is_single_string_literal(text)
                        && syntax::decode_string_literal(text.trim())
                            .is_some_and(|bytes| bytes.contains(&b'%'))
                })
        }) {
            continue;
        }
        *candidates += 1;
        let Some(format_index) = call_format_parameter(
            workspace,
            file,
            source,
            view,
            call.callee_obj,
            call.signature,
        ) else {
            continue;
        };
        if let Some(&span) = call.arg_spans.get(format_index) {
            spans.push(span);
        }
    }
    for [s, e] in spans {
        let format_span = s as usize..e as usize;
        if window.is_some_and(|(w_start, w_end)| {
            format_span.start >= w_end || format_span.end <= w_start
        }) {
            continue;
        }
        let Some(fmt_text) = source.get(format_span.clone()) else {
            continue;
        };
        if fmt_text.contains('\n') || !is_single_string_literal(fmt_text) {
            continue;
        }
        let directives = super::format_literal_directives(source, format_span.clone());
        if !directives.is_empty() {
            out.insert(format_span, directives);
        }
    }
}

/// Top-level argument token ranges of one call, starting at its `(`.
fn call_arguments(
    tokens: &[Token],
    open: usize,
    remaining: &mut usize,
) -> Option<(usize, Vec<(usize, usize)>)> {
    let mut depth = 0usize;
    let mut arguments = Vec::new();
    let mut start = open + 1;
    for (index, token) in tokens
        .iter()
        .enumerate()
        .skip(open + 1)
        .take(super::MAX_STATEMENTS)
    {
        *remaining = remaining.checked_sub(1)?;
        match token.kind {
            TokenKind::LParen | TokenKind::LBrack | TokenKind::LBrace => depth += 1,
            TokenKind::RParen if depth == 0 => {
                if start < index {
                    arguments.push((start, index));
                }
                return Some((index, arguments));
            }
            TokenKind::RParen | TokenKind::RBrack | TokenKind::RBrace => {
                depth = depth.checked_sub(1)?;
            }
            TokenKind::Comma if depth == 0 => {
                if start < index {
                    arguments.push((start, index));
                }
                start = index + 1;
            }
            _ => {}
        }
    }
    None
}
