//! Bounded read-ahead for cancellation while tower-lsp executes handlers in order.
use super::transport::ProcessStatus;
use futures::future::BoxFuture;
use futures::task::AtomicWaker;
use serde::Deserialize;
use std::future::poll_fn;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::task::{Context, Poll};
use std::time::Instant;
use tower::Service;
use tower_lsp::jsonrpc::{Id, Request, Response};

const MAX_ADMITTED_MESSAGES: usize = 8;
const READ_AHEAD_BYTES: usize = 1024 * 1024;

tokio::task_local! {
    pub(super) static CANCELLATION: Option<Arc<AtomicBool>>;
}

#[derive(Default)]
struct Admission {
    messages: AtomicUsize,
    bytes: AtomicUsize,
    ready: AtomicWaker,
}

struct Slot {
    admission: Arc<Admission>,
    bytes: usize,
    cancellation: Option<Arc<AtomicBool>>,
}

impl Drop for Slot {
    fn drop(&mut self) {
        if let Some(flag) = &self.cancellation {
            flag.store(true, Ordering::Relaxed);
        }
        self.admission.messages.fetch_sub(1, Ordering::Relaxed);
        self.admission
            .bytes
            .fetch_sub(self.bytes, Ordering::Relaxed);
        self.admission.ready.wake();
    }
}

pub(super) struct ProtocolExtensions<S> {
    inner: Arc<Mutex<S>>,
    status: Arc<ProcessStatus>,
    admission: Arc<Admission>,
    pending: Vec<(Id, Weak<AtomicBool>)>,
    trace: bool,
}

struct RequestTiming {
    admitted: Instant,
    method: String,
    id: Option<String>,
}

impl RequestTiming {
    fn new(request: &Request, cancelled: Option<&Id>) -> Self {
        Self {
            admitted: Instant::now(),
            method: request.method().chars().take(80).collect(),
            id: cancelled.or(request.id()).map(|id| match id {
                Id::String(id) => id.chars().take(64).collect(),
                Id::Number(id) => id.to_string(),
                Id::Null => "null".into(),
            }),
        }
    }

    fn finish(self, started: Instant, response: Option<&Response>) {
        let queued = (started - self.admitted).as_secs_f64() * 1000.0;
        let elapsed = started.elapsed().as_secs_f64() * 1000.0;
        let error = response
            .and_then(Response::error)
            .map(|error| error.code.code());
        eprintln!(
            "lsp method={:?} id={:?} queue_ms={queued:.3} handler_ms={elapsed:.3} error={error:?}",
            self.method, self.id
        );
    }
}

impl<S> ProtocolExtensions<S> {
    pub(super) fn new(inner: S, status: Arc<ProcessStatus>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(inner)),
            status,
            admission: Arc::new(Admission::default()),
            pending: Vec::new(),
            trace: std::env::var_os("GOQUERY_TRACE_LSP").is_some(),
        }
    }
}

impl<S> Service<Request> for ProtocolExtensions<S>
where
    S: Service<Request, Response = Option<Response>> + Send + 'static,
    S::Future: Send + 'static,
    S::Error: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.admission.ready.register(cx.waker());
        if self.admission.messages.load(Ordering::Relaxed) >= MAX_ADMITTED_MESSAGES
            || self.admission.bytes.load(Ordering::Relaxed) >= READ_AHEAD_BYTES
        {
            return Poll::Pending;
        }
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: Request) -> Self::Future {
        self.pending.retain(|(_, flag)| flag.strong_count() != 0);
        let cancelled = if request.method() == "$/cancelRequest" && request.id().is_none() {
            request
                .params()
                .and_then(|params| params.get("id"))
                .and_then(|id| Id::deserialize(id).ok())
        } else {
            None
        };
        if let Some(id) = &cancelled {
            for (_, flag) in self.pending.iter().filter(|(pending, _)| pending == id) {
                if let Some(flag) = flag.upgrade() {
                    flag.store(true, Ordering::Relaxed);
                }
            }
        }
        let cancellation = request.id().map(|id| {
            let flag = Arc::new(AtomicBool::new(false));
            self.pending.push((id.clone(), Arc::downgrade(&flag)));
            flag
        });
        let initialize = request.method() == "initialize";
        let timing = self
            .trace
            .then(|| RequestTiming::new(&request, cancelled.as_ref()));
        if request.method() == "exit" {
            self.status.exit();
        }
        // LimitedInput supplies one frame at a time. A single frame can cross
        // the byte threshold, after which the next decoded frame must wait.
        let bytes = self.status.frame_bytes.load(Ordering::Relaxed);
        self.admission.messages.fetch_add(1, Ordering::Relaxed);
        self.admission.bytes.fetch_add(bytes, Ordering::Relaxed);
        let slot = Slot {
            admission: self.admission.clone(),
            bytes,
            cancellation: cancellation.clone(),
        };
        let inner = self.inner.clone();
        Box::pin(CANCELLATION.scope(cancellation, async move {
            let _slot = slot;
            let started = timing.as_ref().map(|_| Instant::now());
            // Defer tower-lsp routing until this handler reaches the head of
            // the queue, including lifecycle transitions performed by call().
            poll_fn(|cx| inner.lock().unwrap().poll_ready(cx)).await?;
            let future = inner.lock().unwrap().call(request);
            let response = future.await?;
            if let Some((timing, started)) = timing.zip(started) {
                timing.finish(started, response.as_ref());
            }
            Ok(response.map(|response| {
                if !initialize {
                    return response;
                }
                let (id, result) = response.into_parts();
                // lsp-types 0.94.1 omits this implemented capability.
                Response::from_parts(
                    id,
                    result.map(|mut result| {
                        result["capabilities"]["typeHierarchyProvider"] = true.into();
                        result
                    }),
                )
            }))
        }))
    }
}
