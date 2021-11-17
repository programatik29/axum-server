//! Run with `cargo run --example configure_http` command.
//!
//! To connect through browser, navigate to "http://localhost:3000" url.

use axum::{routing::get, Router};
use axum_server::HttpConfig;
use std::net::SocketAddr;

#[tokio::main]
async fn main() {
    let app = Router::new().route("/", get(|| async { "Hello, world!" }));

    let config = HttpConfig::new()
        .http1_only(true)
        .http2_only(false)
        .max_buf_size(8192)
        .build();

    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
    println!("listening on {}", addr);
    axum_server::bind(addr)
        .http_config(config)
        .serve(app.into_make_service())
        .await
        .unwrap();
}
