//! Automatic certificates through the ACME protocol.
//!
//! This module integrates [`rustls-acme`](https://crates.io/crates/rustls-acme)
//! with the server's acceptor hook. [`AcmeConfig`] describes the domains,
//! contact, cache and ACME directory, and [`Acceptor`] serves the certificates
//! that the resulting [`AcmeState`] acquires.
//!
//! Only TLS-ALPN-01 is used, so the server must listen on the public port 443.
//! The application must keep polling the [`AcmeState`] for certificates to be
//! acquired and renewed; polling needs a `StreamExt` implementation such as
//! `futures_util::StreamExt`.
//!
//! ```rust,no_run
//! use axum::{routing::get, Router};
//! use axum_server::acme::{caches::DirCache, AcmeConfig, Acceptor};
//! use futures_util::StreamExt;
//!
//! # async fn run() -> std::io::Result<()> {
//! let mut state = AcmeConfig::new(["example.com"])
//!     .contact(["mailto:admin@example.com"])
//!     .cache(DirCache::new("./acme-cache"))
//!     .directory_lets_encrypt(true)
//!     .state();
//!
//! let acceptor = Acceptor::new(&state);
//!
//! // Without this loop certificates are never acquired or renewed.
//! tokio::spawn(async move {
//!     while let Some(event) = state.next().await {
//!         if let Err(error) = event {
//!             eprintln!("acme: {error:?}");
//!         }
//!     }
//! });
//!
//! let app = Router::new().route("/", get(|| async { "Hello, world!" }));
//!
//! axum_server::bind(std::net::SocketAddr::from(([0, 0, 0, 0], 443)))
//!     .acceptor(acceptor)
//!     .serve(app.into_make_service())
//!     .await
//! # }
//! ```
//!
//! `ZeroSSL` requires external account binding, which `rustls-acme` does not
//! support. This module works with Let's Encrypt and other ACME CAs that do
//! not require it.

use crate::{
    accept::Accept,
    tls::TlsService,
    tls_rustls::{future::tls_info, set_alpn},
};
use rustls::ServerConfig;
use std::{fmt, future::Future, io, pin::Pin, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    time::{timeout_at, Instant},
};
use tokio_rustls::{server::TlsStream, LazyConfigAcceptor};

pub use rustls_acme::{caches, AcmeConfig, AcmeState};

/// An acceptor that serves the certificates acquired by an [`AcmeState`].
///
/// Create it with [`Acceptor::new`] and pass it to
/// [`Server::acceptor`](crate::Server::acceptor). The application must keep
/// polling the `AcmeState`; see the [module documentation](self).
#[derive(Clone)]
pub struct Acceptor {
    normal: Arc<ServerConfig>,
    challenge: Arc<ServerConfig>,
    handshake_timeout: Duration,
}

impl Acceptor {
    /// Create an acceptor for the certificates acquired by `state`.
    ///
    /// The normal configuration uses the `h2` and `http/1.1` ALPN protocols.
    /// TLS-ALPN-01 challenge connections are accepted and closed after the
    /// handshake.
    pub fn new<EC, EA>(state: &AcmeState<EC, EA>) -> Self
    where
        EC: fmt::Debug + 'static,
        EA: fmt::Debug + 'static,
    {
        let mut normal = (*state.default_rustls_config()).clone();
        set_alpn(&mut normal);

        Self {
            normal: Arc::new(normal),
            challenge: state.challenge_rustls_config(),
            handshake_timeout: Duration::from_secs(10),
        }
    }

    /// Override the default TLS handshake timeout of 10 seconds.
    pub fn handshake_timeout(mut self, val: Duration) -> Self {
        self.handshake_timeout = val;
        self
    }

    #[cfg(test)]
    fn from_configs(normal: Arc<ServerConfig>, challenge: Arc<ServerConfig>) -> Self {
        Self {
            normal,
            challenge,
            handshake_timeout: Duration::from_secs(10),
        }
    }
}

impl fmt::Debug for Acceptor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Acceptor")
            .field("handshake_timeout", &self.handshake_timeout)
            .finish_non_exhaustive()
    }
}

impl<I, S> Accept<I, S> for Acceptor
where
    I: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    S: Send + 'static,
{
    type Stream = TlsStream<I>;
    type Service = TlsService<S>;
    type Future = Pin<Box<dyn Future<Output = io::Result<(Self::Stream, Self::Service)>> + Send>>;

    fn accept(&self, stream: I, service: S) -> Self::Future {
        let normal = self.normal.clone();
        let challenge = self.challenge.clone();
        let handshake_timeout = self.handshake_timeout;

        Box::pin(async move {
            // One deadline covers reading the ClientHello and the handshake.
            let deadline = Instant::now() + handshake_timeout;

            let start = timeout_at(
                deadline,
                LazyConfigAcceptor::new(rustls::server::Acceptor::default(), stream),
            )
            .await
            .map_err(|_| timeout_error())??;

            let is_challenge = rustls_acme::is_tls_alpn_challenge(&start.client_hello());
            let config = if is_challenge { challenge } else { normal };

            let stream = timeout_at(deadline, start.into_stream(config))
                .await
                .map_err(|_| timeout_error())??;

            if is_challenge {
                return Err(io::Error::other("TLS-ALPN-01 validation request"));
            }

            let info = tls_info(&stream);

            Ok((stream, TlsService::new(service, info)))
        })
    }
}

fn timeout_error() -> io::Error {
    io::Error::new(io::ErrorKind::TimedOut, "tls handshake timed out")
}

#[cfg(test)]
mod tests {
    use super::{Acceptor, AcmeConfig};
    use crate::{accept::Accept, tls::TlsInfo, tls_rustls::RustlsConfig};
    use axum::{routing::get, Extension, Router};
    use http_body_util::BodyExt;
    use hyper_util::rt::TokioIo;
    use rustls::{
        client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
        pki_types::{CertificateDer, ServerName, UnixTime},
        ClientConfig, DigitallySignedStruct, SignatureScheme,
    };
    use std::{sync::Arc, time::Duration};
    use tokio::net::TcpStream;
    use tokio_rustls::TlsConnector;

    async fn acceptor_configs() -> (Arc<rustls::ServerConfig>, Arc<rustls::ServerConfig>) {
        let normal = RustlsConfig::from_pem_file(
            "examples/self-signed-certs/sni/localhost-cert.pem",
            "examples/self-signed-certs/sni/localhost-key.pem",
        )
        .await
        .unwrap()
        .get_inner();

        let mut challenge = (*normal).clone();
        challenge.alpn_protocols = vec![b"acme-tls/1".to_vec()];

        (normal, Arc::new(challenge))
    }

    fn tls_connector(alpn: Vec<Vec<u8>>) -> TlsConnector {
        #[derive(Debug)]
        struct NoVerify;

        impl ServerCertVerifier for NoVerify {
            fn verify_server_cert(
                &self,
                _end_entity: &CertificateDer,
                _intermediates: &[CertificateDer],
                _server_name: &ServerName,
                _ocsp_response: &[u8],
                _now: UnixTime,
            ) -> Result<ServerCertVerified, rustls::Error> {
                Ok(ServerCertVerified::assertion())
            }

            fn verify_tls12_signature(
                &self,
                _message: &[u8],
                _cert: &CertificateDer<'_>,
                _dss: &DigitallySignedStruct,
            ) -> Result<HandshakeSignatureValid, rustls::Error> {
                Ok(HandshakeSignatureValid::assertion())
            }

            fn verify_tls13_signature(
                &self,
                _message: &[u8],
                _cert: &CertificateDer<'_>,
                _dss: &DigitallySignedStruct,
            ) -> Result<HandshakeSignatureValid, rustls::Error> {
                Ok(HandshakeSignatureValid::assertion())
            }

            fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
                vec![
                    SignatureScheme::RSA_PKCS1_SHA1,
                    SignatureScheme::RSA_PKCS1_SHA256,
                    SignatureScheme::RSA_PKCS1_SHA384,
                    SignatureScheme::RSA_PKCS1_SHA512,
                    SignatureScheme::RSA_PSS_SHA256,
                    SignatureScheme::RSA_PSS_SHA384,
                    SignatureScheme::RSA_PSS_SHA512,
                ]
            }
        }

        let mut config = ClientConfig::builder_with_provider(crate::tls_rustls::crypto_provider())
            .with_safe_default_protocol_versions()
            .expect("the selected CryptoProvider supports the default protocol versions")
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoVerify))
            .with_no_client_auth();
        config.alpn_protocols = alpn;

        TlsConnector::from(Arc::new(config))
    }

    fn dns_name() -> ServerName<'static> {
        ServerName::try_from("localhost").unwrap()
    }

    #[tokio::test]
    async fn builds_expected_alpn_configs() {
        let state = AcmeConfig::new(["example.com"]).state();
        let acceptor = Acceptor::new(&state);

        assert_eq!(
            acceptor.normal.alpn_protocols,
            vec![b"h2".to_vec(), b"http/1.1".to_vec()]
        );
        assert_eq!(
            acceptor.challenge.alpn_protocols,
            vec![b"acme-tls/1".to_vec()]
        );
    }

    #[tokio::test]
    async fn serves_normal_connections() {
        let (normal, challenge) = acceptor_configs().await;
        let acceptor = Acceptor::from_configs(normal, challenge);

        let (client_io, server_io) = tokio::io::duplex(4096);
        let server = acceptor.accept(server_io, ());
        let client = tls_connector(vec![b"h2".to_vec()]).connect(dns_name(), client_io);

        let (server_result, client_result) = tokio::join!(server, client);
        let (server_stream, _service) = server_result.unwrap();
        let client_stream = client_result.unwrap();

        assert_eq!(server_stream.get_ref().1.alpn_protocol(), Some(&b"h2"[..]));
        assert_eq!(client_stream.get_ref().1.alpn_protocol(), Some(&b"h2"[..]));
    }

    #[tokio::test]
    async fn closes_challenge_connections_after_handshake() {
        let (normal, challenge) = acceptor_configs().await;
        let acceptor = Acceptor::from_configs(normal, challenge);

        let (client_io, server_io) = tokio::io::duplex(4096);
        let server = acceptor.accept(server_io, ());
        let client = tls_connector(vec![b"acme-tls/1".to_vec()]).connect(dns_name(), client_io);

        let (server_result, client_result) = tokio::join!(server, client);

        client_result.expect("the TLS-ALPN-01 handshake should complete");
        let error = server_result.unwrap_err();
        assert_eq!(error.to_string(), "TLS-ALPN-01 validation request");
    }

    #[tokio::test]
    async fn times_out_without_client_hello() {
        let (normal, challenge) = acceptor_configs().await;
        let acceptor =
            Acceptor::from_configs(normal, challenge).handshake_timeout(Duration::from_millis(50));

        let (_client_io, server_io) = tokio::io::duplex(4096);
        let error = acceptor.accept(server_io, ()).await.unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
    }

    #[tokio::test]
    async fn tls_info_is_available_to_handlers() {
        let (normal, challenge) = acceptor_configs().await;
        let acceptor = Acceptor::from_configs(normal, challenge);

        let app = Router::new().route(
            "/",
            get(|Extension(info): Extension<Arc<TlsInfo>>| async move {
                info.server_name.clone().unwrap_or_default()
            }),
        );

        let handle = crate::Handle::new();
        let server_handle = handle.clone();
        let server = tokio::spawn(async move {
            crate::Server::bind(std::net::SocketAddr::from(([127, 0, 0, 1], 0)))
                .handle(server_handle)
                .acceptor(acceptor)
                .serve(app.into_make_service())
                .await
        });
        let addr = handle.listening().await.unwrap();

        let stream = TcpStream::connect(addr).await.unwrap();
        let stream = tls_connector(vec![b"http/1.1".to_vec()])
            .connect(dns_name(), stream)
            .await
            .unwrap();

        let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
            .await
            .unwrap();
        tokio::spawn(async move {
            let _ = connection.await;
        });

        let response = sender
            .send_request(http::Request::new(axum::body::Body::empty()))
            .await
            .unwrap();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(body.as_ref(), b"localhost");

        handle.shutdown();
        let _ = tokio::time::timeout(Duration::from_secs(1), server).await;
    }
}
