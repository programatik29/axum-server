//! Tls implementation using [`rustls`].
//!
//! # Example
//!
//! ```rust,no_run
//! use axum::{routing::get, Router};
//! use axum_server::tls_rustls::RustlsConfig;
//! use std::net::SocketAddr;
//!
//! #[tokio::main]
//! async fn main() {
//!     let app = Router::new().route("/", get(|| async { "Hello, world!" }));
//!
//!     let config = RustlsConfig::from_pem_file(
//!         "examples/self-signed-certs/cert.pem",
//!         "examples/self-signed-certs/key.pem",
//!     )
//!     .await
//!     .unwrap();
//!
//!     let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
//!     println!("listening on {}", addr);
//!     axum_server::bind_rustls(addr, config)
//!         .serve(app.into_make_service())
//!         .await
//!         .unwrap();
//! }
//! ```

use self::future::RustlsAcceptorFuture;
use crate::{
    accept::{Accept, DefaultAcceptor},
    server::{io_other, Server},
    tls::TlsService,
    Address,
};
use arc_swap::ArcSwap;
use rustls::server::ResolvesServerCertUsingSni;
use rustls::sign::CertifiedKey;
use rustls::ServerConfig;
use rustls_pki_types::pem::PemObject;
use rustls_pki_types::{CertificateDer, PrivateKeyDer};

#[cfg(feature = "tls-rustls-encrypted-keys")]
use pkcs8::der::{Decode, Document};
#[cfg(feature = "tls-rustls-encrypted-keys")]
use rustls_pki_types::PrivatePkcs8KeyDer;
use std::time::Duration;
use std::{fmt, io, net::SocketAddr, path::Path, sync::Arc};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    task::spawn_blocking,
};
use tokio_rustls::server::TlsStream;

pub mod future;

/// Create a tls server that will bind to provided address.
#[cfg_attr(docsrs, doc(cfg(feature = "tls-rustls")))]
pub fn bind_rustls<A: Address>(addr: A, config: RustlsConfig) -> Server<A, RustlsAcceptor> {
    let acceptor = RustlsAcceptor::new(config);

    Server::bind(addr).acceptor(acceptor)
}

/// Create a tls server from existing `std::net::TcpListener`.
#[cfg_attr(docsrs, doc(cfg(feature = "tls-rustls")))]
pub fn from_tcp_rustls(
    listener: std::net::TcpListener,
    config: RustlsConfig,
) -> io::Result<Server<SocketAddr, RustlsAcceptor>> {
    let acceptor = RustlsAcceptor::new(config);

    Ok(crate::from_tcp(listener)?.acceptor(acceptor))
}

/// Create a tls server from existing `std::os::unix::net::UnixListener`.
#[cfg_attr(docsrs, doc(cfg(feature = "tls-rustls")))]
#[cfg(unix)]
pub fn from_unix_rustls(
    listener: std::os::unix::net::UnixListener,
    config: RustlsConfig,
) -> io::Result<Server<std::os::unix::net::SocketAddr, RustlsAcceptor>> {
    let acceptor = RustlsAcceptor::new(config);

    Ok(crate::from_unix(listener)?.acceptor(acceptor))
}

/// Tls acceptor using rustls.
#[derive(Clone)]
pub struct RustlsAcceptor<A = DefaultAcceptor> {
    inner: A,
    config: RustlsConfig,
    handshake_timeout: Duration,
}

impl RustlsAcceptor {
    /// Create a new rustls acceptor.
    pub fn new(config: RustlsConfig) -> Self {
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

    /// Override the default TLS handshake timeout of 10 seconds, except during testing.
    pub fn handshake_timeout(mut self, val: Duration) -> Self {
        self.handshake_timeout = val;
        self
    }
}

impl<A> RustlsAcceptor<A> {
    /// Overwrite inner acceptor.
    pub fn acceptor<Acceptor>(self, acceptor: Acceptor) -> RustlsAcceptor<Acceptor> {
        RustlsAcceptor {
            inner: acceptor,
            config: self.config,
            handshake_timeout: self.handshake_timeout,
        }
    }
}

impl<A, I, S> Accept<I, S> for RustlsAcceptor<A>
where
    A: Accept<I, S>,
    A::Stream: AsyncRead + AsyncWrite + Unpin,
{
    type Stream = TlsStream<A::Stream>;
    type Service = TlsService<A::Service>;
    type Future = RustlsAcceptorFuture<A::Future, A::Stream, A::Service>;

    fn accept(&self, stream: I, service: S) -> Self::Future {
        let inner_future = self.inner.accept(stream, service);
        let config = self.config.clone();

        RustlsAcceptorFuture::new(inner_future, config, self.handshake_timeout)
    }
}

impl<A> fmt::Debug for RustlsAcceptor<A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RustlsAcceptor").finish()
    }
}

/// Rustls configuration.
#[derive(Clone)]
pub struct RustlsConfig {
    inner: Arc<ArcSwap<ServerConfig>>,
}

impl RustlsConfig {
    /// Create config from `Arc<`[`ServerConfig`]`>`.
    ///
    /// NOTE: You need to set ALPN protocols (like `http/1.1` or `h2`) manually.
    pub fn from_config(config: Arc<ServerConfig>) -> Self {
        let inner = Arc::new(ArcSwap::new(config));

        Self { inner }
    }

    /// Create config from DER-encoded data.
    ///
    /// The certificate must be DER-encoded X.509.
    ///
    /// The private key must be DER-encoded ASN.1 in either PKCS#8 or PKCS#1 format.
    pub async fn from_der(cert: Vec<Vec<u8>>, key: Vec<u8>) -> io::Result<Self> {
        let server_config = spawn_blocking(|| config_from_der(cert, key))
            .await
            .unwrap()?;
        let inner = Arc::new(ArcSwap::from_pointee(server_config));

        Ok(Self { inner })
    }

    /// Create config from PEM formatted data.
    ///
    /// Certificate and private key must be in PEM format.
    pub async fn from_pem(cert: Vec<u8>, key: Vec<u8>) -> io::Result<Self> {
        let server_config = spawn_blocking(|| config_from_pem(cert, key))
            .await
            .unwrap()?;
        let inner = Arc::new(ArcSwap::from_pointee(server_config));

        Ok(Self { inner })
    }

    /// Create config from PEM formatted files.
    ///
    /// Contents of certificate file and private key file must be in PEM format.
    pub async fn from_pem_file(cert: impl AsRef<Path>, key: impl AsRef<Path>) -> io::Result<Self> {
        let server_config = config_from_pem_file(cert, key).await?;
        let inner = Arc::new(ArcSwap::from_pointee(server_config));

        Ok(Self { inner })
    }

    /// Create config from PEM formatted files, selecting the certificate by
    /// the server name sent by the client using SNI.
    ///
    /// Each item is `(server_name, certificate_path, private_key_path)`. The
    /// server name is matched against the client's SNI value, and each
    /// certificate must be valid for the name it is registered under.
    ///
    /// The certificate and private key files must be in PEM format.
    pub async fn from_sni_pem_files<I, S, P, K>(files: I) -> io::Result<Self>
    where
        I: IntoIterator<Item = (S, P, K)>,
        S: AsRef<str>,
        P: AsRef<Path>,
        K: AsRef<Path>,
    {
        let provider = crypto_provider();

        let mut resolver = ResolvesServerCertUsingSni::new();

        for (name, cert_path, key_path) in files {
            let cert = fs_err::tokio::read(cert_path.as_ref()).await?;
            let key = fs_err::tokio::read(key_path.as_ref()).await?;

            let cert = certificates_from_pem(&cert)?;
            let key = private_key_from_pem(&key)?;
            let signing_key = provider
                .key_provider
                .load_private_key(key)
                .map_err(io_other)?;

            resolver
                .add(name.as_ref(), CertifiedKey::new(cert, signing_key))
                .map_err(io_other)?;
        }

        let mut config = config_builder()
            .with_no_client_auth()
            .with_cert_resolver(Arc::new(resolver));

        set_alpn(&mut config);

        let inner = Arc::new(ArcSwap::from_pointee(config));

        Ok(Self { inner })
    }

    /// Get  inner `Arc<`[`ServerConfig`]`>`.
    pub fn get_inner(&self) -> Arc<ServerConfig> {
        self.inner.load_full()
    }

    /// Reload config from `Arc<`[`ServerConfig`]`>`.
    pub fn reload_from_config(&self, config: Arc<ServerConfig>) {
        self.inner.store(config);
    }

    /// Reload config from DER-encoded data.
    ///
    /// The certificate must be DER-encoded X.509.
    ///
    /// The private key must be DER-encoded ASN.1 in either PKCS#8 or PKCS#1 format.
    pub async fn reload_from_der(&self, cert: Vec<Vec<u8>>, key: Vec<u8>) -> io::Result<()> {
        let server_config = spawn_blocking(|| config_from_der(cert, key))
            .await
            .unwrap()?;
        let inner = Arc::new(server_config);

        self.inner.store(inner);

        Ok(())
    }

    /// This helper will establish a TLS server based on strong cipher suites
    /// from a PEM-formatted certificate chain and key.
    pub async fn from_pem_chain_file(
        chain: impl AsRef<Path>,
        key: impl AsRef<Path>,
    ) -> io::Result<Self> {
        let server_config = config_from_pem_chain_file(chain, key).await?;
        let inner = Arc::new(ArcSwap::from_pointee(server_config));

        Ok(Self { inner })
    }

    /// Reload config from PEM formatted data.
    ///
    /// Certificate and private key must be in PEM format.
    pub async fn reload_from_pem(&self, cert: Vec<u8>, key: Vec<u8>) -> io::Result<()> {
        let server_config = spawn_blocking(|| config_from_pem(cert, key))
            .await
            .unwrap()?;
        let inner = Arc::new(server_config);

        self.inner.store(inner);

        Ok(())
    }

    /// Reload config from PEM formatted files.
    ///
    /// Contents of certificate file and private key file must be in PEM format.
    pub async fn reload_from_pem_file(
        &self,
        cert: impl AsRef<Path>,
        key: impl AsRef<Path>,
    ) -> io::Result<()> {
        let server_config = config_from_pem_file(cert, key).await?;
        let inner = Arc::new(server_config);

        self.inner.store(inner);

        Ok(())
    }
}

#[cfg(feature = "tls-rustls-encrypted-keys")]
impl RustlsConfig {
    /// Create config from a PEM formatted certificate and an encrypted PKCS#8
    /// private key.
    ///
    /// The key must be a PKCS#8 `ENCRYPTED PRIVATE KEY` section, as produced by
    /// `openssl pkcs8 -topk8`.
    pub async fn from_pem_with_password(
        cert: Vec<u8>,
        key: Vec<u8>,
        password: impl AsRef<[u8]>,
    ) -> io::Result<Self> {
        let password = password.as_ref().to_vec();
        let server_config =
            spawn_blocking(move || config_from_pem_with_password(cert, key, password))
                .await
                .unwrap()?;
        let inner = Arc::new(ArcSwap::from_pointee(server_config));

        Ok(Self { inner })
    }

    /// Create config from PEM formatted files, where the private key is an
    /// encrypted PKCS#8 key.
    pub async fn from_pem_file_with_password(
        cert: impl AsRef<Path>,
        key: impl AsRef<Path>,
        password: impl AsRef<[u8]>,
    ) -> io::Result<Self> {
        let cert = fs_err::tokio::read(cert.as_ref()).await?;
        let key = fs_err::tokio::read(key.as_ref()).await?;

        Self::from_pem_with_password(cert, key, password).await
    }

    /// Reload config from a PEM formatted certificate and an encrypted PKCS#8
    /// private key.
    pub async fn reload_from_pem_with_password(
        &self,
        cert: Vec<u8>,
        key: Vec<u8>,
        password: impl AsRef<[u8]>,
    ) -> io::Result<()> {
        let password = password.as_ref().to_vec();
        let server_config =
            spawn_blocking(move || config_from_pem_with_password(cert, key, password))
                .await
                .unwrap()?;

        self.inner.store(Arc::new(server_config));

        Ok(())
    }

    /// Reload config from PEM formatted files, where the private key is an
    /// encrypted PKCS#8 key.
    pub async fn reload_from_pem_file_with_password(
        &self,
        cert: impl AsRef<Path>,
        key: impl AsRef<Path>,
        password: impl AsRef<[u8]>,
    ) -> io::Result<()> {
        let cert = fs_err::tokio::read(cert.as_ref()).await?;
        let key = fs_err::tokio::read(key.as_ref()).await?;

        self.reload_from_pem_with_password(cert, key, password)
            .await
    }
}

#[cfg(feature = "tls-rustls-encrypted-keys")]
fn config_from_pem_with_password(
    cert: Vec<u8>,
    key: Vec<u8>,
    password: Vec<u8>,
) -> io::Result<ServerConfig> {
    let cert = certificates_from_pem(&cert)?;
    let pem = std::str::from_utf8(&key)
        .map_err(|_| io_other("the private key file is not valid UTF-8"))?;
    let (label, document) = Document::from_pem(pem).map_err(|_| io_other("failed to parse PEM"))?;

    if label != "ENCRYPTED PRIVATE KEY" {
        return Err(io_other(
            "expected an encrypted PKCS#8 private key (ENCRYPTED PRIVATE KEY)",
        ));
    }

    let encrypted = pkcs8::EncryptedPrivateKeyInfo::from_der(document.as_bytes())
        .map_err(|_| io_other("failed to parse encrypted private key"))?;
    let decrypted = encrypted
        .decrypt(&password)
        .map_err(|_| io_other("failed to decrypt private key; is the password correct?"))?;
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(decrypted.as_bytes().to_vec()));

    config_from_cert_and_key(cert, key)
}

impl fmt::Debug for RustlsConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RustlsConfig").finish()
    }
}

const ALPN_PROTOCOLS: [&[u8]; 2] = [b"h2", b"http/1.1"];

pub(crate) fn set_alpn(config: &mut ServerConfig) {
    config.alpn_protocols = ALPN_PROTOCOLS.iter().map(|proto| proto.to_vec()).collect();
}

fn crypto_provider() -> &'static Arc<rustls::crypto::CryptoProvider> {
    if let Some(provider) = rustls::crypto::CryptoProvider::get_default() {
        return provider;
    }

    install_provider();

    if let Some(provider) = rustls::crypto::CryptoProvider::get_default() {
        return provider;
    }

    // No provider feature is enabled by this crate. Building a config installs
    // the provider selected by the application's rustls features, if any.
    let _ = ServerConfig::builder();

    rustls::crypto::CryptoProvider::get_default()
        .expect("a default CryptoProvider is installed by ServerConfig::builder")
}

// Prefer aws-lc-rs when both provider features are enabled, for example under
// `--all-features`, instead of letting rustls panic on an ambiguous choice.
#[cfg(feature = "tls-rustls")]
fn install_provider() {
    let _ = rustls::crypto::CryptoProvider::install_default(
        rustls::crypto::aws_lc_rs::default_provider(),
    );
}

#[cfg(all(not(feature = "tls-rustls"), feature = "tls-rustls-ring"))]
fn install_provider() {
    let _ =
        rustls::crypto::CryptoProvider::install_default(rustls::crypto::ring::default_provider());
}

#[cfg(not(any(feature = "tls-rustls", feature = "tls-rustls-ring")))]
fn install_provider() {}

fn config_builder() -> rustls::ConfigBuilder<ServerConfig, rustls::WantsVerifier> {
    ServerConfig::builder_with_provider(crypto_provider().clone())
        .with_safe_default_protocol_versions()
        .expect("the selected CryptoProvider supports the default protocol versions")
}

fn config_from_der(cert: Vec<Vec<u8>>, key: Vec<u8>) -> io::Result<ServerConfig> {
    let cert = cert.into_iter().map(CertificateDer::from).collect();
    let key = PrivateKeyDer::try_from(key).map_err(io_other)?;

    config_from_cert_and_key(cert, key)
}

fn config_from_cert_and_key(
    cert: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
) -> io::Result<ServerConfig> {
    let mut config = config_builder()
        .with_no_client_auth()
        .with_single_cert(cert, key)
        .map_err(io_other)?;

    set_alpn(&mut config);

    Ok(config)
}

fn certificates_from_pem(cert: &[u8]) -> io::Result<Vec<CertificateDer<'static>>> {
    CertificateDer::pem_slice_iter(cert)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| io_other("failed to parse certificate"))
}

fn private_key_from_pem(key: &[u8]) -> io::Result<PrivateKeyDer<'static>> {
    let mut key_result: Result<PrivateKeyDer<'static>, io::Error> =
        Err(io_other("The private key file contained no keys"));

    // Check the entire PEM file for the key in case it is not first section
    for item in rustls_pki_types::pem::PemObject::pem_slice_iter(key) {
        let key: Result<PrivateKeyDer<'static>, io::Error> =
            item.map_err(|_| io_other("failed to parse PEM"));

        match key_result {
            // if we already got a key, then...
            Ok(_) => {
                // ...if we get a key now, we know that there are multiple keys and that's not allowed
                if key.is_ok() {
                    return Err(io_other(
                        "The private key file contains multiple keys (it must only contain one)",
                    ));
                }
            }
            // but if already have an error, just overwrite it with whatever we got this time. If
            // it's a good key, that's cool. If it's an error, then we're just ignoring the old
            // error in favor of this new one
            Err(_) => key_result = key,
        }
    }

    key_result
}

fn config_from_pem(cert: Vec<u8>, key: Vec<u8>) -> io::Result<ServerConfig> {
    let cert = certificates_from_pem(&cert)?;
    let key = private_key_from_pem(&key)?;

    config_from_cert_and_key(cert, key)
}

async fn config_from_pem_file(
    cert: impl AsRef<Path>,
    key: impl AsRef<Path>,
) -> io::Result<ServerConfig> {
    let cert = fs_err::tokio::read(cert.as_ref()).await?;
    let key = fs_err::tokio::read(key.as_ref()).await?;

    config_from_pem(cert, key)
}

async fn config_from_pem_chain_file(
    cert: impl AsRef<Path>,
    chain: impl AsRef<Path>,
) -> io::Result<ServerConfig> {
    let cert = fs_err::tokio::read(cert.as_ref()).await?;
    let cert = certificates_from_pem(&cert)?;
    let key = fs_err::tokio::read(chain.as_ref()).await?;
    let key =
        PrivateKeyDer::from_pem_slice(&key).map_err(|_| io_other("could not parse pem file"))?;

    config_from_cert_and_key(cert, key)
}

#[cfg(test)]
mod tests {
    use crate::accept::{LocalAddr, LocalAddrAcceptor};
    use crate::handle::Handle;
    use crate::server::Server;
    use crate::tls::TlsInfo;
    use crate::tls_rustls::{self, RustlsAcceptor, RustlsConfig};
    use axum::body::Body;
    use axum::routing::get;
    use axum::Extension;
    use axum::Router;
    use bytes::Bytes;
    use http::{response, Request};
    use http_body_util::BodyExt;
    use hyper::client::conn::http1::{handshake, SendRequest};
    use hyper_util::rt::TokioIo;
    use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
    use rustls::{ClientConfig, DigitallySignedStruct, Error, SignatureScheme};
    use rustls_pki_types::{CertificateDer, ServerName, UnixTime};
    use std::fmt::Debug;
    use std::{convert::TryFrom, io, net::SocketAddr, sync::Arc, time::Duration};
    use tokio::time::{sleep, timeout};
    use tokio::{net::TcpStream, task::JoinHandle};
    use tokio_rustls::TlsConnector;

    // Under `--all-features` both aws-lc-rs and ring are compiled in. The
    // provider helper must pick one instead of letting rustls panic.
    #[cfg(any(feature = "tls-rustls", feature = "tls-rustls-ring"))]
    #[test]
    fn crypto_provider_is_installed() {
        let _ = super::crypto_provider();

        assert!(rustls::crypto::CryptoProvider::get_default().is_some());
    }

    #[tokio::test]
    async fn start_and_request() {
        let (_handle, _server_task, addr) = start_server().await;

        let (mut client, _conn) = connect(addr).await;

        let (_parts, body) = send_empty_request(&mut client).await;

        assert_eq!(body.as_ref(), b"Hello, world!");
    }

    #[tokio::test]
    async fn tls_timeout() {
        let (handle, _server_task, addr) = start_server().await;
        assert_eq!(handle.connection_count(), 0);

        // We intentionally avoid driving a TLS handshake to completion.
        let _stream = TcpStream::connect(addr).await.unwrap();

        sleep(Duration::from_millis(500)).await;
        assert_eq!(handle.connection_count(), 1);

        tokio::time::sleep(Duration::from_millis(1000)).await;
        // Timeout defaults to 1s during testing, and we have waited 1.5 seconds.
        assert_eq!(handle.connection_count(), 0);
    }

    #[tokio::test]
    async fn from_pem_chain_file_sets_alpn() {
        let config = RustlsConfig::from_pem_chain_file(
            "examples/self-signed-certs/cert.pem",
            "examples/self-signed-certs/key.pem",
        )
        .await
        .unwrap();

        assert_eq!(
            config.get_inner().alpn_protocols,
            vec![b"h2".to_vec(), b"http/1.1".to_vec()]
        );
    }

    #[tokio::test]
    async fn sni_pem_files_select_certificate_by_server_name() {
        let config = RustlsConfig::from_sni_pem_files([
            (
                "localhost",
                "examples/self-signed-certs/sni/localhost-cert.pem",
                "examples/self-signed-certs/sni/localhost-key.pem",
            ),
            (
                "other.test",
                "examples/self-signed-certs/sni/other-test-cert.pem",
                "examples/self-signed-certs/sni/other-test-key.pem",
            ),
        ])
        .await
        .unwrap();

        let (handle, server_task, addr) = start_server_with_config(config).await;

        let cert_a = get_first_cert_for(addr, "localhost").await;
        let cert_b = get_first_cert_for(addr, "other.test").await;

        assert_ne!(cert_a, cert_b);

        handle.shutdown();
        let _ = timeout(Duration::from_secs(1), server_task).await;
    }

    #[cfg(feature = "tls-rustls-encrypted-keys")]
    #[tokio::test]
    async fn encrypted_key_starts_and_serves() {
        let config = RustlsConfig::from_pem_file_with_password(
            "examples/self-signed-certs/cert.pem",
            "examples/self-signed-certs/encrypted-key.pem",
            b"axum-server",
        )
        .await
        .unwrap();

        let (handle, server_task, addr) = start_server_with_config(config).await;

        let (mut client, _conn) = connect(addr).await;
        let (_parts, body) = send_empty_request(&mut client).await;

        assert_eq!(body.as_ref(), b"Hello, world!");

        handle.shutdown();
        let _ = timeout(Duration::from_secs(1), server_task).await;
    }

    #[cfg(feature = "tls-rustls-encrypted-keys")]
    #[tokio::test]
    async fn wrong_password_is_rejected() {
        let error = RustlsConfig::from_pem_file_with_password(
            "examples/self-signed-certs/cert.pem",
            "examples/self-signed-certs/encrypted-key.pem",
            b"wrong-password",
        )
        .await
        .unwrap_err();

        assert!(error.to_string().contains("decrypt"), "{error}");
    }

    #[cfg(feature = "tls-rustls-encrypted-keys")]
    #[tokio::test]
    async fn reload_from_pem_file_with_password_works() {
        let config = RustlsConfig::from_pem_file(
            "examples/self-signed-certs/cert.pem",
            "examples/self-signed-certs/key.pem",
        )
        .await
        .unwrap();

        config
            .reload_from_pem_file_with_password(
                "examples/self-signed-certs/cert.pem",
                "examples/self-signed-certs/encrypted-key.pem",
                b"axum-server",
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn handshake_is_cancelled_on_shutdown() {
        let handle = Handle::new();
        let server_handle = handle.clone();

        let config = RustlsConfig::from_pem_file(
            "examples/self-signed-certs/cert.pem",
            "examples/self-signed-certs/key.pem",
        )
        .await
        .unwrap();

        // Make sure the test fails (via the timeout) rather than hanging if the
        // handshake is not cancelled.
        let acceptor = RustlsAcceptor::new(config).handshake_timeout(Duration::from_secs(30));
        let addr = SocketAddr::from(([127, 0, 0, 1], 0));

        let server_task = tokio::spawn(async move {
            Server::bind(addr)
                .acceptor(acceptor)
                .handle(server_handle)
                .serve(
                    Router::new()
                        .route("/", get(|| async { "Hello, world!" }))
                        .into_make_service(),
                )
                .await
        });

        let addr = handle.listening().await.unwrap();

        // Open a connection but never start the TLS handshake.
        let _stream = TcpStream::connect(addr).await.unwrap();

        sleep(Duration::from_millis(200)).await;
        assert_eq!(handle.connection_count(), 1);

        handle.shutdown();

        let result = tokio::time::timeout(Duration::from_secs(2), server_task)
            .await
            .expect("server did not shut down")
            .unwrap();
        assert!(result.is_ok());

        // The pending handshake task must be cancelled instead of running
        // until the (30 second) handshake timeout.
        sleep(Duration::from_millis(300)).await;
        assert_eq!(handle.connection_count(), 0);
    }

    #[tokio::test]
    async fn tls_info_is_available_to_handlers() {
        let handle = Handle::new();
        let server_handle = handle.clone();

        let config = RustlsConfig::from_pem_file(
            "examples/self-signed-certs/cert.pem",
            "examples/self-signed-certs/key.pem",
        )
        .await
        .unwrap();

        let addr = SocketAddr::from(([127, 0, 0, 1], 0));

        let server_task = tokio::spawn(async move {
            let app = Router::new().route(
                "/",
                get(
                    |Extension(info): Extension<Arc<TlsInfo>>,
                     Extension(local): Extension<LocalAddr>| async move {
                        format!(
                            "{}|{}|{}|{}",
                            info.server_name.as_deref().unwrap_or(""),
                            info.alpn_protocol.is_some(),
                            info.peer_certificates.len(),
                            local.0,
                        )
                    },
                ),
            );

            let acceptor = RustlsAcceptor::new(config).acceptor(LocalAddrAcceptor::new());

            Server::bind(addr)
                .acceptor(acceptor)
                .handle(server_handle)
                .serve(app.into_make_service())
                .await
        });

        let addr = handle.listening().await.unwrap();
        let (mut client, _conn) = connect(addr).await;
        let (_parts, body) = send_empty_request(&mut client).await;

        assert_eq!(
            String::from_utf8(body.to_vec()).unwrap(),
            format!("localhost|true|0|{addr}")
        );

        handle.shutdown();
        let result = timeout(Duration::from_secs(1), server_task)
            .await
            .unwrap()
            .unwrap();
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_reload() {
        let handle = Handle::new();

        let config = RustlsConfig::from_pem_file(
            "examples/self-signed-certs/cert.pem",
            "examples/self-signed-certs/key.pem",
        )
        .await
        .unwrap();

        let server_handle = handle.clone();
        let rustls_config = config.clone();
        tokio::spawn(async move {
            let app = Router::new().route("/", get(|| async { "Hello, world!" }));

            let addr = SocketAddr::from(([127, 0, 0, 1], 0));

            tls_rustls::bind_rustls(addr, rustls_config)
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
            .await
            .unwrap();

        cert_b = get_first_cert(addr).await;

        assert_ne!(cert_a, cert_b);

        config
            .reload_from_pem_file(
                "examples/self-signed-certs/cert.pem",
                "examples/self-signed-certs/key.pem",
            )
            .await
            .unwrap();

        cert_b = get_first_cert(addr).await;

        assert_eq!(cert_a, cert_b);
    }

    async fn start_server() -> (Handle<SocketAddr>, JoinHandle<io::Result<()>>, SocketAddr) {
        let config = RustlsConfig::from_pem_file(
            "examples/self-signed-certs/cert.pem",
            "examples/self-signed-certs/key.pem",
        )
        .await
        .unwrap();

        start_server_with_config(config).await
    }

    async fn start_server_with_config(
        config: RustlsConfig,
    ) -> (Handle<SocketAddr>, JoinHandle<io::Result<()>>, SocketAddr) {
        let handle = Handle::new();

        let server_handle = handle.clone();
        let server_task = tokio::spawn(async move {
            let app = Router::new().route("/", get(|| async { "Hello, world!" }));
            let addr = SocketAddr::from(([127, 0, 0, 1], 0));

            tls_rustls::bind_rustls(addr, config)
                .handle(server_handle)
                .serve(app.into_make_service())
                .await
        });

        let addr = handle.listening().await.unwrap();

        (handle, server_task, addr)
    }

    async fn get_first_cert(addr: SocketAddr) -> CertificateDer<'static> {
        get_first_cert_for(addr, "localhost").await
    }

    async fn get_first_cert_for(addr: SocketAddr, name: &'static str) -> CertificateDer<'static> {
        let stream = TcpStream::connect(addr).await.unwrap();
        let tls_stream = tls_connector()
            .connect(ServerName::try_from(name).unwrap(), stream)
            .await
            .unwrap();

        let (_io, client_connection) = tls_stream.into_inner();

        client_connection.peer_certificates().unwrap()[0].clone()
    }

    async fn connect(addr: SocketAddr) -> (SendRequest<Body>, JoinHandle<()>) {
        let stream = TcpStream::connect(addr).await.unwrap();
        let tls_stream = TokioIo::new(tls_connector().connect(dns_name(), stream).await.unwrap());

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

    fn tls_connector() -> TlsConnector {
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
            ) -> Result<HandshakeSignatureValid, Error> {
                Ok(HandshakeSignatureValid::assertion())
            }

            fn verify_tls13_signature(
                &self,
                _message: &[u8],
                _cert: &CertificateDer<'_>,
                _dss: &DigitallySignedStruct,
            ) -> Result<HandshakeSignatureValid, Error> {
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

        let mut client_config = ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoVerify))
            .with_no_client_auth();

        client_config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];

        TlsConnector::from(Arc::new(client_config))
    }

    fn dns_name() -> ServerName<'static> {
        ServerName::try_from("localhost").unwrap()
    }

    #[tokio::test]
    async fn from_pem_file_not_found() {
        let err = RustlsConfig::from_pem_file(
            "examples/self-signed-certs/missing.pem",
            "examples/self-signed-certs/key.pem",
        )
        .await
        .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        assert_eq!(
            err.to_string(),
            "failed to read from file `examples/self-signed-certs/missing.pem`: No such file or directory (os error 2)"
        );

        let err = RustlsConfig::from_pem_file(
            "examples/self-signed-certs/cert.pem",
            "examples/self-signed-certs/missing.pem",
        )
        .await
        .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        assert_eq!(
            err.to_string(),
            "failed to read from file `examples/self-signed-certs/missing.pem`: No such file or directory (os error 2)"
        );
    }

    #[tokio::test]
    async fn from_pem_file_chain_file_not_found() {
        let err = RustlsConfig::from_pem_chain_file(
            "examples/self-signed-certs/missing.pem",
            "examples/self-signed-certs/key.pem",
        )
        .await
        .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        assert_eq!(
            err.to_string(),
            "failed to read from file `examples/self-signed-certs/missing.pem`: No such file or directory (os error 2)"
        );

        let err = RustlsConfig::from_pem_chain_file(
            "examples/self-signed-certs/cert.pem",
            "examples/self-signed-certs/missing.pem",
        )
        .await
        .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        assert_eq!(
            err.to_string(),
            "failed to read from file `examples/self-signed-certs/missing.pem`: No such file or directory (os error 2)"
        );
    }
}
