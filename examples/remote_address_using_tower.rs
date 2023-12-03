//! Run with `cargo run --example remote_address_using_tower` command.
//!
//! To connect through browser, navigate to "http://localhost:3000" url.

use axum::body::Body;
use hyper::{body::Incoming, Request, Response};
use std::{convert::Infallible, net::SocketAddr};
use tower::service_fn;
use tower_http::add_extension::AddExtension;

#[tokio::main]
async fn main() {
    let service = service_fn(|mut req: Request<Incoming>| async move {
        let addr: SocketAddr = req.extensions_mut().remove().unwrap();
        let body = Body::from(format!("IP Address: {}", addr));

        Ok::<_, Infallible>(Response::new(body))
    });

    axum_server::bind(SocketAddr::from(([127, 0, 0, 1], 3000)))
        .serve(service_fn(|addr: SocketAddr| async move {
            Ok::<_, Infallible>(AddExtension::new(service, addr))
        }))
        .await
        .unwrap();
}
