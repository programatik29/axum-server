//! Run with `cargo run --example shutdown` command.
//!
//! To connect through browser, navigate to "http://localhost:3000" url.
//!
//! Server will shutdown in 20 seconds.

use axum::{routing::get, Router};
use axum_server::Handle;
use std::{net::SocketAddr, time::Duration};
use tokio::time::sleep;

#[tokio::main]
async fn main() {
    let app = Router::new().route("/", get(|| async { "Hello, world!" }));

    let handle = Handle::new();

    // Spawn a task to shutdown server.
    tokio::spawn(shutdown(handle.clone()));

    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
    println!("listening on {}", addr);
    axum_server::bind(addr)
        .handle(handle)
        .serve(app.into_make_service())
        .await
        .unwrap();

    println!("server is shut down");
}

async fn shutdown(handle: Handle) {
    // Wait 20 seconds.
    sleep(Duration::from_secs(20)).await;

    println!("sending shutdown signal");

    // Signal the server to shutdown using Handle.
    handle.shutdown();
}
