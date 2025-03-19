//! Run with `cargo run --example header_read_timeout` command.
//!
//! To connect through browser, navigate to "http://localhost:3000" url.

use axum::{routing::get, Router};
use hyper_util::rt::TokioTimer;
use std::net::{SocketAddr, TcpListener};
use std::time::Duration;

#[tokio::main]
async fn main() {
    let app = Router::new().route("/", get(|| async { "Hello, world!" }));

    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));

    let listener = TcpListener::bind(addr).unwrap();

    println!("listening on {}", addr);

    let mut server = axum_server::from_tcp(listener);

    server.http_builder().http1().timer(TokioTimer::new());
    server
        .http_builder()
        .http1()
        .header_read_timeout(Duration::from_secs(5));

    server.serve(app.into_make_service()).await.unwrap();
}
