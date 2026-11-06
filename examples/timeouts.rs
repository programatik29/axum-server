//! Run with `cargo run --example timeouts` command.
//!
//! Demonstrates the connection and request timeouts:
//!
//! - `first_request_timeout` closes a new connection if no request arrives in
//!   time, including while hyper is detecting the HTTP protocol.
//! - `header_read_timeout` bounds reading the headers of a request.
//! - `keep_alive_interval` and `keep_alive_timeout` bound idle HTTP/2
//!   connections.
//!
//! For TLS, `RustlsAcceptor::handshake_timeout` and
//! `OpenSSLAcceptor::handshake_timeout` bound the TLS handshake.

use axum::{routing::get, Router};
use axum_server::Server;
use std::{net::SocketAddr, time::Duration};

#[tokio::main]
async fn main() {
    let app = Router::new().route("/", get(|| async { "Hello, world!" }));

    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));

    let mut server = Server::bind(addr).first_request_timeout(Duration::from_secs(10));

    let mut http1 = server.http_builder().http1();
    http1.header_read_timeout(Duration::from_secs(10));

    let mut http2 = server.http_builder().http2();
    http2.keep_alive_interval(Duration::from_secs(30));
    http2.keep_alive_timeout(Duration::from_secs(10));

    println!("listening on {addr}");
    server.serve(app.into_make_service()).await.unwrap();
}
