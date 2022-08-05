//! Run with `cargo run --example from_std_listener` command.
//!
//! To connect through browser, navigate to "http://localhost:3000" url.

use axum::{routing::get, Router};
use std::net::{SocketAddr, TcpListener};

#[tokio::main]
async fn main() {
    let app = Router::new().route("/", get(|| async { "Hello, world!" }));

    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
    let listener = TcpListener::bind(addr).unwrap();
    println!("listening on {}", addr);
    axum_server::from_tcp(listener)
        .serve(app.into_make_service())
        .await
        .unwrap();
}
