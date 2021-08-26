//! Recording utilities for servers.
//!
//! [`Recording`](Recording) type can be used to get data usage.
//!
//! # Example
//!
//! ```rust,no_run
//! use std::net::SocketAddr;
//!
//! use axum::{
//!     handler::get,
//!     extract::Extension,
//!     Router,
//! };
//!
//! use axum_server::record::Recording;
//!
//! #[tokio::main]
//! async fn main() {
//!     let app = Router::new().route("/", get(handler));
//!
//!     axum_server::bind("127.0.0.1:3000")
//!         .serve_and_record(app)
//!         .await
//!         .unwrap();
//! }
//!
//! async fn handler(
//!     Extension(addr): Extension<SocketAddr>,
//!     Extension(rec): Extension<Recording>,
//! ) -> String {
//!     format!(
//!         "addr: {}\nbytes_sent: {}\nbytes_received: {}",
//!         addr,
//!         rec.bytes_sent(),
//!         rec.bytes_received()
//!     )
//! }
//! ```

use crate::server::serve::{shutdown_conns, wait_conns, Accept, Mode, Serve};
use crate::server::Handle;

use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

use http::request::Request;
use http::response::Response;
use http_body::Body;

use tower_http::add_extension::AddExtension;
use tower_service::Service;

use hyper::server::conn::Http;

/// Type to access data that is being recorded in real-time.
#[derive(Clone)]
pub struct Recording {
    sent: Arc<AtomicUsize>,
    received: Arc<AtomicUsize>,
}

impl Recording {
    /// Get recorded outgoing bytes.
    ///
    /// Data might be changed between function calls.
    pub fn bytes_sent(&self) -> usize {
        self.sent.load(Ordering::Acquire)
    }

    /// Get recorded incoming bytes.
    ///
    /// Data might be changed between function calls.
    pub fn bytes_received(&self) -> usize {
        self.received.load(Ordering::Acquire)
    }
}

pub(crate) struct RecordingHttpServer<A, S> {
    acceptor: A,
    service: S,
    handle: Handle,
}

impl<A, S> RecordingHttpServer<A, S> {
    pub fn new(acceptor: A, service: S, handle: Handle) -> Self {
        Self {
            acceptor,
            service,
            handle,
        }
    }
}

impl<A, S, B> Serve for RecordingHttpServer<A, S>
where
    A: Accept<RecordingStream<TcpStream>> + Send + Sync + 'static,
    A::Stream: Send + 'static,
    A::Future: Send + 'static,
    S: Service<Request<hyper::Body>, Response = Response<B>> + Send + Clone + 'static,
    S::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    S::Future: Send,
    B: Body + Send + 'static,
    B::Data: Send,
    B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    fn serve_on(&self, addr: SocketAddr) -> JoinHandle<io::Result<()>> {
        let acceptor = self.acceptor.clone();
        let service = self.service.clone();
        let handle = self.handle.clone();

        tokio::spawn(async move {
            let listener = TcpListener::bind(addr).await?;
            handle.listening.notify_waiters();

            let mut connections = Vec::new();
            let mode;

            mode = loop {
                let (stream, addr) = tokio::select! {
                    result = listener.accept() => result?,
                    _ = handle.shutdown.notified() => break Mode::Shutdown,
                    _ = handle.graceful_shutdown.notified() => break Mode::Graceful,
                };

                let sent = Arc::new(AtomicUsize::new(0));
                let received = Arc::new(AtomicUsize::new(0));
                let acceptor =
                    RecordingAcceptor::new(acceptor.clone(), sent.clone(), received.clone());

                let svc = AddExtension::new(service.clone(), addr);
                let svc = AddExtension::new(svc, Recording { sent, received });

                let conn = tokio::spawn(async move {
                    if let Ok(stream) = acceptor.accept(stream).await {
                        let _ = Http::new()
                            .serve_connection(stream, svc)
                            .with_upgrades()
                            .await;
                    }
                });

                connections.push(conn);
            };

            match mode {
                Mode::Shutdown => shutdown_conns(connections),
                Mode::Graceful => tokio::select! {
                    _ = handle.shutdown.notified() => shutdown_conns(connections),
                    _ = wait_conns(&mut connections) => (),
                },
            }

            Ok(())
        })
    }
}

#[derive(Clone)]
pub(crate) struct RecordingAcceptor<A> {
    inner: A,
    sent: Arc<AtomicUsize>,
    received: Arc<AtomicUsize>,
}

impl<A> RecordingAcceptor<A> {
    fn new(inner: A, sent: Arc<AtomicUsize>, received: Arc<AtomicUsize>) -> Self {
        Self {
            inner,
            sent,
            received,
        }
    }
}

impl<I, A> Accept<I> for RecordingAcceptor<A>
where
    I: AsyncRead + AsyncWrite + Unpin,
    A: Accept<RecordingStream<I>> + Send + Sync + 'static,
    A::Stream: Send + 'static,
    A::Future: Send + 'static,
{
    type Stream = A::Stream;
    type Future = A::Future;

    fn accept(&self, stream: I) -> Self::Future {
        let rec_stream = RecordingStream::new(stream, self.sent.clone(), self.received.clone());

        self.inner.accept(rec_stream)
    }
}

pub(crate) struct RecordingStream<I> {
    inner: I,
    sent: Arc<AtomicUsize>,
    received: Arc<AtomicUsize>,
}

impl<I> RecordingStream<I> {
    pub fn new(inner: I, sent: Arc<AtomicUsize>, received: Arc<AtomicUsize>) -> Self {
        Self {
            inner,
            sent,
            received,
        }
    }
}

impl<I> AsyncRead for RecordingStream<I>
where
    I: AsyncRead + AsyncWrite + Unpin,
{
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let result = Pin::new(&mut self.inner).poll_read(cx, buf);

        self.received
            .fetch_add(buf.filled().len(), Ordering::Release);

        result
    }
}

impl<I> AsyncWrite for RecordingStream<I>
where
    I: AsyncRead + AsyncWrite + Unpin,
{
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.sent.fetch_add(buf.len(), Ordering::Release);

        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}
