// Run with "cargo run --example remote_address"

use axum::{extract::Extension, handler::get, Router};
use std::net::SocketAddr;

#[tokio::main]
async fn main() {
    let app = Router::new().route("/", get(handler));

    axum_server::bind("127.0.0.1:3000")
        .serve(app)
        .await
        .unwrap();
}

async fn handler(Extension(addr): Extension<SocketAddr>) -> String {
    format!("addr: {}", addr)
}
