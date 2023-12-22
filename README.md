[![License](https://img.shields.io/crates/l/axum-server)](https://choosealicense.com/licenses/mit/)
[![Crates.io](https://img.shields.io/crates/v/axum-server)](https://crates.io/crates/axum-server)
[![Docs](https://img.shields.io/crates/v/axum-server?color=blue&label=docs)](https://docs.rs/axum-server/)

# axum-server

axum-server is a [hyper] server implementation designed to be used with [axum] framework.

This project is maintained by community independently from [axum].

## Features

- HTTP/1 and HTTP/2
- HTTPS through [rustls].
- High performance through [hyper].
- Using [tower] make service API.
- Very good [axum] compatibility. Likely to work with future [axum] releases.

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

axum-server's MSRV is `1.63`.

## Safety

This crate uses `#![forbid(unsafe_code)]` to ensure everything is implemented in 100% safe Rust.

## License

This project is licensed under the [MIT license](LICENSE).

[axum]: https://crates.io/crates/axum
[hyper]: https://crates.io/crates/hyper
[rustls]: https://crates.io/crates/rustls
[tower]: https://crates.io/crates/tower
