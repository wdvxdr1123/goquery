//! tower-lsp owns framing, lifecycle, standard routing and custom-method registration.
use super::backend::Backend;
use super::transport::{LimitedInput, ProcessStatus, ProtocolExtensions};
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncWrite, BufReader};
use tower::limit::ConcurrencyLimit;
use tower_lsp::{LspService, Server};

pub(crate) async fn serve(input: impl AsyncRead + Unpin, output: impl AsyncWrite) -> i32 {
    let status = Arc::new(ProcessStatus::default());
    let (service, socket) = LspService::build(|client| Backend::new(client, status.clone()))
        .custom_method(
            "goquery/embedding/supertypes",
            Backend::embedding_supertypes,
        )
        .custom_method("goquery/embedding/subtypes", Backend::embedding_subtypes)
        .finish();
    // The readiness gate prevents tower-lsp's 100-slot task queue from admitting
    // more than one in-flight operation and one decoded waiting message.
    let service = ConcurrencyLimit::new(ProtocolExtensions::new(service, status.clone()), 1);
    let input = LimitedInput::new(BufReader::new(input), status.clone());
    Server::new(input, output, socket)
        .concurrency_level(1)
        .serve(service)
        .await;
    status.code()
}
