//! Capabilities use tower-lsp's wire types; only implemented features are advertised.
use crate::features::SemanticConfig;
use tower_lsp::lsp_types as lsp;

pub(super) fn initialize_result(semantic: &SemanticConfig) -> lsp::InitializeResult {
    let legend = semantic.legend();
    lsp::InitializeResult {
        capabilities: lsp::ServerCapabilities {
            position_encoding: Some(lsp::PositionEncodingKind::UTF16),
            text_document_sync: Some(lsp::TextDocumentSyncCapability::Kind(
                lsp::TextDocumentSyncKind::FULL,
            )),
            type_definition_provider: Some(lsp::TypeDefinitionProviderCapability::Simple(true)),
            hover_provider: Some(lsp::HoverProviderCapability::Simple(true)),
            implementation_provider: Some(lsp::ImplementationProviderCapability::Simple(true)),
            document_highlight_provider: Some(lsp::OneOf::Left(true)),
            document_link_provider: Some(lsp::DocumentLinkOptions {
                resolve_provider: Some(false),
                work_done_progress_options: lsp::WorkDoneProgressOptions::default(),
            }),
            rename_provider: Some(lsp::OneOf::Right(lsp::RenameOptions {
                prepare_provider: Some(true),
                work_done_progress_options: lsp::WorkDoneProgressOptions::default(),
            })),
            folding_range_provider: Some(lsp::FoldingRangeProviderCapability::Simple(true)),
            code_lens_provider: Some(lsp::CodeLensOptions {
                resolve_provider: Some(false),
            }),
            document_formatting_provider: Some(lsp::OneOf::Left(true)),
            code_action_provider: Some(lsp::CodeActionProviderCapability::Options(
                lsp::CodeActionOptions {
                    code_action_kinds: Some(vec![lsp::CodeActionKind::QUICKFIX]),
                    work_done_progress_options: lsp::WorkDoneProgressOptions::default(),
                    resolve_provider: Some(false),
                },
            )),
            definition_provider: Some(lsp::OneOf::Left(true)),
            declaration_provider: Some(lsp::DeclarationCapability::Simple(true)),
            references_provider: Some(lsp::OneOf::Left(true)),
            call_hierarchy_provider: Some(lsp::CallHierarchyServerCapability::Simple(true)),
            document_symbol_provider: Some(lsp::OneOf::Left(true)),
            workspace_symbol_provider: Some(lsp::OneOf::Left(true)),
            workspace: Some(lsp::WorkspaceServerCapabilities {
                workspace_folders: Some(lsp::WorkspaceFoldersServerCapabilities {
                    supported: Some(true),
                    change_notifications: Some(lsp::OneOf::Left(true)),
                }),
                file_operations: None,
            }),
            completion_provider: Some(lsp::CompletionOptions {
                trigger_characters: Some(vec![".".into()]),
                ..Default::default()
            }),
            signature_help_provider: Some(lsp::SignatureHelpOptions {
                trigger_characters: Some(vec!["(".into(), ",".into()]),
                retrigger_characters: Some(vec![")".into()]),
                ..Default::default()
            }),
            semantic_tokens_provider: Some(
                lsp::SemanticTokensServerCapabilities::SemanticTokensOptions(
                    lsp::SemanticTokensOptions {
                        legend: lsp::SemanticTokensLegend {
                            token_types: legend.token_types,
                            token_modifiers: legend.token_modifiers,
                        },
                        full: Some(lsp::SemanticTokensFullOptions::Bool(true)),
                        range: Some(true),
                        work_done_progress_options: lsp::WorkDoneProgressOptions::default(),
                    },
                ),
            ),
            ..Default::default()
        },
        server_info: Some(lsp::ServerInfo {
            name: "goquery".into(),
            version: Some(env!("CARGO_PKG_VERSION").into()),
        }),
    }
}
