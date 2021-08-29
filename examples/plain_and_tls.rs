// Run with "cargo run --all-features --example plain_and_tls"

use axum::{handler::get, Router};
use axum_server::Server;

#[tokio::main]
async fn main() {
    let app = Router::new().route("/", get(|| async { "Hello, world" }));

    Server::new()
        .bind("127.0.0.1:3000")
        .bind_rustls("127.0.0.1:3443")
        .private_key_file("examples/self-signed-certs/key.pem")
        .certificate_file("examples/self-signed-certs/cert.pem")
        .serve(app)
        .await
        .unwrap();
}
