//! Resource admission around tower-lsp's codec; JSON-RPC decoding stays upstream.
use futures::future::BoxFuture;
use futures::task::AtomicWaker;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, ReadBuf};
use tower::Service;
use tower_lsp::jsonrpc::{Request, Response};

const MAX_HEADER_BYTES: usize = 8192;
const MAX_MESSAGE_BYTES: usize = 32 * 1024 * 1024;

/// Exit status/IO wakeup only; tower-lsp owns the protocol lifecycle state.
#[derive(Default)]
pub(super) struct ProcessStatus {
    shutdown: AtomicBool,
    exited: AtomicBool,
    input_error: AtomicBool,
    input_waker: AtomicWaker,
}

impl ProcessStatus {
    pub(super) fn shutdown(&self) {
        self.shutdown.store(true, Ordering::Relaxed);
    }

    fn exit(&self) {
        self.exited.store(true, Ordering::Relaxed);
        self.input_waker.wake();
    }

    pub(super) fn code(&self) -> i32 {
        i32::from(
            self.input_error.load(Ordering::Relaxed)
                || self.exited.load(Ordering::Relaxed) && !self.shutdown.load(Ordering::Relaxed),
        )
    }
}

pub(super) struct ProtocolExtensions<S> {
    inner: S,
    status: Arc<ProcessStatus>,
}

impl<S> ProtocolExtensions<S> {
    pub(super) fn new(inner: S, status: Arc<ProcessStatus>) -> Self {
        Self { inner, status }
    }
}

impl<S> Service<Request> for ProtocolExtensions<S>
where
    S: Service<Request, Response = Option<Response>>,
    S::Future: Send + 'static,
    S::Error: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: Request) -> Self::Future {
        let initialize = request.method() == "initialize";
        if request.method() == "exit" {
            self.status.exit();
        }
        let future = self.inner.call(request);
        Box::pin(async move {
            let response = future.await?;
            Ok(response.map(|response| {
                if !initialize {
                    return response;
                }
                let (id, result) = response.into_parts();
                // lsp-types 0.94.1 has type-hierarchy requests but omits this
                // standard capability. Keep advertising the implemented feature.
                Response::from_parts(
                    id,
                    result.map(|mut result| {
                        result["capabilities"]["typeHierarchyProvider"] = true.into();
                        result
                    }),
                )
            }))
        })
    }
}

/// Exposes one frame at a time, validates its size before admitting a body, and
/// stops input on exit even when the client keeps its stdin pipe open.
pub(super) struct LimitedInput<I> {
    input: I,
    status: Arc<ProcessStatus>,
    header: Vec<u8>,
    sent: usize,
    remaining: Option<usize>,
}

impl<I> LimitedInput<I> {
    pub(super) fn new(input: I, status: Arc<ProcessStatus>) -> Self {
        Self {
            input,
            status,
            header: Vec::new(),
            sent: 0,
            remaining: None,
        }
    }

    fn length(&self) -> io::Result<usize> {
        let header = std::str::from_utf8(&self.header).map_err(io::Error::other)?;
        let mut length = None;
        for line in header.lines() {
            if let Some((name, value)) = line.split_once(':')
                && name.eq_ignore_ascii_case("Content-Length")
            {
                if length.is_some() {
                    return Err(io::Error::other("duplicate Content-Length"));
                }
                length = Some(value.trim().parse::<usize>().map_err(io::Error::other)?);
            }
        }
        let length = length.ok_or_else(|| io::Error::other("missing Content-Length"))?;
        if length == 0 {
            return Err(io::Error::other("empty message body"));
        }
        if length > MAX_MESSAGE_BYTES {
            return Err(io::Error::other("message too large"));
        }
        Ok(length)
    }
}

impl<I: AsyncRead + Unpin> AsyncRead for LimitedInput<I> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        output: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        this.status.input_waker.register(cx.waker());
        if output.remaining() == 0
            || this.status.exited.load(Ordering::Relaxed)
            || this.status.input_error.load(Ordering::Relaxed)
        {
            return Poll::Ready(Ok(()));
        }
        let result = (|| {
            while this.remaining.is_none() {
                let mut byte = [0];
                let mut buffer = ReadBuf::new(&mut byte);
                match Pin::new(&mut this.input).poll_read(cx, &mut buffer) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                    Poll::Ready(Ok(())) if buffer.filled().is_empty() => {
                        return Poll::Ready(if this.header.is_empty() {
                            Ok(())
                        } else {
                            Err(io::Error::other("truncated header"))
                        });
                    }
                    Poll::Ready(Ok(())) => {}
                }
                if this.header.len() == MAX_HEADER_BYTES {
                    return Poll::Ready(Err(io::Error::other("header too large")));
                }
                this.header.push(byte[0]);
                if this.header.ends_with(b"\r\n\r\n") || this.header.ends_with(b"\n\n") {
                    match this.length() {
                        Ok(length) => this.remaining = Some(length),
                        Err(error) => return Poll::Ready(Err(error)),
                    }
                }
            }
            if this.sent < this.header.len() {
                let end = (this.sent + output.remaining()).min(this.header.len());
                output.put_slice(&this.header[this.sent..end]);
                this.sent = end;
                return Poll::Ready(Ok(()));
            }
            let remaining = this.remaining.expect("validated frame length");
            let count = remaining.min(output.remaining());
            let mut buffer = ReadBuf::new(&mut output.initialize_unfilled_to(count)[..count]);
            match Pin::new(&mut this.input).poll_read(cx, &mut buffer) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                Poll::Ready(Ok(())) => {}
            }
            let read = buffer.filled().len();
            output.advance(read);
            if read == 0 && remaining != 0 {
                return Poll::Ready(Err(io::Error::other("truncated body")));
            }
            let remaining = remaining - read;
            this.remaining = Some(remaining);
            if remaining == 0 {
                this.header.clear();
                this.sent = 0;
                this.remaining = None;
            }
            Poll::Ready(Ok(()))
        })();
        if matches!(result, Poll::Ready(Err(_))) {
            this.status.input_error.store(true, Ordering::Relaxed);
        }
        result
    }
}
