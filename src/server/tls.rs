//! Tls types.
//!
//! [`TlsServer`](TlsServer) can be used just like [`Server`](crate::server::Server) to serve apps.
//!
//! # Example
//!
//! ```rust,no_run
//! use axum::{
//!     handler::get,
//!     Router,
//! };
//!
//! use axum_server::Server;
//!
//! #[tokio::main]
//! async fn main() {
//!     let app = Router::new().route("/", get(|| async { "Hello, world" }));
//!
//!     Server::new()
//!         .bind("127.0.0.1:3000")
//!         .bind_rustls("127.0.0.1:3443")
//!         .private_key_file("certs/key.pem")
//!         .certificate_file("certs/cert.pem")
//!         .serve_and_record(app)
//!         .await
//!         .unwrap();
//! }
//! ```

#[cfg(feature = "record")]
use crate::server::record::RecordingHttpServer;

use crate::server::serve::{Accept, HttpServer, Serve};
use crate::server::{collect_addrs, BoxedToSocketAddrs, NoopAcceptor, Server};

use std::{
    fs::File,
    io::{self, BufRead, BufReader, Cursor, ErrorKind, Seek, SeekFrom},
    net::{SocketAddr, ToSocketAddrs},
    path::{Path, PathBuf},
    sync::Arc,
};

use parking_lot::RwLock;

use futures_util::stream::{FuturesUnordered, StreamExt};

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::task::spawn_blocking;

use rustls::{
    internal::pemfile::certs, internal::pemfile::pkcs8_private_keys,
    internal::pemfile::rsa_private_keys, Certificate, NoClientAuth, PrivateKey, ServerConfig,
};

use tokio_rustls::{server::TlsStream, Accept as AcceptFuture, TlsAcceptor as CoreTlsAcceptor};

use http::request::Request;
use http::response::Response;
use http_body::Body;

use tower_service::Service;

/// A struct that can be passed to `TlsServer` to reload tls configuration.
///
/// # Example
///
/// ```rust,no_run
/// use axum::{
///     handler::get,
///     Router,
/// };
/// use axum_server::tls::TlsLoader;
/// use tokio::time::{sleep, Duration};
///
/// #[tokio::main]
/// async fn main() {
///     let app = Router::new().route("/", get(|| async { "Hello, World!" }));
///
///     let mut loader = TlsLoader::new();
///
///     // Must be loaded before passing.
///     loader
///         .private_key_file("certs/key.pem")
///         .certificate_file("certs/cert.pem")
///         .load()
///         .await
///         .unwrap();
///
///     tokio::spawn(reload_every_day(loader.clone()));
///
///     axum_server::bind_rustls("127.0.0.1:3000")
///         .loader(loader)
///         .serve(app)
///         .await
///         .unwrap();
/// }
///
/// async fn reload_every_day(mut loader: TlsLoader) {
///     loop {
///         // Sleep first since certificates are loaded after loader is built.
///         sleep(Duration::from_secs(3600 * 24)).await;
///
///         // Can be loaded with recent settings.
///         // For example: Read previously provided file contents again.
///         loader.load().await.unwrap();
///
///         // Can overwrite settings and load.
///         loader
///             .private_key_file("certs/private_key.pem")
///             .certificate_file("certs/fullchain.pem")
///             .load()
///             .await
///             .unwrap();
///     }
/// }
/// ```
#[derive(Default, Clone)]
pub struct TlsLoader {
    server_config: Option<Arc<ServerConfig>>,
    private_key: Option<Vec<u8>>,
    certificate: Option<Vec<u8>>,
    private_key_path: Option<PathBuf>,
    certificate_path: Option<PathBuf>,
    acceptor: Option<Arc<RwLock<CoreTlsAcceptor>>>,
}

impl TlsLoader {
    /// Create a `TlsLoader`.
    pub fn new() -> Self {
        TlsLoader::default()
    }

    /// Provide [`ServerConfig`](ServerConfig) containing private key and certificate(s).
    ///
    /// When this value is set, other tls configurations are ignored.
    ///
    /// Successive calls will overwrite last value.
    pub fn config(&mut self, config: Arc<ServerConfig>) -> &mut Self {
        self.server_config = Some(config);
        self
    }

    /// Set private key in PEM format.
    ///
    /// Successive calls will overwrite last private key.
    pub fn private_key(&mut self, private_key: Vec<u8>) -> &mut Self {
        self.private_key = Some(private_key);
        self
    }

    /// Set certificate(s) in PEM format.
    ///
    /// Successive calls will overwrite last certificate.
    pub fn certificate(&mut self, certificate: Vec<u8>) -> &mut Self {
        self.certificate = Some(certificate);
        self
    }

    /// Set private key from file in PEM format.
    ///
    /// Successive calls will overwrite last private key.
    pub fn private_key_file(&mut self, path: impl AsRef<Path>) -> &mut Self {
        self.private_key_path = Some(path.as_ref().to_owned());
        self
    }

    /// Set certificate(s) from file in PEM format.
    ///
    /// Successive calls will overwrite last certificate.
    pub fn certificate_file(&mut self, path: impl AsRef<Path>) -> &mut Self {
        self.certificate_path = Some(path.as_ref().to_owned());
        self
    }

    /// Load private key or certificate(s).
    ///
    /// Will apply to connections after this function is called.
    pub async fn load(&mut self) -> io::Result<()> {
        let config = self.get_server_config().await?;
        let acceptor = CoreTlsAcceptor::from(config);

        match &self.acceptor {
            Some(lock) => *lock.write() = acceptor,
            None => self.acceptor = Some(Arc::new(RwLock::new(acceptor))),
        }

        Ok(())
    }

    async fn get_acceptor(&mut self) -> io::Result<Arc<RwLock<CoreTlsAcceptor>>> {
        match &self.acceptor {
            Some(acceptor) => Ok(acceptor.clone()),
            None => Err(invalid_input("`TlsLoader` is not loaded.")),
        }
    }

    async fn get_server_config(&mut self) -> io::Result<Arc<ServerConfig>> {
        if let Some(config) = &self.server_config {
            return Ok(config.clone());
        }

        let key = match &self.private_key {
            Some(value) => pkey_from_value(&value).await?,
            None => match &self.private_key_path {
                Some(path) => pkey_from_file(&path).await?,
                None => return Err(invalid_input("private_key is not set")),
            },
        };

        let certs = match &self.certificate {
            Some(value) => cert_from_value(&value).await?,
            None => match &self.certificate_path {
                Some(path) => cert_from_file(&path).await?,
                None => return Err(invalid_input("certificate is not set")),
            },
        };

        Ok(rustls_config(key, certs)?)
    }
}

/// Configurable HTTP and HTTPS server, supporting HTTP/1.1 and HTTP2.
///
/// See [module](crate::server::tls) page for examples.
#[derive(Default)]
pub struct TlsServer {
    server: Server,
    tls_addrs: Vec<BoxedToSocketAddrs>,
    tls_loader: TlsLoader,
}

impl TlsServer {
    /// Create a new `TlsServer`.
    pub fn new() -> Self {
        TlsServer::default()
    }

    /// Bind to a single address or multiple addresses.
    pub fn bind<A>(mut self, addr: A) -> Self
    where
        A: ToSocketAddrs<Iter = std::vec::IntoIter<SocketAddr>> + Send + 'static,
    {
        self.server = self.server.bind(addr);
        self
    }

    /// Bind to a single address or multiple addresses. Using tls protocol on streams.
    ///
    /// Certificate and private key must be set before or after calling this.
    pub fn bind_rustls<A>(mut self, addr: A) -> Self
    where
        A: ToSocketAddrs<Iter = std::vec::IntoIter<SocketAddr>> + Send + 'static,
    {
        self.tls_addrs.push(Box::new(addr));
        self
    }

    /// Provide a **loaded** [`TlsLoader`](TlsLoader).
    ///
    /// This will overwrite any previously set private key and certificate(s) with its own ones.
    pub fn loader(mut self, loader: TlsLoader) -> Self {
        self.tls_loader = loader;
        self
    }

    /// Provide [`ServerConfig`](ServerConfig) containing private key and certificate(s).
    ///
    /// When this value is set, other tls configurations are ignored.
    ///
    /// Successive calls will overwrite last value.
    pub fn config(mut self, config: Arc<ServerConfig>) -> Self {
        self.tls_loader.config(config);
        self
    }

    /// Set private key in PEM format.
    ///
    /// Successive calls will overwrite last private key.
    pub fn private_key(mut self, private_key: Vec<u8>) -> Self {
        self.tls_loader.private_key(private_key);
        self
    }

    /// Set certificate(s) in PEM format.
    ///
    /// Successive calls will overwrite last certificate.
    pub fn certificate(mut self, certificate: Vec<u8>) -> Self {
        self.tls_loader.certificate(certificate);
        self
    }

    /// Set private key from file in PEM format.
    ///
    /// Successive calls will overwrite last private key.
    pub fn private_key_file(mut self, path: impl AsRef<Path>) -> Self {
        self.tls_loader.private_key_file(path);
        self
    }

    /// Set certificate(s) from file in PEM format.
    ///
    /// Successive calls will overwrite last certificate.
    pub fn certificate_file(mut self, path: impl AsRef<Path>) -> Self {
        self.tls_loader.certificate_file(path);
        self
    }

    /// Serve provided cloneable service on all binded addresses.
    ///
    /// If accepting connection fails in any one of binded addresses, listening in all
    /// binded addresses will be stopped and then an error will be returned.
    pub async fn serve<S, B>(self, service: S) -> io::Result<()>
    where
        S: Service<Request<hyper::Body>, Response = Response<B>> + Send + Sync + 'static + Clone,
        S::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
        S::Future: Send,
        B: Body + Send + 'static,
        B::Data: Send,
        B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        let service2 = service.clone();

        self.custom_serve(
            move || HttpServer::new(NoopAcceptor::new(), service.clone()),
            move |acceptor| HttpServer::new(TlsAcceptor::new(acceptor), service2.clone()),
        )
        .await
    }

    /// Serve provided cloneable service on all binded addresses.
    ///
    /// Record sent and received bytes for each connection. Sent and received bytes
    /// through a connection can be accessed through [`Request`](Request) extensions.
    ///
    /// See [`axum_server::record`](crate::server::record) module for examples.
    ///
    /// If accepting connection fails in any one of binded addresses, listening in all
    /// binded addresses will be stopped and then an error will be returned.
    #[cfg(feature = "record")]
    #[cfg_attr(docsrs, doc(cfg(feature = "record")))]
    pub async fn serve_and_record<S, B>(self, service: S) -> io::Result<()>
    where
        S: Service<Request<hyper::Body>, Response = Response<B>> + Send + Sync + 'static + Clone,
        S::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
        S::Future: Send,
        B: Body + Send + 'static,
        B::Data: Send,
        B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        let service2 = service.clone();

        self.custom_serve(
            move || RecordingHttpServer::new(NoopAcceptor::new(), service.clone()),
            move |acceptor| RecordingHttpServer::new(TlsAcceptor::new(acceptor), service2.clone()),
        )
        .await
    }

    async fn custom_serve<F1, A1, F2, A2>(
        mut self,
        make_server: F1,
        make_tls_server: F2,
    ) -> io::Result<()>
    where
        F1: Fn() -> A1,
        A1: Serve + Send + Sync + 'static,
        F2: Fn(Arc<RwLock<CoreTlsAcceptor>>) -> A2,
        A2: Serve + Send + Sync + 'static,
    {
        if self.server.addrs.is_empty() && self.tls_addrs.is_empty() {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "no address provided to bind",
            ));
        }

        let mut fut_list = FuturesUnordered::new();

        if !self.server.addrs.is_empty() {
            let http_server = make_server();

            let addrs = collect_addrs(self.server.addrs).await.unwrap()?;

            for addr in addrs {
                fut_list.push(http_server.serve_on(addr));
            }
        }

        if !self.tls_addrs.is_empty() {
            let acceptor = self.tls_loader.get_acceptor().await?;
            let http_server = make_tls_server(acceptor);

            let addrs = collect_addrs(self.tls_addrs).await.unwrap()?;

            for addr in addrs {
                fut_list.push(http_server.serve_on(addr));
            }
        }

        while let Some(handle) = fut_list.next().await {
            if let Err(e) = handle.unwrap() {
                fut_list.iter().for_each(|handle| handle.abort());

                return Err(e);
            }
        }

        Ok(())
    }
}

impl From<Server> for TlsServer {
    fn from(server: Server) -> Self {
        let mut tls_server = TlsServer::default();
        tls_server.server = server;
        tls_server
    }
}

async fn pkey_from_value(key: &[u8]) -> io::Result<PrivateKey> {
    let key = key.to_vec();
    spawn_blocking(move || {
        let mut reader = Cursor::new(key);

        Ok(load_private_key(&mut reader)?)
    })
    .await
    .unwrap()
}

async fn cert_from_value(cert: &[u8]) -> io::Result<Vec<Certificate>> {
    let cert = cert.to_vec();
    spawn_blocking(move || {
        let mut reader = Cursor::new(cert);

        Ok(load_certificates(&mut reader)?)
    })
    .await
    .unwrap()
}

async fn pkey_from_file(path: &Path) -> io::Result<PrivateKey> {
    let path = path.to_owned();
    spawn_blocking(move || {
        let mut reader = BufReader::new(File::open(path)?);

        Ok(load_private_key(&mut reader)?)
    })
    .await
    .unwrap()
}

async fn cert_from_file(path: &Path) -> io::Result<Vec<Certificate>> {
    let path = path.to_owned();
    spawn_blocking(move || {
        let mut reader = BufReader::new(File::open(path)?);

        Ok(load_certificates(&mut reader)?)
    })
    .await
    .unwrap()
}

fn load_private_key<R: BufRead + Seek>(reader: &mut R) -> io::Result<PrivateKey> {
    let key = pkcs8_private_keys(reader)
        .and_then(try_get_first_key)
        .or_else(|_| {
            reader.seek(SeekFrom::Start(0)).unwrap();
            rsa_private_keys(reader).and_then(try_get_first_key)
        })
        .map_err(|_| io::Error::new(ErrorKind::InvalidData, "invalid private key"))?;

    Ok(key)
}

fn load_certificates(reader: &mut dyn BufRead) -> io::Result<Vec<Certificate>> {
    let certs =
        certs(reader).map_err(|_| io::Error::new(ErrorKind::InvalidData, "invalid certificate"))?;

    Ok(certs)
}

fn try_get_first_key(mut keys: Vec<PrivateKey>) -> Result<PrivateKey, ()> {
    if !keys.is_empty() {
        Ok(keys.remove(0))
    } else {
        Err(())
    }
}

fn rustls_config(
    private_key: PrivateKey,
    certificates: Vec<Certificate>,
) -> io::Result<Arc<ServerConfig>> {
    let mut config = ServerConfig::new(NoClientAuth::new());

    config
        .set_single_cert(certificates, private_key)
        .map_err(|e| io::Error::new(ErrorKind::Other, e))?;

    config.set_protocols(&[b"h2".to_vec(), b"http/1.1".to_vec()]);

    Ok(Arc::new(config))
}

fn invalid_input(msg: &'static str) -> io::Error {
    io::Error::new(ErrorKind::InvalidInput, msg)
}

#[derive(Clone)]
pub(crate) struct TlsAcceptor {
    inner: Arc<RwLock<CoreTlsAcceptor>>,
}

impl TlsAcceptor {
    fn new(inner: Arc<RwLock<CoreTlsAcceptor>>) -> Self {
        Self { inner }
    }
}

impl<I> Accept<I> for TlsAcceptor
where
    I: AsyncRead + AsyncWrite + Unpin,
{
    type Stream = TlsStream<I>;
    type Future = AcceptFuture<I>;

    fn accept(&self, stream: I) -> Self::Future {
        self.inner.read().accept(stream)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::bind_rustls;

    // Self Signed Certificate
    const CERTIFICATE: &'static str = r#"-----BEGIN CERTIFICATE-----
MIIDkzCCAnugAwIBAgIUaVoRuh53PqMETXoouyFrcDmZeSkwDQYJKoZIhvcNAQEL
BQAwWTELMAkGA1UEBhMCVVMxEzARBgNVBAgMClNvbWUtU3RhdGUxITAfBgNVBAoM
GEludGVybmV0IFdpZGdpdHMgUHR5IEx0ZDESMBAGA1UEAwwJbG9jYWxob3N0MB4X
DTIxMDgyMTExMDg1OVoXDTIyMDgyMTExMDg1OVowWTELMAkGA1UEBhMCVVMxEzAR
BgNVBAgMClNvbWUtU3RhdGUxITAfBgNVBAoMGEludGVybmV0IFdpZGdpdHMgUHR5
IEx0ZDESMBAGA1UEAwwJbG9jYWxob3N0MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8A
MIIBCgKCAQEAoUG6X1vBuwCoVeE+SlRPvJnbLPdwBlzuipE9sQIVkA+XJi56qKOU
/V054bUvJ32JD6MM2LH/jqjg+DE/7t2hd0wylRmJK8Y3PcZI4ulgNSLg2MTu6EvG
3TfjAW7hFV18XwGJfDwN46/XN5HpEKgj0FCuxTyENhkgAH2yKVfYc/RMhdPssG/M
spKWYji1MS/Pq5qBzbgk3Ish4Yet85/kbzxrrVQVMGOqyv5iR1sWX9AtHkGUfNrW
NA8hDIWnZH6xa4gVWF+ZDtVEXmxGSr7R3AlED9aKd2bfDXJAFbLCPUvKRe9tbDN1
LnGeVjYrpAMwTKb5x6LbYcOSGhfw2+ivEwIDAQABo1MwUTAdBgNVHQ4EFgQUm+sW
WZboXk7yY4ixgiIBH8pvJX0wHwYDVR0jBBgwFoAUm+sWWZboXk7yY4ixgiIBH8pv
JX0wDwYDVR0TAQH/BAUwAwEB/zANBgkqhkiG9w0BAQsFAAOCAQEAa55VErxh6DyV
qbiItDSecqLgK3gJ97M3igV2ZXiqtwkjPbjC6RiTj+qB58PEgEK57DMQ100CwC4H
08vfwPyaDgxAgdDMf6y/ZpQXrPDGFC3MK7aDgN5Ewk++i5rLUIF7uzdUQG7IgCOO
ofDgLTGmBldeM99QgKkxq0b2UGyC25AhFiEoKUDM9cLl1fHYIauicXDXyZzrlqyY
W28VYPMeMGOjPUbSP/CN85N1Yaoqxnf3CmhJbgkdDstKxZr92Dyx7RHBQFN5sGnK
w5yD87DSEgRZ1aqKPmenXFkGspsLcLL0VBRr5ItNSQ46xWSjEbdCQXvCd1XEsLXh
Fll6LboxLg==
-----END CERTIFICATE-----"#;

    // Self Signed Private Key
    const PRIVATE_KEY: &'static str = r#"-----BEGIN PRIVATE KEY-----
MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQChQbpfW8G7AKhV
4T5KVE+8mdss93AGXO6KkT2xAhWQD5cmLnqoo5T9XTnhtS8nfYkPowzYsf+OqOD4
MT/u3aF3TDKVGYkrxjc9xkji6WA1IuDYxO7oS8bdN+MBbuEVXXxfAYl8PA3jr9c3
kekQqCPQUK7FPIQ2GSAAfbIpV9hz9EyF0+ywb8yykpZiOLUxL8+rmoHNuCTciyHh
h63zn+RvPGutVBUwY6rK/mJHWxZf0C0eQZR82tY0DyEMhadkfrFriBVYX5kO1URe
bEZKvtHcCUQP1op3Zt8NckAVssI9S8pF721sM3UucZ5WNiukAzBMpvnHotthw5Ia
F/Db6K8TAgMBAAECggEAHtpPiU0p/Nh8XKoS2+/TrbcWOz1AXsdLEJIHZqWKcJy7
A6Ai8b8Sk4NRvsCGvByFq8s7ev5bcfUXzgTGQbJ/4S5gAyz2lLyA9z3H1jpmoOQC
dxb+ys0syEiYEz8eq5LOZ/MIVg+7bJPJLqWpHPm+mG3Hco9IkH0wJUfnXYekL1MB
zXbyQIf+YV+qIlEDFQ+HMozBPOtEDomq/9WuZ6EPEG7Ulkpl1JQ70+YzkuNlGbco
pYVSryjHg5e7MCl4sGs/nviO1kmBUA+OYt2sbA8h+kUuLbk3WThbMPc7Cy3Llx6P
92Q4ERVvgR4yNi5ZUZNRXPv1K2368Ekh21VeRNkxoQKBgQDLlGH0hJV3bXX1hPjJ
kSm+ULwPUKJQDucaF6CaOodMRLCnQy88CXOEqF00R2sI3BPIpJMyyqHjrXxOYBDQ
SjotzPFNoL8WRzpFEszZmd5Q7lRGVjcH7aJ4Cg34QPKL8DVzNFFmafJxAe2bzw3J
fC4xIdEGnu0CnP6ey6e+rMhncQKBgQDKx36slJVeXj5ODl5KrpFsmARIcAyk+M5x
zN6ZcYcITYqqEztn4gghO46sQJjF+T8xPKhQVR1Z5/6/fAkwKScDA0ITq0nfS3oe
LTi+v98ijmFLp/R17rDlfFRLYAi8S4hSlAMuSJkpcVhaZFtfhf/aEJLmSfHspg/Q
Z8psfRYkwwKBgHvy5gkYSGCkdrN7uHYROhcz1KyGbazMxgxu4kvE4uee0uej0jh9
kKXuVIEmEpccV7dL7It6MEMNN6gIeXQ4HWARbcHT40RPLb0siyjZtDAWS51flLXx
C4CGrqa99G8bW4+/BOiUDRadE+xPjpdkUkN70WZ0kN2MdMJ+QK2pSYMhAoGBAMCj
zR+++DfyaFZXKBTiypzTvh3i9OA0zksmScKUK6gjojv4kVMbVIXdwqi5pWlOZE4u
RegrM/sZftYCy+fI8JrYGYn+C+vqFFVeuK3eMejuQlhRctgmrj8VYi9JSIM5boSk
wHDT302TtFALTxLshidv316PmRksmZFvSMrP+p1pAoGAUgzxEfQXfagS+hE/g1cs
ZOwXFVl9mCxXHeYG/tnW4TStiro0hP3lwGUKPaFcR3vHbXLoDrmLycMLP13eOCSt
7t/QgTOtGGRHGOOSqJeDM++kcbvnRY6w6Y4bB7geiUswFvtuZ3TAQJuIOAXr9DCW
SfyHiEc0jh9LdjUlMvCXaB8=
-----END PRIVATE KEY-----"#;

    #[tokio::test]
    async fn test_bind_addresses() {
        let server = bind_rustls("127.0.0.1:3443").bind_rustls("127.0.0.1:3444");

        let addrs = collect_addrs(server.tls_addrs).await.unwrap().unwrap();

        assert_eq!(addrs, [addr("127.0.0.1:3443"), addr("127.0.0.1:3444")]);
    }

    #[tokio::test]
    async fn test_key_cert() {
        let key = PRIVATE_KEY.as_bytes().to_vec();
        let cert = CERTIFICATE.as_bytes().to_vec();

        let server = Server::new().private_key(key).certificate(cert);

        let private_key = server.tls_loader.private_key.unwrap();
        let certificate = server.tls_loader.certificate.unwrap();

        let private_key = pkey_from_value(private_key).await.unwrap();
        let certificate = cert_from_value(certificate).await.unwrap();

        rustls_config(private_key, certificate).unwrap();
    }

    fn addr(s: &'static str) -> SocketAddr {
        s.parse().unwrap()
    }
}
