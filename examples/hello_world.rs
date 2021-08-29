// Run with "cargo run --example hello_world"

use axum::{
    handler::get,
    Router,
};

#[tokio::main]
async fn main() {
    let app = Router::new().route("/", get(|| async { "Hello, World!" }));

    axum_server::bind("127.0.0.1:3000")
        .serve(app)
        .await
        .unwrap();
}
