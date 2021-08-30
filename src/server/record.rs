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

use crate::server::http_server::{HttpServer, NoopAcceptor};
use crate::server::{Accept, Handle, MakeParts};

use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use http::uri::Scheme;

use tower_http::add_extension::AddExtension;
use tower_layer::Layer;

/// Type to access data that is being recorded in real-time.
#[derive(Default, Clone)]
pub struct Recording {
    inner: Arc<RecordingInner>,
}

#[derive(Default)]
struct RecordingInner {
    sent: AtomicUsize,
    received: AtomicUsize,
}

impl Recording {
    /// Get recorded outgoing bytes.
    ///
    /// Data might be changed between function calls.
    pub fn bytes_sent(&self) -> usize {
        self.inner.sent.load(Ordering::Acquire)
    }

    /// Get recorded incoming bytes.
    ///
    /// Data might be changed between function calls.
    pub fn bytes_received(&self) -> usize {
        self.inner.received.load(Ordering::Acquire)
    }
}

#[cfg(feature = "tls-rustls")]
impl<S, A> HttpServer<S, MakeRecordingParts<A>> {
    pub(crate) fn recording_new(scheme: Scheme, service: S, handle: Handle, acceptor: A) -> Self {
        HttpServer::new(scheme, service, handle, MakeRecordingParts::new(acceptor))
    }
}

impl<S> HttpServer<S, MakeRecordingParts<NoopAcceptor>> {
    pub(crate) fn recording_from_service(scheme: Scheme, service: S, handle: Handle) -> Self {
        HttpServer::new(scheme, service, handle, MakeRecordingParts::noop())
    }
}

#[derive(Clone)]
pub(crate) struct MakeRecordingParts<A> {
    acceptor: A,
}

impl<A> MakeRecordingParts<A> {
    fn new(acceptor: A) -> Self {
        Self { acceptor }
    }
}

impl MakeRecordingParts<NoopAcceptor> {
    fn noop() -> Self {
        MakeRecordingParts::new(NoopAcceptor)
    }
}

impl<A> MakeParts for MakeRecordingParts<A>
where
    A: Clone,
{
    type Layer = RecordingLayer;
    type Acceptor = RecordingAcceptor<A>;

    fn make_parts(&self) -> (Self::Layer, Self::Acceptor) {
        let recording = Recording::default();

        let layer = RecordingLayer::new(recording.clone());
        let acceptor = RecordingAcceptor::new(self.acceptor.clone(), recording);

        (layer, acceptor)
    }
}

#[derive(Clone)]
pub(crate) struct RecordingLayer {
    recording: Recording,
}

impl RecordingLayer {
    pub(crate) fn new(recording: Recording) -> Self {
        Self { recording }
    }
}

impl<S> Layer<S> for RecordingLayer {
    type Service = AddExtension<S, Recording>;

    fn layer(&self, service: S) -> Self::Service {
        AddExtension::new(service, self.recording.clone())
    }
}

#[derive(Clone)]
pub(crate) struct RecordingAcceptor<A> {
    inner: A,
    recording: Recording,
}

impl<A> RecordingAcceptor<A> {
    pub(crate) fn new(inner: A, recording: Recording) -> Self {
        Self { inner, recording }
    }
}

impl<I, A> Accept<I> for RecordingAcceptor<A>
where
    I: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    A: Accept<RecordingStream<I>>,
{
    type Conn = A::Conn;
    type Future = A::Future;

    fn accept(&self, stream: I) -> Self::Future {
        let rec_stream = RecordingStream::new(stream, self.recording.clone());

        self.inner.accept(rec_stream)
    }
}

pub(crate) struct RecordingStream<I> {
    inner: I,
    recording: Recording,
}

impl<I> RecordingStream<I> {
    pub(crate) fn new(inner: I, recording: Recording) -> Self {
        Self { inner, recording }
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

        self.recording
            .inner
            .received
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
        self.recording
            .inner
            .sent
            .fetch_add(buf.len(), Ordering::Release);

        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::Recording;

    use crate::server::tests::{empty_request, http_client, into_text};
    use crate::{Handle, Server};

    use axum::{extract::Extension, handler::get};
    use tower_service::Service;
    use tower_util::ServiceExt;

    #[ignore]
    #[tokio::test]
    async fn test_record() {
        let app = get(handler);

        let handle = Handle::new();
        let server = Server::new()
            .bind("127.0.0.1:0")
            .bind("127.0.0.1:0")
            .handle(handle.clone())
            .serve_and_record(app);

        tokio::spawn(server);

        handle.listening().await;

        let addrs = handle.listening_addrs().unwrap();
        println!("listening addrs: {:?}", addrs);

        for addr in addrs {
            let mut client = http_client(addr).await.unwrap();

            let req = empty_request(&format!("http://127.0.0.1:{}/", addr.port()));

            let resp = client.ready_and().await.unwrap().call(req).await.unwrap();

            assert_ne!(into_text(resp).await, "0");
        }
    }

    #[cfg(feature = "tls-rustls")]
    use crate::server::tls::tests::{https_client, CERTIFICATE, PRIVATE_KEY};

    #[ignore]
    #[tokio::test]
    #[cfg(feature = "tls-rustls")]
    async fn test_tls_record() {
        let key = PRIVATE_KEY.as_bytes().to_vec();
        let cert = CERTIFICATE.as_bytes().to_vec();

        let app = get(handler);

        let handle = Handle::new();
        let server = Server::new()
            .bind_rustls("127.0.0.1:0")
            .bind_rustls("127.0.0.1:0")
            .private_key(key)
            .certificate(cert)
            .handle(handle.clone())
            .serve_and_record(app);

        tokio::spawn(server);

        handle.listening().await;

        let addrs = handle.listening_addrs().unwrap();
        println!("listening addrs: {:?}", addrs);

        for addr in addrs {
            let mut client = https_client(addr).await.unwrap();

            let req = empty_request(&format!("http://127.0.0.1:{}/", addr.port()));

            let resp = client.ready_and().await.unwrap().call(req).await.unwrap();

            assert_ne!(into_text(resp).await, "0");
        }
    }

    async fn handler(Extension(rec): Extension<Recording>) -> String {
        format!("{}", rec.bytes_received())
    }
}
