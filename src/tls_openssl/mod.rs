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
use openssl::ssl::Error as OpenSSLError;
use openssl::ssl::{SslAcceptor, SslAcceptorBuilder, SslFiletype, SslMethod};
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
    acceptor: Arc<SslAcceptor>,
}

impl OpenSSLConfig {
    /// This helper will established a TLS server based on strong cipher suites
    /// from a PEM formatted certificate and key.
    pub fn from_pem_file<A: AsRef<Path>, B: AsRef<Path>>(
        cert: A,
        key: B,
    ) -> Result<Self, OpenSSLError> {
        let mut tls_builder = SslAcceptor::mozilla_modern_v5(SslMethod::tls())?;

        tls_builder.set_certificate_file(cert, SslFiletype::PEM)?;

        tls_builder.set_private_key_file(key, SslFiletype::PEM)?;

        tls_builder.check_private_key()?;

        let acceptor = Arc::new(tls_builder.build());

        Ok(OpenSSLConfig { acceptor })
    }

    /// This helper will established a TLS server based on strong cipher suites
    /// from a PEM formatted certificate chain and key.
    pub fn from_pem_chain_file<A: AsRef<Path>, B: AsRef<Path>>(
        chain: A,
        key: B,
    ) -> Result<Self, OpenSSLError> {
        let mut tls_builder = SslAcceptor::mozilla_modern_v5(SslMethod::tls())?;

        tls_builder.set_certificate_chain_file(chain)?;

        tls_builder.set_private_key_file(key, SslFiletype::PEM)?;

        tls_builder.check_private_key()?;

        let acceptor = Arc::new(tls_builder.build());

        Ok(OpenSSLConfig { acceptor })
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
    fn try_from(tls_builder: SslAcceptorBuilder) -> Result<Self, Self::Error> {
        // Any other checks?
        tls_builder.check_private_key()?;

        let acceptor = Arc::new(tls_builder.build());

        Ok(OpenSSLConfig { acceptor })
    }
}

impl fmt::Debug for OpenSSLConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenSSLConfig").finish()
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        handle::Handle,
        tls_openssl::{self, OpenSSLConfig},
    };
    use axum::{routing::get, Router};
    use bytes::Bytes;
    use http::{response, Request};
    use hyper::{
        client::conn::{handshake, SendRequest},
        Body,
    };
    use std::{io, net::SocketAddr, time::Duration};
    use tokio::{net::TcpStream, task::JoinHandle, time::timeout};
    use tower::{Service, ServiceExt};

    use openssl::ssl::{Ssl, SslConnector, SslMethod, SslVerifyMode};
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
    async fn test_shutdown() {
        let (handle, _server_task, addr) = start_server().await;

        let (mut client, conn) = connect(addr).await;

        handle.shutdown();

        let response_future_result = client
            .ready()
            .await
            .unwrap()
            .call(Request::new(Body::empty()))
            .await;

        assert!(response_future_result.is_err());

        // Connection task should finish soon.
        let _ = timeout(Duration::from_secs(1), conn).await.unwrap();
    }

    #[tokio::test]
    async fn test_graceful_shutdown() {
        let (handle, server_task, addr) = start_server().await;

        let (mut client, conn) = connect(addr).await;

        handle.graceful_shutdown(None);

        let (_parts, body) = send_empty_request(&mut client).await;

        assert_eq!(body.as_ref(), b"Hello, world!");

        // Disconnect client.
        conn.abort();

        // Server task should finish soon.
        let server_result = timeout(Duration::from_secs(1), server_task)
            .await
            .unwrap()
            .unwrap();

        assert!(server_result.is_ok());
    }

    #[ignore]
    #[tokio::test]
    async fn test_graceful_shutdown_timed() {
        let (handle, server_task, addr) = start_server().await;

        let (mut client, _conn) = connect(addr).await;

        handle.graceful_shutdown(Some(Duration::from_millis(250)));

        let (_parts, body) = send_empty_request(&mut client).await;

        assert_eq!(body.as_ref(), b"Hello, world!");

        // Don't disconnect client.
        // conn.abort();

        // Server task should finish soon.
        let server_result = timeout(Duration::from_secs(1), server_task)
            .await
            .unwrap()
            .unwrap();

        assert!(server_result.is_ok());
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

    async fn connect(addr: SocketAddr) -> (SendRequest<Body>, JoinHandle<()>) {
        let stream = TcpStream::connect(addr).await.unwrap();
        let tls_stream = tls_connector(dns_name(), stream).await;

        let (send_request, connection) = handshake(tls_stream).await.unwrap();

        let task = tokio::spawn(async move {
            let _ = connection.await;
        });

        (send_request, task)
    }

    async fn send_empty_request(client: &mut SendRequest<Body>) -> (response::Parts, Bytes) {
        let (parts, body) = client
            .ready()
            .await
            .unwrap()
            .call(Request::new(Body::empty()))
            .await
            .unwrap()
            .into_parts();
        let body = hyper::body::to_bytes(body).await.unwrap();

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
