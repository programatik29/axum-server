//! axum-server is a [hyper] server implementation designed to be used with [axum] framework.
//!
//! # Features
//!
//! - HTTP/1 and HTTP/2
//! - HTTPS through [rustls] or [openssl].
//! - High performance through [hyper].
//! - Using [tower] make service API.
//! - Very good [axum] compatibility. Likely to work with future [axum] releases.
//!
//! # Guide
//!
//! axum-server can [`serve`] items that implement [`MakeService`] with some additional [trait
//! bounds](crate::service::MakeServiceRef). Make services that are [created] using [`axum`]
//! complies with those trait bounds out of the box. Therefore it is more convenient to use this
//! crate with [`axum`].
//!
//! All examples in this crate uses [`axum`]. If you want to use this crate without [`axum`] it is
//! highly recommended to learn how [tower] works.
//!
//! [`Server::bind`] or [`bind`] function can be called to create a server that will bind to
//! provided [`SocketAddr`] when [`serve`] is called.
//!
//! A [`Handle`] can be passed to [`Server`](Server::handle) for additional utilities like shutdown
//! and graceful shutdown.
//!
//! [`bind_rustls`] can be called by providing [`RustlsConfig`] to create a HTTPS [`Server`] that
//! will bind on provided [`SocketAddr`]. [`RustlsConfig`] can be cloned, reload methods can be
//! used on clone to reload tls configuration.
//!
//! # Features
//!
//! * `tls-rustls` - activate [rustls] support.
//! * `tls-rustls-no-provider` - activate [rustls] support without a default provider.
//! * `tls-openssl` - activate [openssl] support.
//!
//! # Example
//!
//! A simple hello world application can be served like:
//!
//! ```rust,no_run
//! use axum::{routing::get, Router};
//! use std::net::SocketAddr;
//!
//! #[tokio::main]
//! async fn main() {
//!     let app = Router::new().route("/", get(|| async { "Hello, world!" }));
//!
//!     let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
//!     println!("listening on {}", addr);
//!     axum_server::bind(addr)
//!         .serve(app.into_make_service())
//!         .await
//!         .unwrap();
//! }
//! ```
//!
//! You can find more examples in [repository].
//!
//! [axum]: https://crates.io/crates/axum
//! [bind]: crate::bind
//! [bind_rustls]: crate::bind_rustls
//! [created]: https://docs.rs/axum/0.3/axum/struct.Router.html#method.into_make_service
//! [hyper]: https://crates.io/crates/hyper
//! [openssl]: https://crates.io/crates/openssl
//! [repository]: https://github.com/programatik29/axum-server/tree/v0.3.0/examples
//! [rustls]: https://crates.io/crates/rustls
//! [tower]: https://crates.io/crates/tower
//! [`axum`]: https://docs.rs/axum/0.3
//! [`serve`]: crate::server::Server::serve
//! [`MakeService`]: https://docs.rs/tower/0.4/tower/make/trait.MakeService.html
//! [`RustlsConfig`]: crate::tls_rustls::RustlsConfig
//! [`SocketAddr`]: std::net::SocketAddr

#![forbid(unsafe_code)]
#![warn(
    clippy::await_holding_lock,
    clippy::cargo_common_metadata,
    clippy::dbg_macro,
    clippy::doc_markdown,
    clippy::empty_enum,
    clippy::enum_glob_use,
    clippy::inefficient_to_string,
    clippy::mem_forget,
    clippy::mutex_integer,
    clippy::needless_continue,
    clippy::todo,
    clippy::unimplemented,
    clippy::wildcard_imports,
    future_incompatible,
    missing_docs,
    missing_debug_implementations,
    unreachable_pub
)]
#![cfg_attr(docsrs, feature(doc_cfg))]

mod handle;
mod notify_once;
mod server;

pub mod accept;
pub mod service;

pub use self::{
    // addr_incoming_config::AddrIncomingConfig,
    handle::Handle,
    // http_config::HttpConfig,
    server::{bind, from_tcp, Server},
};

#[cfg(feature = "tls-rustls-no-provider")]
#[cfg_attr(docsrs, doc(cfg(feature = "tls-rustls")))]
pub mod tls_rustls;

#[doc(inline)]
#[cfg(feature = "tls-rustls-no-provider")]
pub use self::tls_rustls::export::{bind_rustls, from_tcp_rustls};

#[cfg(feature = "tls-openssl")]
#[cfg_attr(docsrs, doc(cfg(feature = "tls-openssl")))]
pub mod tls_openssl;

#[doc(inline)]
#[cfg(feature = "tls-openssl")]
pub use self::tls_openssl::bind_openssl;
