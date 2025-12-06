//! Run with `cargo run --example hello_world_unix` command.
//!
//! To make a request using curl, try `curl --unix-socket /tmp/axum-server.sock http:/localhost`

use axum::{routing::get, Router};
use std::os::unix::net::SocketAddr;

#[tokio::main]
async fn main() {
    let app = Router::new().route("/", get(|| async { "Hello, world!" }));

    let addr = SocketAddr::from_pathname("/tmp/axum-server.sock").unwrap();
    println!("listening on {}", addr.as_pathname().unwrap().display());
    axum_server::bind(addr)
        .serve(app.into_make_service())
        .await
        .unwrap();
}
