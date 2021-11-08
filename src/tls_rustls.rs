//! Tls implementation using [`rustls`]

use crate::{
    handle::Handle,
    server::{accept, io_other},
    service::MakeServiceRef,
};
use futures_util::future::poll_fn;
use http::Request;
use hyper::server::conn::{AddrIncoming, AddrStream, Http};
use rustls::{Certificate, PrivateKey, ServerConfig};
use std::{
    fmt, io,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

/// HTTPS server using rustls.
#[derive(Debug)]
pub struct RustlsServer {
    addr: SocketAddr,
    handle: Handle,
    config: RustlsConfig,
}

/// Create a [`RustlsServer`] that will bind to provided address.
#[cfg_attr(docsrs, doc(cfg(feature = "tls-rustls")))]
pub fn bind_rustls(addr: SocketAddr, config: RustlsConfig) -> RustlsServer {
    RustlsServer::bind(addr, config)
}

impl RustlsServer {
    /// Create a server that will bind to provided address.
    pub fn bind(addr: SocketAddr, config: RustlsConfig) -> Self {
        let handle = Handle::new();

        Self {
            addr,
            handle,
            config,
        }
    }

    /// Provide a handle for additional utilities.
    pub fn handle(mut self, handle: Handle) -> Self {
        self.handle = handle;
        self
    }

    /// Serve provided [`MakeService`].
    ///
    /// [`MakeService`]: https://docs.rs/tower/0.4/tower/make/trait.MakeService.html
    pub async fn serve<M>(self, mut make_service: M) -> io::Result<()>
    where
        M: MakeServiceRef<AddrStream, Request<hyper::Body>>,
    {
        let handle = self.handle;
        let acceptor = tls_acceptor_from_config(self.config).await?;

        let listener = TcpListener::bind(self.addr).await?;
        let mut incoming = AddrIncoming::from_listener(listener).map_err(io_other)?;

        handle.notify_listening(incoming.local_addr());

        let accept_loop_future = async {
            loop {
                let addr_stream = tokio::select! {
                    biased;
                    result = accept(&mut incoming) => result?,
                    _ = handle.wait_graceful_shutdown() => return Ok(()),
                };

                poll_fn(|cx| make_service.poll_ready(cx))
                    .await
                    .map_err(io_other)?;

                let service = match make_service.make_service(&addr_stream).await {
                    Ok(service) => service,
                    Err(_) => continue,
                };

                let acceptor = acceptor.clone();
                let watcher = handle.watcher();

                tokio::spawn(async move {
                    if let Ok(stream) = acceptor.accept(addr_stream).await {
                        let serve_future = Http::new()
                            .serve_connection(stream, service)
                            .with_upgrades();

                        tokio::select! {
                            biased;
                            _ = watcher.wait_shutdown() => (),
                            _ = serve_future => (),
                        }
                    }
                });
            }
        };

        let result = tokio::select! {
            biased;
            _ = handle.wait_shutdown() => return Ok(()),
            result = accept_loop_future => result,
        };

        if let Err(e) = result {
            return Err(e);
        }

        handle.wait_connections_end().await;

        Ok(())
    }
}

/// Rustls configuration.
#[derive(Debug)]
pub struct RustlsConfig {
    inner: ConfigType,
}

impl RustlsConfig {
    /// Create config from [`ServerConfig`].
    ///
    /// Sets alpn protocols.
    pub fn from_config(config: ServerConfig) -> Self {
        let config = Config(config);
        let inner = ConfigType::Rustls { config };

        Self { inner }
    }

    /// Create config from DER-encoded data.
    ///
    /// The certificate must be DER-encoded X.509.
    ///
    /// The private key must be DER-encoded ASN.1 in either PKCS#8 or PKCS#1 format.
    pub fn from_der(cert: Vec<Vec<u8>>, key: Vec<u8>) -> Self {
        let inner = ConfigType::Der { cert, key };

        Self { inner }
    }

    /// Create config from PEM formatted data.
    ///
    /// Certificate and private key must be in PEM format.
    pub fn from_pem(cert: Vec<u8>, key: Vec<u8>) -> Self {
        let inner = ConfigType::Pem { cert, key };

        Self { inner }
    }

    /// Create config from PEM formatted files.
    ///
    /// Contents of certificate file and private key file must be in PEM format.
    pub fn from_pem_file(cert: impl AsRef<Path>, key: impl AsRef<Path>) -> Self {
        let (cert, key) = (cert.as_ref().to_owned(), key.as_ref().to_owned());
        let inner = ConfigType::PemFile { cert, key };

        Self { inner }
    }
}

#[derive(Debug)]
enum ConfigType {
    Rustls { config: Config },
    Der { cert: Vec<Vec<u8>>, key: Vec<u8> },
    Pem { cert: Vec<u8>, key: Vec<u8> },
    PemFile { cert: PathBuf, key: PathBuf },
}

async fn tls_acceptor_from_config(config: RustlsConfig) -> io::Result<TlsAcceptor> {
    let mut server_config = match config.inner {
        ConfigType::Rustls { config } => config.0,
        ConfigType::Der { cert, key } => config_from_der(cert, key)?,
        ConfigType::Pem { cert, key } => config_from_pem(cert, key)?,
        ConfigType::PemFile { cert, key } => config_from_pemfile(cert, key).await?,
    };

    server_config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];

    Ok(TlsAcceptor::from(Arc::new(server_config)))
}

fn config_from_der(cert: Vec<Vec<u8>>, key: Vec<u8>) -> io::Result<ServerConfig> {
    let cert = cert.into_iter().map(Certificate).collect();
    let key = PrivateKey(key);

    let config = ServerConfig::builder()
        .with_safe_defaults()
        .with_no_client_auth()
        .with_single_cert(cert, key)
        .map_err(io_other)?;

    Ok(config)
}

fn config_from_pem(cert: Vec<u8>, key: Vec<u8>) -> io::Result<ServerConfig> {
    use rustls_pemfile::Item;

    let cert = rustls_pemfile::certs(&mut cert.as_ref())?;
    let key = match rustls_pemfile::read_one(&mut key.as_ref())? {
        Some(Item::RSAKey(key)) | Some(Item::PKCS8Key(key)) => key,
        _ => return Err(io_other("private key not found")),
    };

    config_from_der(cert, key)
}

async fn config_from_pemfile(cert: PathBuf, key: PathBuf) -> io::Result<ServerConfig> {
    let cert = tokio::fs::read(cert).await?;
    let key = tokio::fs::read(key).await?;

    config_from_pem(cert, key)
}

struct Config(ServerConfig);

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ServerConfig").finish()
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        handle::Handle,
        tls_rustls::{RustlsConfig, RustlsServer},
    };
    use axum::{routing::get, Router};
    use http::Request;
    use hyper::{
        client::conn::{handshake, SendRequest},
        Body,
    };
    use rustls::{
        client::{ServerCertVerified, ServerCertVerifier},
        Certificate, ClientConfig, ServerName,
    };
    use std::{
        convert::TryFrom,
        net::SocketAddr,
        sync::Arc,
        time::{Duration, SystemTime},
    };
    use tokio::{net::TcpStream, task::JoinHandle, time::timeout};
    use tokio_rustls::TlsConnector;
    use tower::{Service, ServiceExt};

    #[tokio::test]
    async fn start_and_request() {
        let handle = Handle::new();

        let server_handle = handle.clone();
        tokio::spawn(async move {
            let app = Router::new().route("/", get(|| async { "Hello, world!" }));

            let config = RustlsConfig::from_pem_file(
                "examples/self-signed-certs/cert.pem",
                "examples/self-signed-certs/key.pem",
            );

            let addr = SocketAddr::from(([127, 0, 0, 1], 0));

            RustlsServer::bind(addr, config)
                .handle(server_handle)
                .serve(app.into_make_service())
                .await
        });

        let addr = handle.listening().await;

        let (client, _conn) = connect(addr).await;
        let response = client.oneshot(Request::new(Body::empty())).await.unwrap();

        let body = hyper::body::to_bytes(response.into_body()).await.unwrap();

        assert_eq!(body.as_ref(), b"Hello, world!");
    }

    #[tokio::test]
    async fn test_shutdown() {
        let handle = Handle::new();

        let server_handle = handle.clone();
        tokio::spawn(async move {
            let app = Router::new().route("/", get(|| async { "Hello, world!" }));

            let config = RustlsConfig::from_pem_file(
                "examples/self-signed-certs/cert.pem",
                "examples/self-signed-certs/key.pem",
            );

            let addr = SocketAddr::from(([127, 0, 0, 1], 0));

            RustlsServer::bind(addr, config)
                .handle(server_handle)
                .serve(app.into_make_service())
                .await
        });

        let addr = handle.listening().await;

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
        let handle = Handle::new();

        let server_handle = handle.clone();
        let server_task = tokio::spawn(async move {
            let app = Router::new().route("/", get(|| async { "Hello, world!" }));

            let config = RustlsConfig::from_pem_file(
                "examples/self-signed-certs/cert.pem",
                "examples/self-signed-certs/key.pem",
            );

            let addr = SocketAddr::from(([127, 0, 0, 1], 0));

            RustlsServer::bind(addr, config)
                .handle(server_handle)
                .serve(app.into_make_service())
                .await
        });

        let addr = handle.listening().await;

        let (mut client, conn) = connect(addr).await;

        handle.graceful_shutdown(None);

        let response = client
            .ready()
            .await
            .unwrap()
            .call(Request::new(Body::empty()))
            .await
            .unwrap();
        let body = hyper::body::to_bytes(response.into_body()).await.unwrap();

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
        let handle = Handle::new();

        let server_handle = handle.clone();
        let server_task = tokio::spawn(async move {
            let app = Router::new().route("/", get(|| async { "Hello, world!" }));

            let config = RustlsConfig::from_pem_file(
                "examples/self-signed-certs/cert.pem",
                "examples/self-signed-certs/key.pem",
            );

            let addr = SocketAddr::from(([127, 0, 0, 1], 0));

            RustlsServer::bind(addr, config)
                .handle(server_handle)
                .serve(app.into_make_service())
                .await
        });

        let addr = handle.listening().await;

        let (mut client, _conn) = connect(addr).await;

        handle.graceful_shutdown(Some(Duration::from_millis(250)));

        let response = client
            .ready()
            .await
            .unwrap()
            .call(Request::new(Body::empty()))
            .await
            .unwrap();
        let body = hyper::body::to_bytes(response.into_body()).await.unwrap();

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

    async fn connect(addr: SocketAddr) -> (SendRequest<Body>, JoinHandle<()>) {
        let stream = TcpStream::connect(addr).await.unwrap();
        let tls_stream = tls_connector().connect(dns_name(), stream).await.unwrap();

        let (send_request, connection) = handshake(tls_stream).await.unwrap();

        let task = tokio::spawn(async move {
            let _ = connection.await;
        });

        (send_request, task)
    }

    fn tls_connector() -> TlsConnector {
        struct NoVerify;

        impl ServerCertVerifier for NoVerify {
            fn verify_server_cert(
                &self,
                _end_entity: &Certificate,
                _intermediates: &[Certificate],
                _server_name: &ServerName,
                _scts: &mut dyn Iterator<Item = &[u8]>,
                _ocsp_response: &[u8],
                _now: SystemTime,
            ) -> Result<ServerCertVerified, rustls::Error> {
                Ok(ServerCertVerified::assertion())
            }
        }

        let mut client_config = ClientConfig::builder()
            .with_safe_defaults()
            .with_custom_certificate_verifier(Arc::new(NoVerify))
            .with_no_client_auth();

        client_config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];

        TlsConnector::from(Arc::new(client_config))
    }

    fn dns_name() -> ServerName {
        ServerName::try_from("localhost").unwrap()
    }
}
