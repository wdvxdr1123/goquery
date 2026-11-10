//! Typed feature entry points. Wire method decoding belongs to server dispatch.
use crate::analysis::query::Context;
use crate::text;
use crate::workspace::Workspace;
use serde_json::Value;
use std::path::Path;

pub(crate) enum CallDirection {
    Incoming,
    Outgoing,
}
pub(crate) enum TypeDirection {
    Supertypes,
    Subtypes,
}

pub(crate) enum Query<'a> {
    WorkspaceSymbols(&'a Value),
    Calls(CallDirection, &'a Value),
    Types(TypeDirection, &'a Value),
    Embedding(TypeDirection, &'a Value),
    Document {
        file: &'a Path,
        query: DocumentQuery<'a>,
    },
}

pub(crate) enum DocumentQuery<'a> {
    Symbols,
    SemanticTokens,
    SemanticTokensRange(&'a Value),
    Diagnostics,
    FoldingRange,
    CodeLens,
    Completion(&'a Value),
    SignatureHelp(&'a Value),
    Definition(&'a Value),
    Declaration(&'a Value),
    TypeDefinition(&'a Value),
    Hover(&'a Value),
    PrepareRename(&'a Value),
    Rename(&'a Value),
    Implementation(&'a Value),
    DocumentHighlight(&'a Value),
    PrepareCallHierarchy(&'a Value),
    PrepareTypeHierarchy(&'a Value),
    References(&'a Value),
    Formatting(&'a Value),
    CodeAction(&'a Value),
    DocumentLink,
}

pub(crate) fn execute(workspace: &mut Workspace, query: Query<'_>) -> Result<Value, (i32, String)> {
    let (file, query) = match query {
        Query::WorkspaceSymbols(params) => {
            return super::symbol_search::execute(&Context::metadata(workspace), params);
        }
        Query::Calls(direction, params) => {
            return super::call_hierarchy::execute(
                &Context::metadata(workspace),
                params,
                matches!(direction, CallDirection::Incoming),
            );
        }
        Query::Types(direction, params) => {
            return super::type_hierarchy::execute(
                &Context::metadata(workspace),
                params,
                matches!(direction, TypeDirection::Supertypes),
                super::type_hierarchy::Relation::Structural,
            );
        }
        Query::Embedding(direction, params) => {
            return super::type_hierarchy::execute(
                &Context::metadata(workspace),
                params,
                matches!(direction, TypeDirection::Supertypes),
                super::type_hierarchy::Relation::Embedding,
            );
        }
        Query::Document { file, query } => (file, query),
    };
    // Syntactic features only need the admitted source text; documents excluded
    // by build constraints or discovery rules still answer them, as gopls does.
    if matches!(
        query,
        DocumentQuery::FoldingRange
            | DocumentQuery::CodeLens
            | DocumentQuery::Formatting(_)
            | DocumentQuery::DocumentLink
            | DocumentQuery::Symbols
    ) {
        let hierarchical = workspace.hierarchical_document_symbols;
        let Ok(mut context) = Context::source_only(workspace, file) else {
            return Ok(Value::Null);
        };
        return match query {
            DocumentQuery::FoldingRange => super::folding_range::execute(&context),
            DocumentQuery::CodeLens => super::code_lens::execute(&context),
            DocumentQuery::DocumentLink => super::document_link::execute(&context),
            DocumentQuery::Symbols => super::document_symbol::execute(&context, hierarchical),
            DocumentQuery::Formatting(params) => super::formatting::execute(&mut context, params),
            _ => unreachable!("syntax-only dispatch"),
        };
    }
    let mut context = match Context::new(workspace, file) {
        Ok(context) => context,
        Err(error) => {
            // gopls keeps answering open documents outside the build scope through
            // an orphaned-file package. Until that native path exists, semantic
            // requests return no result instead of a protocol error.
            if Context::source_only(workspace, file).is_ok() {
                return Ok(Value::Null);
            }
            return Err(error);
        }
    };
    match query {
        DocumentQuery::SemanticTokens => return super::highlighting::execute(&context),
        DocumentQuery::SemanticTokensRange(params) => {
            return super::highlighting::execute_range(&context, params);
        }
        DocumentQuery::Diagnostics => return super::diagnostics::execute(&context),
        DocumentQuery::CodeAction(params) => return super::code_actions::execute(&context, params),
        _ => {}
    }
    let source = context.source();
    let params = match query {
        DocumentQuery::Completion(params)
        | DocumentQuery::SignatureHelp(params)
        | DocumentQuery::Definition(params)
        | DocumentQuery::Declaration(params)
        | DocumentQuery::Hover(params)
        | DocumentQuery::PrepareRename(params)
        | DocumentQuery::Rename(params)
        | DocumentQuery::Implementation(params)
        | DocumentQuery::DocumentHighlight(params)
        | DocumentQuery::PrepareCallHierarchy(params)
        | DocumentQuery::PrepareTypeHierarchy(params)
        | DocumentQuery::References(params)
        | DocumentQuery::TypeDefinition(params) => params,
        DocumentQuery::Symbols
        | DocumentQuery::SemanticTokens
        | DocumentQuery::SemanticTokensRange(_)
        | DocumentQuery::Diagnostics
        | DocumentQuery::FoldingRange
        | DocumentQuery::CodeLens
        | DocumentQuery::Formatting(_)
        | DocumentQuery::CodeAction(_)
        | DocumentQuery::DocumentLink => {
            unreachable!("handled before position decoding")
        }
    };
    let offset = text::offset(source, &params["position"]).ok_or((
        crate::error::INVALID_PARAMS,
        "invalid UTF-16 position".into(),
    ))?;
    match query {
        DocumentQuery::TypeDefinition(_) => super::type_definition::execute(&mut context, offset),
        DocumentQuery::Completion(_) => super::completion::execute(&context, offset),
        DocumentQuery::SignatureHelp(_) => super::signature_help::execute(&context, offset, params),
        DocumentQuery::Definition(_) => {
            super::navigation::execute(&context, offset, "textDocument/definition")
        }
        DocumentQuery::Declaration(_) => {
            super::navigation::execute(&context, offset, "textDocument/declaration")
        }
        DocumentQuery::Hover(_) => super::hover::execute(&context, offset),
        DocumentQuery::PrepareRename(_) => super::rename::prepare(&context, offset),
        DocumentQuery::Rename(_) => super::rename::execute(&mut context, offset, params),
        DocumentQuery::Implementation(_) => super::implementation::execute(&context, offset),
        DocumentQuery::DocumentHighlight(_) => super::document_highlight::execute(&context, offset),
        DocumentQuery::PrepareCallHierarchy(_) => {
            super::navigation::execute(&context, offset, "textDocument/prepareCallHierarchy")
        }
        DocumentQuery::PrepareTypeHierarchy(_) => {
            super::navigation::execute(&context, offset, "textDocument/prepareTypeHierarchy")
        }
        DocumentQuery::References(_) => super::references::execute(context, offset, params),
        DocumentQuery::Symbols
        | DocumentQuery::SemanticTokens
        | DocumentQuery::SemanticTokensRange(_)
        | DocumentQuery::Diagnostics
        | DocumentQuery::FoldingRange
        | DocumentQuery::CodeLens
        | DocumentQuery::Formatting(_)
        | DocumentQuery::CodeAction(_)
        | DocumentQuery::DocumentLink => {
            unreachable!("handled before positioned access")
        }
    }
}

// Protocol coordinates are constructed only after semantic candidate sources drop.
pub(super) fn source_range(
    range: crate::analysis::query::SourceRange,
) -> tower_lsp::lsp_types::Range {
    tower_lsp::lsp_types::Range {
        start: tower_lsp::lsp_types::Position {
            line: range.start.line,
            character: range.start.utf16_column,
        },
        end: tower_lsp::lsp_types::Position {
            line: range.end.line,
            character: range.end.utf16_column,
        },
    }
}
