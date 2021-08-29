// Run with "cargo run --all-features --example tls_reload"

use axum::{handler::get, Router};
use axum_server::tls::TlsLoader;
use tokio::time::{sleep, Duration};

#[tokio::main]
async fn main() {
    let app = Router::new().route("/", get(|| async { "Hello, World!" }));

    let mut loader = TlsLoader::new();

    // Must be loaded before passing.
    // Those certificates has "localhost" as common name.
    loader
        .private_key_file("examples/self-signed-certs/key.pem")
        .certificate_file("examples/self-signed-certs/cert.pem")
        .load()
        .await
        .unwrap();

    tokio::spawn(reload_every_minute(loader.clone()));

    axum_server::bind_rustls("127.0.0.1:3000")
        .loader(loader)
        .serve(app)
        .await
        .unwrap();
}

async fn reload_every_minute(mut loader: TlsLoader) {
    loop {
        // Sleep first since certificates are loaded after loader is built.
        sleep(Duration::from_secs(60)).await;

        // Those certificates has "reloaded" as common name.
        loader
            .private_key_file("examples/self-signed-certs/reload/key.pem")
            .certificate_file("examples/self-signed-certs/reload/cert.pem")
            .load()
            .await
            .unwrap();
    }
}
