// Run with "cargo run --example shutdown"

use axum::{
    handler::get,
    Router,
};
use axum_server::Handle;
use tokio::time::{sleep, Duration};

#[tokio::main]
async fn main() {
    let app = Router::new().route("/", get(|| async { "Hello, World!" }));

    let handle = Handle::new();

    tokio::spawn(shutdown_in_twenty_secs(handle.clone()));

    axum_server::bind("127.0.0.1:3000")
        .handle(handle)
        .serve(app)
        .await
        .unwrap();
}

async fn shutdown_in_twenty_secs(handle: Handle) {
    sleep(Duration::from_secs(20)).await;

    handle.shutdown();
}
