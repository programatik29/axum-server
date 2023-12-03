//! Run with `cargo run --all-features --example from_std_listener_rustls`
//! command.
//!
//! To connect through browser, navigate to "https://localhost:3000" url.

use axum::{routing::get, Router};
use axum_server::tls_rustls::RustlsConfig;
use std::net::{SocketAddr, TcpListener};

#[tokio::main]
async fn main() {
    let app = Router::new().route("/", get(|| async { "Hello, world!" }));

    let config = RustlsConfig::from_pem_file(
        "examples/self-signed-certs/cert.pem",
        "examples/self-signed-certs/key.pem",
    )
    .await
    .unwrap();

    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
    let listener = TcpListener::bind(addr).unwrap();
    println!("listening on {}", addr);
    axum_server::from_tcp_rustls(listener, config)
        .serve(app.into_make_service())
        .await
        .unwrap();
}
