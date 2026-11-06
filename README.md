[![License](https://img.shields.io/crates/l/axum-server)](https://choosealicense.com/licenses/mit/)
[![Crates.io](https://img.shields.io/crates/v/axum-server)](https://crates.io/crates/axum-server)
[![Docs](https://img.shields.io/crates/v/axum-server?color=blue&label=docs)](https://docs.rs/axum-server/)

# axum-server

axum-server is a [hyper] server implementation built for [axum] applications. It
serves a tower make service rather than anything axum-specific, so the name
undersells it a little: a proxy, or a service you already built from tower
layers, runs on it unchanged.

What it handles is everything around the app. A single port answers HTTP/1 and
HTTP/2, TLS is terminated through [rustls] or [openssl] and can be reloaded
while the server runs, and shutdown waits for requests that are still in
flight. Your code stays a plain `Router`.

This project is maintained by community independently from [axum].

## Features

- HTTP/1 and HTTP/2
- HTTPS through [rustls] or [openssl].
- TLS certificate hot-reloading.
- TCP and Unix socket listeners.
- High performance through [hyper].
- Using [tower] make service API.
- Very good [axum] compatibility. Likely to work with future [axum] releases.

## Getting started

Add the crate, with the TLS backend if you need one:

```toml
axum-server = "0.9"
# axum-server = { version = "0.9", features = ["tls-rustls"] }
```

Then hand it a router:

```rust
use axum::{routing::get, Router};
use std::net::SocketAddr;

#[tokio::main]
async fn main() {
    let app = Router::new().route("/", get(|| async { "Hello, world!" }));

    axum_server::bind(SocketAddr::from(([127, 0, 0, 1], 3000)))
        .serve(app.into_make_service())
        .await
        .unwrap();
}
```

## Examples

The rest of this page walks through the features in the order you tend to need
them, with the code that uses each one.

### TLS

Moving an app to HTTPS changes the bind call, not the app. With `tls-rustls`,
`bind_rustls` takes a `RustlsConfig` and negotiates TLS 1.2 and 1.3 using the
`aws-lc-rs` provider. The `tls-openssl` feature provides `bind_openssl` for
deployments that already depend on OpenSSL.

```rust
use axum_server::tls_rustls::RustlsConfig;

let config = RustlsConfig::from_pem_file("cert.pem", "key.pem").await?;

axum_server::bind_rustls(addr, config)
    .serve(app.into_make_service())
    .await?;
```

Neither TLS feature is on by default, so an HTTP-only build carries no TLS
dependency.

### Rotating certificates without a restart

`RustlsConfig` holds the live TLS settings rather than a snapshot of them, so
loading new files replaces what the next handshake sees. Copy the new
certificate into place and signal the process:

```rust
use axum_server::tls_rustls::RustlsConfig;
use tokio::signal::unix::{signal, SignalKind};

let config = RustlsConfig::from_pem_file("cert.pem", "key.pem").await?;

let mut hup = signal(SignalKind::hangup())?;
tokio::spawn({
    let config = config.clone();
    async move {
        while hup.recv().await.is_some() {
            config
                .reload_from_pem_file("cert.pem", "key.pem")
                .await
                .unwrap();
        }
    }
});

axum_server::bind_rustls(addr, config)
    .serve(app.into_make_service())
    .await?;
```

Connections that are already open finish on the certificate they started with,
so a rotation never interrupts them.

### Seeing the handshake in a handler

The TLS acceptors attach a `TlsInfo` to every request they handle, which is how
a handler tells which hostname the client asked for or which certificate it
presented. Name-based routing and mTLS authorization both read from it.

```rust
use axum::{extract::Extension, routing::get, Router};
use axum_server::tls::TlsInfo;
use std::sync::Arc;

let app = Router::new().route("/", get(whoami));

async fn whoami(Extension(info): Extension<Arc<TlsInfo>>) -> String {
    format!(
        "sni {:?}, alpn {:?}, client certs {}",
        info.server_name,
        info.alpn_protocol.as_deref().map(String::from_utf8_lossy),
        info.peer_certificates.len(),
    )
}
```

`alpn_protocol` says whether the connection is h2 or HTTP/1.1. Both are
advertised by default, so browsers and gRPC clients can share a port.

### How connections are accepted

Between the listener and hyper there is one hook, the `Accept` trait. It gets the
socket and the per-connection service, and returns them, possibly wrapped or
replaced. Everything connection-level in this crate is built on it, TLS
included.

```rust
pub trait Accept<I, S> {
    type Stream;
    type Service;
    type Future: Future<Output = io::Result<(Self::Stream, Self::Service)>>;

    fn accept(&self, stream: I, service: S) -> Self::Future;
}
```

Because TLS is just an acceptor, socket options can sit underneath it:

```rust
use axum_server::accept::{TcpConfigAcceptor, TcpKeepaliveConfig, TcpSocketConfig};
use axum_server::tls_rustls::{RustlsAcceptor, RustlsConfig};
use axum_server::Server;
use std::time::Duration;

let config = RustlsConfig::from_pem_file("cert.pem", "key.pem").await?;

let acceptor = RustlsAcceptor::new(config)
    .handshake_timeout(Duration::from_secs(5))
    .acceptor(TcpConfigAcceptor::new(
        TcpSocketConfig::new()
            .nodelay(true)
            .keepalive(TcpKeepaliveConfig::new().time(Duration::from_secs(60))),
    ));

Server::bind(addr)
    .acceptor(acceptor)
    .serve(app.into_make_service())
    .await?;
```

The inner acceptor runs first, so `TCP_NODELAY` and keepalive are set on the raw
socket before the handshake starts. The handshake timeout defaults to ten
seconds, and five is a choice.

### Writing your own acceptor

Implementing the trait gives you the socket before hyper has read a single HTTP
byte. That is enough to turn away clients which connect and then say nothing,
which scanners do all day and which otherwise costs a task and a file
descriptor each.

```rust
use axum_server::accept::Accept;
use std::{future::Future, io, pin::Pin, time::Duration};
use tokio::net::TcpStream;

/// Drops a connection if the client sends no bytes within the timeout.
#[derive(Clone)]
struct IdleClientGuard(Duration);

impl<S: Send + 'static> Accept<TcpStream, S> for IdleClientGuard {
    type Stream = TcpStream;
    type Service = S;
    type Future = Pin<Box<dyn Future<Output = io::Result<(TcpStream, S)>> + Send>>;

    fn accept(&self, stream: TcpStream, service: S) -> Self::Future {
        let timeout = self.0;
        Box::pin(async move {
            // Wait for the first byte without consuming it.
            let mut byte = [0u8; 1];
            tokio::time::timeout(timeout, stream.peek(&mut byte))
                .await
                .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "idle client"))??;
            Ok((stream, service))
        })
    }
}
```

Pass it to `Server::bind(addr).acceptor(...)` and it composes with the built-in
acceptors the same way they compose with each other. The stream type is an
associated type, which is what lets the TLS acceptor wrap a `TcpStream` without
the rest of the server knowing.

### Unix sockets

A service that only local processes talk to does not need a port. A Unix socket
keeps it off the network and leaves access to filesystem permissions, which
covers sidecars and anything sitting behind a local nginx.

```rust
use axum::{routing::get, Router};
use std::os::unix::net::SocketAddr;

let app = Router::new().route("/health", get(|| async { "ok" }));

let addr = SocketAddr::from_pathname("/run/myapp.sock")?;
axum_server::bind(addr)
    .serve(app.into_make_service())
    .await?;
```

The address is generic in this server, so shutdown, TLS and acceptors behave
exactly as they do on TCP. `from_unix_rustls` covers the case where the socket
itself needs TLS.

### Serving without axum

The name says axum, but `serve` takes the crate's `MakeService` bound, which any
tower make service satisfies as long as its futures and bodies are `Send`. A
reverse proxy or a service you already built from tower layers can use the same
TLS, listeners and shutdown code.

```rust
use bytes::Bytes;
use http_body_util::Full;
use hyper::{body::Incoming, Request, Response};
use std::{convert::Infallible, net::SocketAddr};
use tower::service_fn;
use tower_http::add_extension::AddExtension;

let service = service_fn(|mut req: Request<Incoming>| async move {
    let addr: SocketAddr = req.extensions_mut().remove().unwrap();
    Ok::<_, Infallible>(Response::new(Full::new(Bytes::from(format!("{addr}")))))
});

axum_server::bind(addr)
    .serve(service_fn(|addr: SocketAddr| async move {
        Ok::<_, Infallible>(AddExtension::new(service, addr))
    }))
    .await?;
```

A plain `service_fn` answers every request here, and the client address arrives
as an ordinary request extension. There are no axum types involved. axum's
`into_make_service()` is simply the most common way to satisfy the bound.

### Shutdown without dropping requests

On Kubernetes a pod gets `SIGTERM`, then a grace period, then the kill. If the
process exits on the first signal, requests in flight are reset and clients see
errors. `Handle` separates stopping the listener from ending the connections
that are still running.

```rust
use axum_server::Handle;
use std::time::Duration;

let handle = Handle::new();

let watcher = handle.clone();
tokio::spawn(async move {
    tokio::signal::ctrl_c().await.unwrap();
    watcher.graceful_shutdown(Some(Duration::from_secs(30)));

    while watcher.connection_count() > 0 {
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    println!("drained");
});

axum_server::bind(addr)
    .handle(handle)
    .serve(app.into_make_service())
    .await?;
```

`graceful_shutdown(None)` waits without a deadline, and `shutdown()` skips the
waiting. The same handle reports the live number through `connection_count()` and
the bound address through `listening()`, which is how a server on port `0` finds
its port.

More examples live in [examples](/examples).

## Cargo features

- `tls-rustls` (default off) - TLS support through [rustls] with the
  `aws-lc-rs` cryptography provider. Enables TLS 1.2 and TLS 1.3.
- `tls-rustls-no-provider` - TLS support through [rustls] without selecting a
  cryptography provider. Use this if you want to use `ring` or install a
  process-level provider yourself. For example:
  ```toml
  axum-server = { version = "0.9", features = ["tls-rustls-no-provider"] }
  rustls = { version = "0.23", default-features = false, features = ["ring", "std", "tls12"] }
  ```
- `tls-rustls-encrypted-keys` - support loading encrypted PKCS#8 private keys
  with `RustlsConfig`.
- `tls-openssl` - TLS support through [openssl].

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
