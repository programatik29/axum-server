use axum::{routing::get, Router};
use futures_util::future::try_join_all;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};

#[tokio::main]
async fn main() {
    let servers = vec![
        SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 3000),
        SocketAddr::new(Ipv6Addr::LOCALHOST.into(), 3000),
    ]
    .into_iter()
    .map(|addr| tokio::spawn(start_server(addr)));

    // Returns the first error if any of the servers return an error.
    try_join_all(servers).await.unwrap();
}

async fn start_server(addr: SocketAddr) {
    let app = Router::new().route("/", get(|| async { "Hello, world!" }));

    axum_server::bind(addr)
        .serve(app.into_make_service())
        .await
        .unwrap();
}
