// Run with "cargo run --all-features --example uri_scheme"

use axum::{extract::Extension, handler::get, http::uri::Scheme, Router};
use axum_server::Server;

#[tokio::main]
async fn main() {
    let app = Router::new().route("/", get(handler));

    Server::new()
        .bind("127.0.0.1:3000")
        .bind_rustls("127.0.0.1:3443")
        .private_key_file("examples/self-signed-certs/key.pem")
        .certificate_file("examples/self-signed-certs/cert.pem")
        .serve(app)
        .await
        .unwrap();
}

async fn handler(Extension(scheme): Extension<Scheme>) -> String {
    format!("scheme: {}", scheme)
}
