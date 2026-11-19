//! tower-lsp owns framing, lifecycle, standard routing and custom-method registration.
use super::backend::Backend;
use super::scheduling::ProtocolExtensions;
use super::transport::{LimitedInput, ProcessStatus};
use super::worker::Worker;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncWrite, BufReader};
use tower_lsp::{LspService, Server};

pub(crate) async fn serve(input: impl AsyncRead + Unpin, output: impl AsyncWrite) -> i32 {
    let status = Arc::new(ProcessStatus::default());
    let worker = match Worker::start() {
        Ok(worker) => worker,
        Err(error) => {
            eprintln!("start workspace worker: {error}");
            return 1;
        }
    };
    let (service, socket) =
        LspService::build(|client| Backend::new(client, status.clone(), worker))
            .custom_method(
                "goquery/embedding/supertypes",
                Backend::embedding_supertypes,
            )
            .custom_method("goquery/embedding/subtypes", Backend::embedding_subtypes)
            .finish();
    let service = ProtocolExtensions::new(service, status.clone());
    let input = LimitedInput::new(BufReader::new(input), status.clone());
    Server::new(input, output, socket)
        .concurrency_level(1)
        .serve(service)
        .await;
    status.code()
}
