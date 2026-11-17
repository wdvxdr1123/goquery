//! Package declaration errors rely on the indexed declaration namespace.
use super::{Code, Diagnostic, Kind, MAX_DIAGNOSTICS, Related};
use crate::index::IndexedFile;
use crate::text::SourceMap;
use std::path::Path;

pub(crate) fn file_diagnostics(
    file: &Path,
    stub: &IndexedFile,
    source: &str,
    rows: &mut Vec<Diagnostic>,
) {
    duplicate_declarations(file, stub, source, rows);
}

fn duplicate_declarations(
    file: &Path,
    stub: &IndexedFile,
    source: &str,
    rows: &mut Vec<Diagnostic>,
) {
    let mut declarations: Vec<&crate::index::IndexedSymbol> = stub
        .symbols
        .iter()
        .filter(|symbol| symbol.receiver.is_none() && symbol.name != "_" && symbol.name != "init")
        .collect();
    declarations.sort_unstable_by_key(|symbol| symbol.start);
    let mut grouped: Vec<Vec<&crate::index::IndexedSymbol>> = Vec::new();
    for symbol in declarations {
        match grouped
            .iter_mut()
            .find(|group| group.first().is_some_and(|first| first.name == symbol.name))
        {
            Some(group) => group.push(symbol),
            None => grouped.push(vec![symbol]),
        }
    }
    for group in grouped.iter().filter(|group| group.len() > 1) {
        let Some(first) = group.first().copied() else {
            continue;
        };
        for later in &group[1..] {
            rows.push(Diagnostic {
                span: later.start..later.end,
                kind: Kind::Compiler(Code::DuplicateDecl),
                message: format!("{} redeclared in this block", first.name),
                unnecessary: false,
                related: vec![Related {
                    span: first.start..first.end,
                    message: format!("other declaration of {}", first.name),
                }],
            });
            rows.push(Diagnostic {
                span: first.start..first.end,
                kind: Kind::Compiler(Code::DuplicateDecl),
                message: format!(
                    "{} redeclared in this block (this error: other declaration of {})",
                    first.name, first.name
                ),
                unnecessary: false,
                related: vec![Related {
                    span: later.start..later.end,
                    message: String::new(),
                }],
            });
            if rows.len() >= MAX_DIAGNOSTICS {
                return;
            }
        }
    }
    duplicate_methods(file, stub, &SourceMap::new(source), rows);
}

fn duplicate_methods(
    file: &Path,
    stub: &IndexedFile,
    map: &SourceMap<'_>,
    rows: &mut Vec<Diagnostic>,
) {
    let mut methods: Vec<&crate::index::IndexedSymbol> = stub
        .symbols
        .iter()
        .filter(|symbol| symbol.receiver.is_some() && symbol.name != "_")
        .collect();
    methods.sort_unstable_by_key(|symbol| symbol.start);
    for (index, method) in methods.iter().enumerate() {
        let Some(receiver) = method.receiver.as_deref().and_then(method_owner) else {
            continue;
        };
        let Some(first) = methods[..index].iter().rev().find(|earlier| {
            earlier.name == method.name
                && earlier
                    .receiver
                    .as_deref()
                    .and_then(method_owner)
                    .is_some_and(|owner| owner == receiver)
        }) else {
            continue;
        };
        let position = map.position(first.start);
        rows.push(Diagnostic {
            span: method.start..method.end,
            kind: Kind::Compiler(Code::DuplicateMethod),
            message: format!(
                "method {}.{} already declared at {}:{}:{}",
                receiver,
                method.name,
                file.display(),
                position.line + 1,
                position.character + 1
            ),
            unnecessary: false,
            related: Vec::new(),
        });
        if rows.len() >= MAX_DIAGNOSTICS {
            return;
        }
    }
}

/// Method identity is the declared base type name. Pointerness does not separate
/// method sets, so `T` and `*T` share one declaration namespace.
fn method_owner(receiver: &str) -> Option<&str> {
    let receiver = receiver.strip_prefix('*').unwrap_or(receiver);
    if receiver.is_empty()
        || receiver
            .chars()
            .any(|ch| !crate::syntax::is_identifier_start(ch) && !ch.is_numeric())
    {
        return None;
    }
    Some(receiver)
}
