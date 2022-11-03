# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog], and this project adheres to
[Semantic Versioning].

# Unreleased

None.

# 0.4.3 (3. November 2022)

- **added:** Added `tcp_keepalive_interval` and `tcp_keepalive_retries` to
  `AddrIncomingConfig`.

# 0.4.2 (5. August 2022)

- **added:** Added `Server::from_tcp`, `axum_server::from_tcp` and
  `axum_server::from_tcp_rustls` methods to create `Server` from
  `std::net::TcpListener`.

# 0.4.1 (29. July 2022)

- **added:** Added `map`, `get` and `get_mut` methods to access the acceptor
  of `Server`.

# 0.4.0 (18. April 2022)

- Added TLS handshake timeout(10 seconds).
- In `RustlsConfig`: `from_pem` and `from_pem_file` methods now accept EC
  keys.
- **added:** Added `AddrIncomingConfig` to allow configuration of
  `hyper::server::conn::AddrIncoming`.
- **added:** Added `HttpConfig::http1_header_read_timeout`.
- **breaking:** Changed `Handle::listening` return type to
  `Option<SocketAddr>`. If binding fails, `Option::None` will be returned.

# 0.3.2 (17. November 2021)

- **added:** Added `HttpConfig` to allow more configuration.

# 0.3.1 (10. November 2021)

- **fixed:** `tls-rustls` feature doesn't compile if `fs` feature in `tokio`
  is not enabled.

# 0.3.0 (10. November 2021)

- **Total rewrite of source code.**
- **Major api changes:**
  - **breaking:** Removed `bind_rustls`, `certificate`, `certificate_file`,
    `loader`, `new`, `private_key`, `private_key_file`, `serve_and_record`,
    `tls_config` methods from `Server`.
  - **breaking:** Removed `tls` module.
  - **breaking:** Removed `record` module and feature.
  - **breaking:** Removed `Handle::listening_addrs` method.
  - **breaking:** `Server::bind` method doesn't take `self` anymore and
    creates an `Server`.
  - **breaking:** `bind` method now takes a `SocketAddr`.
  - **breaking:** `bind_rustls` method now takes a `SocketAddr` and an
    `tls_rustls::RustlsConfig`.
  - **breaking:** `Server::serve` method now takes a `MakeService`.
  - **breaking:** `Handle::listening` method now returns `SocketAddr`.
  - **added:** Added `Handle::connection_count` that can be used to get alive
    connection count.
  - **added:** Added `service` module.
  - **added:** Added `service::MakeServiceRef` and `service::SendService`
    traits aliases for convenience.
  - **added:** Added `accept` module.
  - **added:** Added `accept::Accept` trait that can be implemented to modify
    io stream and service.
  - **added:** Added `accept::DefaultAcceptor` struct that implements
    `accept::Accept` to be used as a default 'Accept' for 'Server'.
  - **added:** Added `Server::acceptor` method that can be used to provide a
    custom `accept::Accept`.
  - **added:** Added `tls_rustls` module.
  - **added:** Added `tls_rustls::RustlsAcceptor` that can be used with
    `Server::acceptor` to make a tls `Server`.
  - **added:** Added `tls_rustls::RustlsConfig` to create rustls utilities and
    to provide reload functionality.
  - **added:** Added `tls_rustls::bind_rustls` which is same as `bind_rustls`
    function.

# 0.2.5 (5. October 2021)

- Compile on rust `1.51`.

# 0.2.4 (17. September 2021)

- Reduced `futures-util` features to improve compile times.

# 0.2.3 (14. September 2021)

- Fixed `bind` and `bind_rustls` not working on some types.

# 0.2.2 (6. September 2021)

- Added uri `Scheme` in `Request` extensions.
- Fixed memory leak that happens as connections are accepted.

# 0.2.1 (30. August 2021)

- Fixed `serve_and_record` not recording independently for each connection.

# 0.2.0 (29. August 2021)

- Added `TlsLoader` to reload tls configuration.
- Added `Handle` to provide additional utilities for server.

# 0.1.2 (24. August 2021)

- Fixed an import issue when using `tls-rustls` feature.

# 0.1.0 (23. August 2021)

- Initial release.

[Keep a Changelog]: https://keepachangelog.com/en/1.0.0/
[Semantic Versioning]: https://semver.org/spec/v2.0.0.html
