//! Tls implementation using [`openssl`]
//!
//! # Example
//!
//! ```rust,no_run
//! use axum::{routing::get, Router};
//! use axum_server::tls_openssl::OpenSSLConfig;
//! use std::net::SocketAddr;
//!
//! #[tokio::main]
//! async fn main() {
//!     let app = Router::new().route("/", get(|| async { "Hello, world!" }));
//!
//!     let config = OpenSSLConfig::from_pem_file(
//!         "examples/self-signed-certs/cert.pem",
//!         "examples/self-signed-certs/key.pem",
//!     )
//!     .unwrap();
//!
//!     let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
//!     println!("listening on {}", addr);
//!     axum_server::bind_openssl(addr, config)
//!         .serve(app.into_make_service())
//!         .await
//!         .unwrap();
//! }
//! ```

use self::future::OpenSSLAcceptorFuture;
use crate::{
    accept::{Accept, DefaultAcceptor},
    server::Server,
};
use arc_swap::ArcSwap;
use openssl::{
    pkey::PKey,
    ssl::{
        self, AlpnError, Error as OpenSSLError, SslAcceptor, SslAcceptorBuilder, SslFiletype,
        SslMethod, SslRef,
    },
    x509::X509,
};
use std::{convert::TryFrom, fmt, net::SocketAddr, path::Path, sync::Arc, time::Duration};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_openssl::SslStream;

pub mod future;

/// Create a TLS server that will be bound to the provided socket with a configuration. See
/// the [`crate::tls_openssl`] module for more details.
pub fn bind_openssl(addr: SocketAddr, config: OpenSSLConfig) -> Server<OpenSSLAcceptor> {
    let acceptor = OpenSSLAcceptor::new(config);

    Server::bind(addr).acceptor(acceptor)
}

/// Tls acceptor that uses OpenSSL. For details on how to use this see [`crate::tls_openssl`] module
/// for more details.
#[derive(Clone)]
pub struct OpenSSLAcceptor<A = DefaultAcceptor> {
    inner: A,
    config: OpenSSLConfig,
    handshake_timeout: Duration,
}

impl OpenSSLAcceptor {
    /// Create a new OpenSSL acceptor based on the provided [`OpenSSLConfig`]. This is
    /// generally used with manual calls to [`Server::bind`]. You may want [`bind_openssl`]
    /// instead.
    pub fn new(config: OpenSSLConfig) -> Self {
        let inner = DefaultAcceptor::new();

        #[cfg(not(test))]
        let handshake_timeout = Duration::from_secs(10);

        // Don't force tests to wait too long.
        #[cfg(test)]
        let handshake_timeout = Duration::from_secs(1);

        Self {
            inner,
            config,
            handshake_timeout,
        }
    }

    /// Override the default TLS handshake timeout of 10 seconds.
    pub fn handshake_timeout(mut self, val: Duration) -> Self {
        self.handshake_timeout = val;
        self
    }
}

impl<A> OpenSSLAcceptor<A> {
    /// Overwrite inner acceptor.
    pub fn acceptor<Acceptor>(self, acceptor: Acceptor) -> OpenSSLAcceptor<Acceptor> {
        OpenSSLAcceptor {
            inner: acceptor,
            config: self.config,
            handshake_timeout: self.handshake_timeout,
        }
    }
}

impl<A, I, S> Accept<I, S> for OpenSSLAcceptor<A>
where
    A: Accept<I, S>,
    A::Stream: AsyncRead + AsyncWrite + Unpin,
{
    type Stream = SslStream<A::Stream>;
    type Service = A::Service;
    type Future = OpenSSLAcceptorFuture<A::Future, A::Stream, A::Service>;

    fn accept(&self, stream: I, service: S) -> Self::Future {
        let inner_future = self.inner.accept(stream, service);
        let config = self.config.clone();

        OpenSSLAcceptorFuture::new(inner_future, config, self.handshake_timeout)
    }
}

impl<A> fmt::Debug for OpenSSLAcceptor<A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenSSLAcceptor").finish()
    }
}

/// OpenSSL configuration.
#[derive(Clone)]
pub struct OpenSSLConfig {
    acceptor: Arc<ArcSwap<SslAcceptor>>,
}

impl OpenSSLConfig {
    /// Create config from `Arc<`[`SslAcceptor`]`>`.
    pub fn from_acceptor(acceptor: Arc<SslAcceptor>) -> Self {
        let acceptor = Arc::new(ArcSwap::new(acceptor));

        OpenSSLConfig { acceptor }
    }

    /// This helper will establish a TLS server based on strong cipher suites
    /// from a DER-encoded certificate and key.
    pub fn from_der(cert: &[u8], key: &[u8]) -> Result<Self, OpenSSLError> {
        let acceptor = Arc::new(ArcSwap::from_pointee(config_from_der(cert, key)?));

        Ok(OpenSSLConfig { acceptor })
    }

    /// This helper will establish a TLS server based on strong cipher suites
    /// from a PEM-formatted certificate and key.
    pub fn from_pem(cert: &[u8], key: &[u8]) -> Result<Self, OpenSSLError> {
        let acceptor = Arc::new(ArcSwap::from_pointee(config_from_pem(cert, key)?));

        Ok(OpenSSLConfig { acceptor })
    }

    /// This helper will establish a TLS server based on strong cipher suites
    /// from a PEM-formatted certificate and key.
    pub fn from_pem_file(
        cert: impl AsRef<Path>,
        key: impl AsRef<Path>,
    ) -> Result<Self, OpenSSLError> {
        let acceptor = Arc::new(ArcSwap::from_pointee(config_from_pem_file(cert, key)?));

        Ok(OpenSSLConfig { acceptor })
    }

    /// This helper will establish a TLS server based on strong cipher suites
    /// from a PEM-formatted certificate chain and key.
    pub fn from_pem_chain_file(
        chain: impl AsRef<Path>,
        key: impl AsRef<Path>,
    ) -> Result<Self, OpenSSLError> {
        let acceptor = Arc::new(ArcSwap::from_pointee(config_from_pem_chain_file(
            chain, key,
        )?));

        Ok(OpenSSLConfig { acceptor })
    }

    /// Get inner `Arc<`[`SslAcceptor`]`>`.
    pub fn get_inner(&self) -> Arc<SslAcceptor> {
        self.acceptor.load_full()
    }

    /// Reload acceptor from `Arc<`[`SslAcceptor`]`>`.
    pub fn reload_from_acceptor(&self, acceptor: Arc<SslAcceptor>) {
        self.acceptor.store(acceptor);
    }

    /// Reload acceptor from a DER-encoded certificate and key.
    pub fn reload_from_der(&self, cert: &[u8], key: &[u8]) -> Result<(), OpenSSLError> {
        let acceptor = Arc::new(config_from_der(cert, key)?);
        self.acceptor.store(acceptor);

        Ok(())
    }

    /// Reload acceptor from a PEM-formatted certificate and key.
    pub fn reload_from_pem(&self, cert: &[u8], key: &[u8]) -> Result<(), OpenSSLError> {
        let acceptor = Arc::new(config_from_pem(cert, key)?);
        self.acceptor.store(acceptor);

        Ok(())
    }

    /// Reload acceptor from a PEM-formatted certificate and key.
    pub fn reload_from_pem_file(
        &self,
        cert: impl AsRef<Path>,
        key: impl AsRef<Path>,
    ) -> Result<(), OpenSSLError> {
        let acceptor = Arc::new(config_from_pem_file(cert, key)?);
        self.acceptor.store(acceptor);

        Ok(())
    }

    /// Reload acceptor from a PEM-formatted certificate chain and key.
    pub fn reload_from_pem_chain_file(
        &self,
        chain: impl AsRef<Path>,
        key: impl AsRef<Path>,
    ) -> Result<(), OpenSSLError> {
        let acceptor = Arc::new(config_from_pem_chain_file(chain, key)?);
        self.acceptor.store(acceptor);

        Ok(())
    }
}

impl TryFrom<SslAcceptorBuilder> for OpenSSLConfig {
    type Error = OpenSSLError;

    /// Build the [`OpenSSLConfig`] from an [`SslAcceptorBuilder`]. This allows precise
    /// control over the settings that will be used by OpenSSL in this server.
    ///
    /// # Example
    /// ```
    /// use axum_server::tls_openssl::OpenSSLConfig;
    /// use openssl::ssl::{SslAcceptor, SslMethod};
    /// use std::convert::TryFrom;
    ///
    /// #[tokio::main]
    /// async fn main() {
    ///     let mut tls_builder = SslAcceptor::mozilla_modern_v5(SslMethod::tls())
    ///         .unwrap();
    ///     // Set configurations like set_certificate_chain_file or
    ///     // set_private_key_file.
    ///     // let tls_builder.set_ ... ;

    ///     let _config = OpenSSLConfig::try_from(tls_builder);
    /// }
    /// ```
    fn try_from(mut tls_builder: SslAcceptorBuilder) -> Result<Self, Self::Error> {
        // Any other checks?
        tls_builder.check_private_key()?;
        tls_builder.set_alpn_select_callback(alpn_select);

        let acceptor = Arc::new(ArcSwap::from_pointee(tls_builder.build()));

        Ok(OpenSSLConfig { acceptor })
    }
}

impl fmt::Debug for OpenSSLConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenSSLConfig").finish()
    }
}

fn alpn_select<'a>(_tls: &mut SslRef, client: &'a [u8]) -> Result<&'a [u8], AlpnError> {
    ssl::select_next_proto(b"\x02h2\x08http/1.1", client).ok_or(AlpnError::NOACK)
}

fn config_from_der(cert: &[u8], key: &[u8]) -> Result<SslAcceptor, OpenSSLError> {
    let cert = X509::from_der(cert)?;
    let key = PKey::private_key_from_der(key)?;

    let mut tls_builder = SslAcceptor::mozilla_modern_v5(SslMethod::tls())?;
    tls_builder.set_certificate(&cert)?;
    tls_builder.set_private_key(&key)?;
    tls_builder.check_private_key()?;
    tls_builder.set_alpn_select_callback(alpn_select);

    let acceptor = tls_builder.build();
    Ok(acceptor)
}

fn config_from_pem(cert: &[u8], key: &[u8]) -> Result<SslAcceptor, OpenSSLError> {
    let cert = X509::from_pem(cert)?;
    let key = PKey::private_key_from_pem(key)?;

    let mut tls_builder = SslAcceptor::mozilla_modern_v5(SslMethod::tls())?;
    tls_builder.set_certificate(&cert)?;
    tls_builder.set_private_key(&key)?;
    tls_builder.check_private_key()?;
    tls_builder.set_alpn_select_callback(alpn_select);

    let acceptor = tls_builder.build();
    Ok(acceptor)
}

fn config_from_pem_file(
    cert: impl AsRef<Path>,
    key: impl AsRef<Path>,
) -> Result<SslAcceptor, OpenSSLError> {
    let mut tls_builder = SslAcceptor::mozilla_modern_v5(SslMethod::tls())?;
    tls_builder.set_certificate_file(cert, SslFiletype::PEM)?;
    tls_builder.set_private_key_file(key, SslFiletype::PEM)?;
    tls_builder.check_private_key()?;
    tls_builder.set_alpn_select_callback(alpn_select);

    let acceptor = tls_builder.build();
    Ok(acceptor)
}

fn config_from_pem_chain_file(
    chain: impl AsRef<Path>,
    key: impl AsRef<Path>,
) -> Result<SslAcceptor, OpenSSLError> {
    let mut tls_builder = SslAcceptor::mozilla_modern_v5(SslMethod::tls())?;
    tls_builder.set_certificate_chain_file(chain)?;
    tls_builder.set_private_key_file(key, SslFiletype::PEM)?;
    tls_builder.check_private_key()?;
    tls_builder.set_alpn_select_callback(alpn_select);

    let acceptor = tls_builder.build();
    Ok(acceptor)
}

#[cfg(test)]
mod tests {
    use crate::{
        handle::Handle,
        tls_openssl::{self, OpenSSLConfig},
    };
    use axum::body::Body;
    use axum::routing::get;
    use axum::Router;
    use bytes::Bytes;
    use http::{response, Request};
    use http_body_util::BodyExt;
    use hyper::client::conn::http1::{handshake, SendRequest};
    use hyper_util::rt::TokioIo;
    use std::{io, net::SocketAddr};
    use tokio::{net::TcpStream, task::JoinHandle};

    use openssl::{
        ssl::{Ssl, SslConnector, SslMethod, SslVerifyMode},
        x509::X509,
    };
    use std::pin::Pin;
    use tokio_openssl::SslStream;

    #[tokio::test]
    async fn start_and_request() {
        let (_handle, _server_task, addr) = start_server().await;

        let (mut client, _conn) = connect(addr).await;

        let (_parts, body) = send_empty_request(&mut client).await;

        assert_eq!(body.as_ref(), b"Hello, world!");
    }

    #[tokio::test]
    async fn test_reload() {
        let handle = Handle::new();

        let config = OpenSSLConfig::from_pem_file(
            "examples/self-signed-certs/cert.pem",
            "examples/self-signed-certs/key.pem",
        )
        .unwrap();

        let server_handle = handle.clone();
        let openssl_config = config.clone();
        tokio::spawn(async move {
            let app = Router::new().route("/", get(|| async { "Hello, world!" }));

            let addr = SocketAddr::from(([127, 0, 0, 1], 0));

            tls_openssl::bind_openssl(addr, openssl_config)
                .handle(server_handle)
                .serve(app.into_make_service())
                .await
        });

        let addr = handle.listening().await.unwrap();

        let cert_a = get_first_cert(addr).await;
        let mut cert_b = get_first_cert(addr).await;

        assert_eq!(cert_a, cert_b);

        config
            .reload_from_pem_file(
                "examples/self-signed-certs/reload/cert.pem",
                "examples/self-signed-certs/reload/key.pem",
            )
            .unwrap();

        cert_b = get_first_cert(addr).await;

        assert_ne!(cert_a, cert_b);

        config
            .reload_from_pem_file(
                "examples/self-signed-certs/cert.pem",
                "examples/self-signed-certs/key.pem",
            )
            .unwrap();

        cert_b = get_first_cert(addr).await;

        assert_eq!(cert_a, cert_b);
    }

    async fn start_server() -> (Handle, JoinHandle<io::Result<()>>, SocketAddr) {
        let handle = Handle::new();

        let server_handle = handle.clone();
        let server_task = tokio::spawn(async move {
            let app = Router::new().route("/", get(|| async { "Hello, world!" }));

            let config = OpenSSLConfig::from_pem_file(
                "examples/self-signed-certs/cert.pem",
                "examples/self-signed-certs/key.pem",
            )
            .unwrap();

            let addr = SocketAddr::from(([127, 0, 0, 1], 0));

            tls_openssl::bind_openssl(addr, config)
                .handle(server_handle)
                .serve(app.into_make_service())
                .await
        });

        let addr = handle.listening().await.unwrap();

        (handle, server_task, addr)
    }

    async fn get_first_cert(addr: SocketAddr) -> X509 {
        let stream = TcpStream::connect(addr).await.unwrap();
        let tls_stream = tls_connector(dns_name(), stream).await;

        tls_stream.ssl().peer_certificate().unwrap()
    }

    async fn connect(addr: SocketAddr) -> (SendRequest<Body>, JoinHandle<()>) {
        let stream = TcpStream::connect(addr).await.unwrap();
        let tls_stream = TokioIo::new(tls_connector(dns_name(), stream).await);

        let (send_request, connection) = handshake(tls_stream).await.unwrap();

        let task = tokio::spawn(async move {
            let _ = connection.await;
        });

        (send_request, task)
    }

    async fn send_empty_request(client: &mut SendRequest<Body>) -> (response::Parts, Bytes) {
        let (parts, body) = client
            .send_request(Request::new(Body::empty()))
            .await
            .unwrap()
            .into_parts();
        let body = body.collect().await.unwrap().to_bytes();

        (parts, body)
    }

    async fn tls_connector(hostname: &str, stream: TcpStream) -> SslStream<TcpStream> {
        let mut tls_parms = SslConnector::builder(SslMethod::tls_client()).unwrap();
        tls_parms.set_verify(SslVerifyMode::NONE);
        let hostname_owned = hostname.to_string();
        tls_parms.set_client_hello_callback(move |ssl_ref, _ssl_alert| {
            ssl_ref
                .set_hostname(hostname_owned.as_str())
                .map(|()| openssl::ssl::ClientHelloResponse::SUCCESS)
        });
        let tls_parms = tls_parms.build();

        let ssl = Ssl::new(tls_parms.context()).unwrap();
        let mut tls_stream = SslStream::new(ssl, stream).unwrap();

        SslStream::connect(Pin::new(&mut tls_stream)).await.unwrap();

        tls_stream
    }

    fn dns_name() -> &'static str {
        "localhost"
    }
}
