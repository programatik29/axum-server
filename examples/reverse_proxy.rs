//! Run with `cargo run --all-features --example reverse_proxy` command.
//!
//! Terminates TLS and forwards every request to an HTTP upstream. Set the
//! `UPSTREAM` environment variable to change the target, for example:
//!
//! ```text
//! UPSTREAM=http://127.0.0.1:8080 cargo run --all-features --example reverse_proxy
//! ```
//!
//! To connect through a browser, navigate to "https://localhost:3443".
//!
//! This is intentionally minimal: a production proxy should also handle
//! hop-by-hop headers, timeouts, request size limits, and upstream TLS.

use axum::{
    body::Body,
    extract::{ConnectInfo, State},
    http::{
        header::{HeaderValue, HOST},
        Request, StatusCode,
    },
    response::{IntoResponse, Response},
    Router,
};
use axum_server::tls_rustls::RustlsConfig;
use hyper_util::{
    client::legacy::{connect::HttpConnector, Client},
    rt::TokioExecutor,
};
use std::{net::SocketAddr, sync::Arc};

#[derive(Clone)]
struct AppState {
    upstream: String,
    client: Client<HttpConnector, Body>,
}

#[tokio::main]
async fn main() {
    let upstream = std::env::var("UPSTREAM")
        .unwrap_or_else(|_| "http://127.0.0.1:8080".to_owned())
        .trim_end_matches('/')
        .to_owned();

    // Validate the upstream URL early.
    upstream.parse::<http::Uri>().expect("invalid UPSTREAM");

    let state = Arc::new(AppState {
        upstream,
        client: Client::builder(TokioExecutor::new()).build_http(),
    });

    let app = Router::new().fallback(proxy).with_state(state);

    let config = RustlsConfig::from_pem_file(
        "examples/self-signed-certs/cert.pem",
        "examples/self-signed-certs/key.pem",
    )
    .await
    .unwrap();

    let addr = SocketAddr::from(([127, 0, 0, 1], 3443));
    println!("listening on https://{addr}");

    axum_server::bind_rustls(addr, config)
        .serve(app.into_make_service_with_connect_info::<SocketAddr>())
        .await
        .unwrap();
}

async fn proxy(
    State(state): State<Arc<AppState>>,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    mut req: Request<Body>,
) -> Response {
    let path_and_query = req
        .uri()
        .path_and_query()
        .map(|path| path.as_str())
        .unwrap_or("/");

    let host = req.headers().get(HOST).cloned();

    *req.uri_mut() = match format!("{}{}", state.upstream, path_and_query).parse() {
        Ok(uri) => uri,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid upstream request URI").into_response(),
    };

    // Preserve the original Host header and record the external request
    // information instead of trusting client-supplied headers.
    if let Some(host) = host {
        req.headers_mut().insert(HOST, host);
    }
    req.headers_mut()
        .insert("x-forwarded-proto", HeaderValue::from_static("https"));
    if let Ok(ip) = HeaderValue::from_str(&remote.ip().to_string()) {
        req.headers_mut().insert("x-forwarded-for", ip);
    }

    match state.client.request(req).await {
        Ok(response) => response.into_response(),
        Err(error) => (
            StatusCode::BAD_GATEWAY,
            format!("upstream request failed: {error}"),
        )
            .into_response(),
    }
}
