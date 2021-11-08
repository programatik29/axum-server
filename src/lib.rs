//! axum-server is a [hyper] server implementation designed to be used with [axum] framework.
//!
//! [hyper]: https://crates.io/crates/hyper
//! [axum]: https://crates.io/crates/axum

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

mod server;
mod service;

pub use self::server::{bind, Handle, Server};
