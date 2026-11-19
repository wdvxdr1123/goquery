//! Resource admission around tower-lsp's codec; JSON-RPC decoding stays upstream.
use futures::task::AtomicWaker;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, ReadBuf};

const MAX_HEADER_BYTES: usize = 8192;
const MAX_MESSAGE_BYTES: usize = 32 * 1024 * 1024;

/// Exit status/IO wakeup only; tower-lsp owns the protocol lifecycle state.
#[derive(Default)]
pub(super) struct ProcessStatus {
    shutdown: AtomicBool,
    exited: AtomicBool,
    input_error: AtomicBool,
    input_waker: AtomicWaker,
    pub(super) frame_bytes: AtomicUsize,
}

impl ProcessStatus {
    pub(super) fn shutdown(&self) {
        self.shutdown.store(true, Ordering::Relaxed);
    }

    pub(super) fn exit(&self) {
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
                        Ok(length) => {
                            this.status.frame_bytes.store(length, Ordering::Relaxed);
                            this.remaining = Some(length);
                        }
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
