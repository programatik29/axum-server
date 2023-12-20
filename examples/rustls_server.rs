//! Run with `cargo run --all-features --example rustls_server` command.
//!
//! To connect through browser, navigate to "https://localhost:3000" url.

use axum::{routing::get, Router};
use axum_server::tls_rustls::RustlsConfig;
use std::net::SocketAddr;

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
    println!("listening on {}", addr);
    axum_server::tls_rustls::bind_rustls(addr, config)
        .serve(app.into_make_service())
        .await
        .unwrap();
}
