[![License](https://img.shields.io/crates/l/axum-server)](https://choosealicense.com/licenses/mit/)
[![Crates.io](https://img.shields.io/crates/v/axum-server)](https://crates.io/crates/axum-server)
[![Docs](https://img.shields.io/crates/v/axum-server?color=blue&label=docs)](https://docs.rs/axum-server/)

# axum-server

axum-server is a [hyper] server implementation designed to be used with [axum] framework.

This project is maintained by community independently from [axum].

## Features

- HTTP/1 and HTTP/2
- HTTPS through [rustls] or [openssl].
- TLS certificate hot-reloading.
- TCP and Unix socket listeners.
- High performance through [hyper].
- Using [tower] make service API.
- Very good [axum] compatibility. Likely to work with future [axum] releases.

## Cargo features

- `tls-rustls` (default off) - TLS support through [rustls] with the
  `aws-lc-rs` cryptography provider. Enables TLS 1.2 and TLS 1.3.
- `tls-rustls-no-provider` - TLS support through [rustls] without selecting a
  cryptography provider. Use this if you want to use `ring` or install a
  process-level provider yourself. For example:
  ```toml
  axum-server = { version = "0.8", features = ["tls-rustls-no-provider"] }
  rustls = { version = "0.23", default-features = false, features = ["ring", "std", "tls12"] }
  ```
- `tls-rustls-encrypted-keys` - support loading encrypted PKCS#8 private keys
  with `RustlsConfig`.
- `tls-openssl` - TLS support through [openssl].

## Usage Example

A simple hello world application can be served like:

```rust
use axum::{routing::get, Router};
use std::net::SocketAddr;

#[tokio::main]
async fn main() {
    let app = Router::new().route("/", get(|| async { "Hello, world!" }));

    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
    println!("listening on {}", addr);
    axum_server::bind(addr)
        .serve(app.into_make_service())
        .await
        .unwrap();
}
```

You can find more examples [here](/examples).

## Minimum Supported Rust Version

axum-server's MSRV is `1.82`.

## Safety

This crate uses `#![forbid(unsafe_code)]` to ensure everything is implemented in 100% safe Rust.

## License

This project is licensed under the [MIT license](LICENSE).

[axum]: https://crates.io/crates/axum
[hyper]: https://crates.io/crates/hyper
[openssl]: https://crates.io/crates/openssl
[rustls]: https://crates.io/crates/rustls
[tower]: https://crates.io/crates/tower
