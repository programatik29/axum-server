//! Run with `cargo run --example graceful_shutdown` command.
//!
//! To connect through browser, navigate to "http://localhost:3000" url.
//!
//! After 10 seconds:
//!  - If there aren't any connections alive, server will shutdown.
//!  - If there are connections alive, server will wait until deadline is elapsed.
//!  - Deadline is 30 seconds. Server will shutdown anyways when deadline is elapsed.

use axum::{routing::get, Router};
use axum_server::Handle;
use std::{net::SocketAddr, time::Duration};
use tokio::time::sleep;

#[tokio::main]
async fn main() {
    let app = Router::new().route("/", get(|| async { "Hello, world!" }));

    let handle = Handle::new();

    // Spawn a task to gracefully shutdown server.
    tokio::spawn(graceful_shutdown(handle.clone()));

    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
    println!("listening on {}", addr);
    axum_server::bind(addr)
        .handle(handle)
        .serve(app.into_make_service())
        .await
        .unwrap();

    println!("server is shut down");
}

async fn graceful_shutdown(handle: Handle) {
    // Wait 10 seconds.
    sleep(Duration::from_secs(10)).await;

    println!("sending graceful shutdown signal");

    // Signal the server to shutdown using Handle.
    handle.graceful_shutdown(Some(Duration::from_secs(30)));

    // Print alive connection count every second.
    loop {
        sleep(Duration::from_secs(1)).await;

        println!("alive connections: {}", handle.connection_count());
    }
}
