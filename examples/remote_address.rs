//! Run with `cargo run --example remote_address` command.
//!
//! To connect through browser, navigate to "http://localhost:3000" url.

use axum::{extract::ConnectInfo, routing::get, Router};
use std::net::SocketAddr;

#[tokio::main]
async fn main() {
    let app = Router::new()
        .route("/", get(handler))
        .into_make_service_with_connect_info::<SocketAddr>();

    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));

    axum_server::bind(addr).serve(app).await.unwrap();
}

async fn handler(ConnectInfo(addr): ConnectInfo<SocketAddr>) -> String {
    format!("your ip address is: {}", addr)
}
