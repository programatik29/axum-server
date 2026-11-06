//! Run with `cargo run --all-features --example rustls_session` command.
//!
//! To connect through browser, navigate to "https://localhost:3000" url.
//!
//! Shows the TLS connection information that is available to handlers: the
//! SNI server name, the negotiated ALPN protocol, and client certificates.

use axum::{routing::get, Extension, Router};
use axum_server::{tls::TlsInfo, tls_rustls::RustlsConfig};
use std::{net::SocketAddr, sync::Arc};

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

    axum_server::bind_rustls(addr, config)
        .serve(app.into_make_service())
        .await
        .unwrap();
}

async fn handler(Extension(info): Extension<Arc<TlsInfo>>) -> String {
    format!(
        "server name: {:?}\nalpn: {:?}\nclient certificates: {}",
        info.server_name,
        info.alpn_protocol.as_deref().map(String::from_utf8_lossy),
        info.peer_certificates.len(),
    )
}
