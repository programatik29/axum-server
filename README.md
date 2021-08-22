# axum-server

`axum-server` is a [`hyper`] server implementation designed to be used with [`axum`] framework.

## Features

- Conveniently bind to any number of addresses.
- Tls support through [`rustls`]. Only `pem` format is supported for certificates.
- Access to client ip address from services/handlers.
- Record incoming and outgoing bytes for each connection.
- Services created by [`axum`] can directly be served.
- Although designed to be used with [`axum`], any `Service` that implements `Clone` can be served.

## Usage example

[`axum`] "Hello, World!" example can be run like:

```rust
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
```

## Safety

This crate uses `#![forbid(unsafe_code)]` to ensure everything is implemented in 100% safe Rust.

## License

This project is licensed under the [MIT license](LICENSE).

[`hyper`]: https://github.com/hyperium/hyper
[`axum`]: https://github.com/tokio-rs/axum
[`rustls`]: https://github.com/rustls/rustls
