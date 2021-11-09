//! Run with `cargo run --all-features --example rustls_reload` command.
//!
//! To connect through browser, navigate to "https://localhost:3000" url.
//!
//! Certificate common name will be "localhost".
//!
//! After 20 seconds, certificate common name will be "reloaded".

use axum::{routing::get, Router};
use axum_server::tls_rustls::RustlsConfig;
use std::{net::SocketAddr, time::Duration};
use tokio::time::sleep;

#[tokio::main]
async fn main() {
    let app = Router::new().route("/", get(|| async { "Hello, world!" }));

    let config = RustlsConfig::from_pem_file(
        "examples/self-signed-certs/cert.pem",
        "examples/self-signed-certs/key.pem",
    )
    .await
    .unwrap();

    // Spawn a task to reload tls.
    tokio::spawn(reload(config.clone()));

    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
    println!("listening on {}", addr);
    axum_server::bind_rustls(addr, config)
        .serve(app.into_make_service())
        .await
        .unwrap();
}

async fn reload(config: RustlsConfig) {
    // Wait for 20 seconds.
    sleep(Duration::from_secs(20)).await;

    println!("reloading rustls configuration");

    // Reload rustls configuration from new files.
    config
        .reload_from_pem_file(
            "examples/self-signed-certs/reload/cert.pem",
            "examples/self-signed-certs/reload/key.pem",
        )
        .await
        .unwrap();

    println!("rustls configuration reloaded");
}
