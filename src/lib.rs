//! axum-server is a [`hyper`] server implementation designed to be used with [`axum`] framework.
//!
//! # Features
//!
//! - Conveniently bind to any number of addresses.
//! - Tls support through [`rustls`]. Only `pem` format is supported for certificates.
//! - Access to client ip address from services/handlers.
//! - Record incoming and outgoing bytes for each connection.
//! - Services created by [`axum`] can directly be served.
//! - Although designed to be used with [`axum`], any `Service` that implements `Clone` can be served.
//!
//! # Guide
//!
//! [`Server`](Server) can be created using [`Server::new`](Server::new) or [`bind`](bind), pick
//! whichever you are comfortable with.
//!
//! To serve an app, server must at least bind to an `address:port`. Anything that implements
//! [`ToSocketAddrs`] can be used with [`bind`](Server::bind) function for example `"127.0.0.1:3000"`.
//! After binding at least to one address, a [`Service`](tower_service::Service) that implements
//! [`Clone`](Clone) can be provided to [`Server::serve`](Server::serve) method which can then be `.await`ed
//! to run the server. This means all [`axum`] services can be served.
//!
//! In [`Request`](http::request::Request::extensions) extensions, [`SocketAddr`](std::net::SocketAddr) type
//! can be used to get client ip address.
//!
//! When `tls-rustls` feature is enabled, [`Server`](Server) can be turned into a
//! [`TlsServer`](tls::TlsServer) by calling methods about tls. Addresses defined using
//! [`bind`](tls::TlsServer::bind) will be served with HTTP protocol and addresses defined using
//! [`bind_rustls`](tls::TlsServer::bind_rustls) with HTTPS protocol.
//!
//! When `record` feature is enabled, [`serve_and_record`](Server::serve_and_record) method can be
//! used to record incoming and outgoing bytes. See [module](record) page for getting recorded bytes.
//!
//! # Example
//!
//! [`axum`] "Hello, World!" example can be run like:
//!
//! ```rust,no_run
//! use axum::{
//!     handler::get,
//!     Router,
//! };
//!
//! #[tokio::main]
//! async fn main() {
//!     let app = Router::new().route("/", get(|| async { "Hello, World!" }));
//!
//!     axum_server::bind("127.0.0.1:3000")
//!         .serve(app)
//!         .await
//!         .unwrap();
//! }
//! ```
//!
//! [`axum`]: https://crates.io/crates/axum
//! [`hyper`]: hyper
//! [`rustls`]: tokio_rustls::rustls
//! [`ToSocketAddrs`]: https://doc.rust-lang.org/stable/std/net/trait.ToSocketAddrs.html#implementors

#![forbid(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_cfg))]

mod server;

pub use server::{bind, Server};

#[cfg(feature = "tls-rustls")]
pub use server::{bind_rustls, tls};

#[cfg(feature = "record")]
pub use server::record;
