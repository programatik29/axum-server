//! Run with `cargo run --all-features --example rustls_session` command.
//!
//! To connect through browser, navigate to "https://localhost:3000" url.

use axum::{middleware::AddExtension, routing::get, Extension, Router};
use axum_server::{
    accept::Accept,
    tls_rustls::{RustlsAcceptor, RustlsConfig},
};
use futures_util::future::BoxFuture;
use std::{io, net::SocketAddr, sync::Arc};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_rustls::server::TlsStream;
use tower::Layer;

#[tokio::main]
async fn main() {
    let app = Router::new().route("/", get(handler));

    let config = RustlsConfig::from_pem_file(
        "examples/self-signed-certs/cert.pem",
        "examples/self-signed-certs/key.pem",
    )
    .await
    .unwrap();

    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));

    println!("listening on {}", addr);

    let acceptor = CustomAcceptor::new(RustlsAcceptor::new(config));
    let server = axum_server::bind(addr).acceptor(acceptor);

    server.serve(app.into_make_service()).await.unwrap();
}

async fn handler(tls_data: Extension<TlsData>) -> String {
    format!("{:?}", tls_data)
}

#[derive(Debug, Clone)]
struct TlsData {
    _hostname: Option<Arc<str>>,
}

#[derive(Debug, Clone)]
struct CustomAcceptor {
    inner: RustlsAcceptor,
}

impl CustomAcceptor {
    fn new(inner: RustlsAcceptor) -> Self {
        Self { inner }
    }
}

impl<I, S> Accept<I, S> for CustomAcceptor
where
    I: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    S: Send + 'static,
{
    type Stream = TlsStream<I>;
    type Service = AddExtension<S, TlsData>;
    type Future = BoxFuture<'static, io::Result<(Self::Stream, Self::Service)>>;

    fn accept(&self, stream: I, service: S) -> Self::Future {
        let acceptor = self.inner.clone();

        Box::pin(async move {
            let (stream, service) = acceptor.accept(stream, service).await?;
            let server_conn = stream.get_ref().1;
            let sni_hostname = TlsData {
                _hostname: server_conn.server_name().map(From::from),
            };
            let service = Extension(sni_hostname).layer(service);

            Ok((stream, service))
        })
    }
}
