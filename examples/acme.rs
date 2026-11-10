//! Run with `ACME_DOMAINS=example.com cargo run --all-features --example acme`.
//!
//! Obtains and renews certificates automatically with Let's Encrypt.
//!
//! TLS-ALPN-01 requires the CA to reach this server on port 443, so the port
//! cannot be changed on the public side. The staging directory is used by
//! default to avoid production rate limits; set `ACME_PRODUCTION=1` for real
//! certificates.
//!
//! Environment:
//!
//! - `ACME_DOMAINS`: comma-separated domains, required.
//! - `ACME_CONTACT`: contact email, optional.
//! - `ACME_CACHE`: certificate cache directory, defaults to `./acme-cache`.
//! - `ACME_PORT`: port to bind, defaults to `443`.
//! - `ACME_PRODUCTION`: set to `1` to use the production directory.

use axum::{routing::get, Router};
use axum_server::acme::{caches::DirCache, Acceptor, AcmeConfig};
use futures_util::StreamExt;
use std::{env, net::SocketAddr};

#[tokio::main]
async fn main() {
    let domains = env::var("ACME_DOMAINS")
        .expect("set ACME_DOMAINS to a comma-separated list of domains")
        .split(',')
        .map(str::to_owned)
        .collect::<Vec<_>>();

    let cache = env::var("ACME_CACHE").unwrap_or_else(|_| "./acme-cache".to_owned());
    let port = env::var("ACME_PORT")
        .ok()
        .and_then(|port| port.parse().ok())
        .unwrap_or(443);
    let production = env::var("ACME_PRODUCTION").is_ok_and(|value| value == "1");

    let mut config = AcmeConfig::new(domains)
        .cache(DirCache::new(cache))
        .directory_lets_encrypt(production);

    if let Ok(contact) = env::var("ACME_CONTACT") {
        config = config.contact([format!("mailto:{contact}")]);
    }

    let mut state = config.state();
    let acceptor = Acceptor::new(&state);

    // Polling the state is what acquires and renews the certificates.
    tokio::spawn(async move {
        while let Some(event) = state.next().await {
            match event {
                Ok(event) => println!("acme: {event:?}"),
                Err(error) => eprintln!("acme error: {error:?}"),
            }
        }
    });

    let app = Router::new().route("/", get(|| async { "Hello, world!" }));
    let addr = SocketAddr::from(([0, 0, 0, 0], port));

    println!("listening on https://{addr}");
    if !production {
        println!("using the Let's Encrypt staging directory");
    }

    axum_server::bind(addr)
        .acceptor(acceptor)
        .serve(app.into_make_service())
        .await
        .unwrap();
}
