// Run with "cargo run --all-features --example record"

use std::net::SocketAddr;

use axum::{
    handler::get,
    extract::Extension,
    Router,
};

use axum_server::record::Recording;

#[tokio::main]
async fn main() {
    let app = Router::new().route("/", get(handler));

    axum_server::bind("127.0.0.1:3000")
        .serve_and_record(app)
        .await
        .unwrap();
}

async fn handler(
    Extension(addr): Extension<SocketAddr>,
    Extension(rec): Extension<Recording>,
) -> String {
    format!(
        "addr: {}\nbytes_sent: {}\nbytes_received: {}",
        addr,
        rec.bytes_sent(),
        rec.bytes_received()
    )
}
