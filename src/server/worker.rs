//! One FIFO worker owns the workspace, including indexing and diagnostics.
use crate::workspace::Workspace;
use std::io;
use tokio::sync::{mpsc, oneshot};
use tower_lsp::jsonrpc::{Error, Result};

type Job = Box<dyn FnOnce(&mut Workspace) + Send>;

#[derive(Clone)]
pub(super) struct Worker(mpsc::Sender<Job>);

impl Worker {
    pub(super) fn start() -> io::Result<Self> {
        let (sender, mut receiver) = mpsc::channel::<Job>(1);
        std::thread::Builder::new()
            .name("goquery-workspace".into())
            .spawn(move || {
                let mut workspace = Workspace::default();
                while let Some(job) = receiver.blocking_recv() {
                    job(&mut workspace);
                }
            })?;
        Ok(Self(sender))
    }

    pub(super) async fn run<R: Send + 'static>(
        &self,
        run: impl FnOnce(&mut Workspace) -> R + Send + 'static,
    ) -> Result<R> {
        let (sender, receiver) = oneshot::channel();
        self.0
            .send(Box::new(move |workspace| {
                if !sender.is_closed() {
                    let _ = sender.send(run(workspace));
                }
            }))
            .await
            .map_err(|_| Error::internal_error())?;
        receiver.await.map_err(|_| Error::internal_error())
    }
}
