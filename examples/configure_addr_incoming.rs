//! Run with `cargo run --example configure_http` command.
//!
//! To connect through browser, navigate to "http://localhost:3000" url.

use axum::{routing::get, Router};
use axum_server::AddrIncomingConfig;
use std::net::SocketAddr;
use std::time::Duration;

#[tokio::main]
async fn main() {
    let app = Router::new().route("/", get(|| async { "Hello, world!" }));

    let config = AddrIncomingConfig::new()
        .tcp_nodelay(true)
        .tcp_sleep_on_accept_errors(true)
        .tcp_keepalive(Some(Duration::from_secs(32)))
        .tcp_keepalive_interval(Some(Duration::from_secs(1)))
        .tcp_keepalive_retries(Some(1))
        .build();

    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
    println!("listening on {}", addr);
    axum_server::bind(addr)
        .addr_incoming_config(config)
        .serve(app.into_make_service())
        .await
        .unwrap();
}
