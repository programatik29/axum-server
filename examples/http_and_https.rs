//! Run with `cargo run --all-features --example http_and_https` command.
//!
//! To connect through browser, navigate to "http://localhost:3000" url which should redirect to
//! "https://localhost:3443".

use axum::{http::uri::Uri, response::Redirect, routing::get, Router};
use axum_server::tls_rustls::RustlsConfig;
use std::net::SocketAddr;

#[tokio::main]
async fn main() {
    let http = tokio::spawn(http_server());
    let https = tokio::spawn(https_server());

    // Ignore errors.
    let _ = tokio::join!(http, https);
}

async fn http_server() {
    let app = Router::new().route("/", get(http_handler));

    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
    println!("http listening on {}", addr);
    axum_server::bind(addr)
        .serve(app.into_make_service())
        .await
        .unwrap();
}

async fn http_handler(uri: Uri) -> Redirect {
    let uri = format!("https://127.0.0.1:3443{}", uri.path());

    Redirect::temporary(&uri)
}

async fn https_server() {
    let app = Router::new().route("/", get(|| async { "Hello, world!" }));

    let config = RustlsConfig::from_pem_file(
        "examples/self-signed-certs/cert.pem",
        "examples/self-signed-certs/key.pem",
    )
    .await
    .unwrap();

    let addr = SocketAddr::from(([127, 0, 0, 1], 3443));
    println!("https listening on {}", addr);
    axum_server::bind_rustls(addr, config)
        .serve(app.into_make_service())
        .await
        .unwrap();
}
