//! Run with `cargo run --all-features --example async_sni` command.
//!
//! Resolves the certificate from the server name in the ClientHello, before
//! the handshake completes. The built-in `RustlsAcceptor` picks one
//! `ServerConfig` up front; this example defers that choice by implementing
//! `Accept` with `tokio_rustls::LazyConfigAcceptor`.
//!
//! Try it with:
//!
//! ```text
//! curl -k --resolve localhost:3000:127.0.0.1 https://localhost:3000/
//! curl -k --resolve other.test:3000:127.0.0.1 https://other.test:3000/
//! ```
//!
//! The server prints which certificate it resolved for each connection.

use axum::{routing::get, Router};
use axum_server::{accept::Accept, tls_rustls::RustlsConfig};
use rustls::server::Acceptor;
use std::{
    collections::HashMap, future::Future, io, net::SocketAddr, pin::Pin, sync::Arc, time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    time::{timeout_at, Instant},
};
use tokio_rustls::{server::TlsStream, LazyConfigAcceptor};

#[tokio::main]
async fn main() {
    let app = Router::new().route("/", get(|| async { "Hello, world!" }));

    let acceptor = AsyncSniAcceptor::new(CertificateStore::example());
    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));

    println!("listening on {addr}");
    println!("try: curl -k --resolve localhost:3000:127.0.0.1 https://localhost:3000/");
    println!("try: curl -k --resolve other.test:3000:127.0.0.1 https://other.test:3000/");

    axum_server::bind(addr)
        .acceptor(acceptor)
        .serve(app.into_make_service())
        .await
        .unwrap();
}

/// An acceptor that resolves the certificate after reading the ClientHello.
#[derive(Clone)]
struct AsyncSniAcceptor {
    store: CertificateStore,
    handshake_timeout: Duration,
}

impl AsyncSniAcceptor {
    fn new(store: CertificateStore) -> Self {
        Self {
            store,
            handshake_timeout: Duration::from_secs(10),
        }
    }
}

impl<I, S> Accept<I, S> for AsyncSniAcceptor
where
    I: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    S: Send + 'static,
{
    type Stream = TlsStream<I>;
    type Service = S;
    type Future = Pin<Box<dyn Future<Output = io::Result<(Self::Stream, Self::Service)>> + Send>>;

    fn accept(&self, stream: I, service: S) -> Self::Future {
        let store = self.store.clone();
        let handshake_timeout = self.handshake_timeout;

        Box::pin(async move {
            // One deadline covers reading the ClientHello and the handshake.
            let deadline = Instant::now() + handshake_timeout;

            let start = timeout_at(
                deadline,
                LazyConfigAcceptor::new(Acceptor::default(), stream),
            )
            .await
            .map_err(|_| timeout_error())??;

            let server_name = start.client_hello().server_name().map(str::to_owned);
            let config = store.resolve(server_name.as_deref()).await?;

            let stream = timeout_at(deadline, start.into_stream(config.get_inner()))
                .await
                .map_err(|_| timeout_error())??;

            Ok((stream, service))
        })
    }
}

fn timeout_error() -> io::Error {
    io::Error::new(io::ErrorKind::TimedOut, "tls handshake timed out")
}

/// Resolves certificates asynchronously. A real store could query a database,
/// a secret manager, or an ACME directory here.
#[derive(Clone)]
struct CertificateStore {
    certificates: Arc<HashMap<&'static str, (&'static str, &'static str)>>,
}

impl CertificateStore {
    fn example() -> Self {
        Self {
            certificates: Arc::new(HashMap::from([
                (
                    "localhost",
                    (
                        "examples/self-signed-certs/sni/localhost-cert.pem",
                        "examples/self-signed-certs/sni/localhost-key.pem",
                    ),
                ),
                (
                    "other.test",
                    (
                        "examples/self-signed-certs/sni/other-test-cert.pem",
                        "examples/self-signed-certs/sni/other-test-key.pem",
                    ),
                ),
            ])),
        }
    }

    async fn resolve(&self, server_name: Option<&str>) -> io::Result<RustlsConfig> {
        let server_name = server_name.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "client did not send a server name",
            )
        })?;

        let (cert, key) = self.certificates.get(server_name).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("no certificate registered for {server_name}"),
            )
        })?;

        println!("resolving certificate for {server_name}");

        // Async I/O happens here, before the handshake continues. `from_pem_file`
        // also sets the `h2` and `http/1.1` ALPN protocols. A `ServerConfig`
        // built by hand must set them too.
        RustlsConfig::from_pem_file(cert, key).await
    }
}
